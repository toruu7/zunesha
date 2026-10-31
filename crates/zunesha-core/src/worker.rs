//! Agent process orchestration: a bounded pool that runs coding agents.
//!
//! Phase 4 turns `TODO` tasks into running subprocesses; Phase 8 adds the
//! self-healing validation loop around them. Each worker:
//!
//! 1. Claims the task (`IN_PROGRESS`) and spins up its isolated workspace.
//! 2. Spawns the agent CLI (e.g. `opencode`) inside that workspace.
//! 3. Streams `stdout`/`stderr` line-by-line, persisting them to the task's
//!    `worker_logs` and broadcasting them on a [`tokio::sync::broadcast`]
//!    channel so the daemon (Phase 5) can relay them to the web UI.
//!    OpenCode's `--format json` stream is translated into that readable
//!    transcript, and its final assistant message is stored separately in
//!    `agent_output` for the dashboard's Markdown Output tab.
//! 4. On a successful exit, moves the task to `EVALUATING` and runs the
//!    project's test/lint command ([`crate::eval`]) in the workspace:
//!    * pass → `REVIEW` (with the diff captured for the human),
//!    * fail → the error output is appended to the task's description and the
//!      agent runs again in the same workspace, up to 3 failures; then the
//!      task parks in `FAILED_NEEDS_HUMAN`.
//!
//!    A non-zero agent exit still returns the task to `TODO` as before.
//!
//! Phase 9 adds a second run mode, `RunMode::ResolveConflicts`, reached via
//! [`WorkerPool::spawn_resolution`] when an approval hit a merge conflict: the
//! workspace already exists (rebased onto the repository's current content,
//! conflict markers left in place), so the worker skips workspace creation and
//! instead runs the agent with the strict conflict-resolution prompt. Its
//! differences from a normal run:
//!
//! * the agent gets the conflict-resolution text (keep the new logic,
//!   integrate it with the updated base),
//! * a failed agent exit parks in `FAILED_NEEDS_HUMAN` rather than `TODO` —
//!   there is no from-scratch re-plan to fall back to, a human must look,
//! * validation retries loop back to `RESOLVING_CONFLICTS`, not `IN_PROGRESS`.
//!
//! Either way a green validation returns the task to `REVIEW`, so the now-clean
//! diff is still approved by a human (the merge-only-after-approval rule).
//!
//! Phase 10 appends the workspace's `CODEBASE_MAP.md` — a bounded repository
//! tree ([`crate::codebase_map`]) — to every prompt in both run modes, so the
//! agent orients itself before touching a file. The map rides along from
//! workspace setup; a workspace without one (pre-Phase 10) simply runs with
//! the shorter prompt.
//!
//! Concurrency is capped by a [`tokio::sync::Semaphore`]: at most
//! `max_concurrency` agents run at once, no matter how many tasks are ready.
//! A task holds its slot for the *whole* loop — retries and evaluations
//! count against the cap exactly like the initial agent run.
//!
//! ## Database access
//! `rusqlite::Connection` is `Send` but not `Sync`, so the pool does **not**
//! share one connection. Instead each worker opens its own short-lived
//! [`Database`] from the shared path — safe because Phase 3 added WAL mode
//! *and* a `busy_timeout`, so concurrent writers queue instead of failing.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, Semaphore};

use crate::config::{AgentProvider, Config};
use crate::control::{RunControl, RunRegistry};
use crate::db::{Database, RunRecord};
use crate::error::{Error, Result};
use crate::eval::{self, EvalCommand};
use crate::paths::ZuneshaPaths;
use crate::sandbox::{redact_secrets, SandboxMode, SandboxPolicy};
use crate::task::{Task, TaskStatus, MAX_EVAL_RETRIES};
use crate::workspace::WorkspaceManager;

/// Flush accumulated logs to the database at least this often while an agent
/// runs, so the UI shows progress even for a long, quiet step.
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(400);

/// Default cap on simultaneously running agents.
pub const DEFAULT_MAX_CONCURRENCY: usize = 3;

/// Default wall-clock limit for one agent attempt (30 minutes).
pub const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 30 * 60;

/// Grace period between SIGTERM and SIGKILL when cancelling a process tree.
const KILL_GRACE: Duration = Duration::from_millis(200);

/// Which stream a log line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Stdout,
    Stderr,
}

/// Events emitted by the pool for the daemon/UI to consume.
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// One line of agent output.
    Log {
        task_id: String,
        stream: StreamKind,
        line: String,
        /// 0-based index of this line within the current run.
        ///
        /// Lets consumers merge streamed lines with the persisted
        /// `worker_logs` (and with snapshots taken at an arbitrary moment)
        /// idempotently: same index, same line, no duplicates, gaps visible.
        seq: usize,
    },
    /// A task changed status as a result of worker activity.
    StatusChanged {
        task_id: String,
        from: TaskStatus,
        to: TaskStatus,
    },
    /// A worker finished (successfully or not).
    TaskFinished { task_id: String, success: bool },
}

/// The agent CLI to run for a task.
///
/// `args` may contain the placeholders `{prompt}` (the task title +
/// description) and `{workspace}` (the workspace path). If neither appears,
/// the prompt is appended as a final argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl Default for AgentCommand {
    /// The agent command, resolved in this order:
    ///
    /// 1. `ZOU_AGENT_CMD`, parsed as a command line honoring shell-style
    ///    quoting (e.g. `ZOU_AGENT_CMD="opencode run --auto '{prompt}'"`).
    /// 2. The provider/model saved by `zou agent` / `zou model`:
    ///    * OpenCode → `opencode run --auto --format json --model <id> {prompt}`
    ///    * Antigravity → `agy -p {prompt} --output-format stream-json …`
    /// 3. The installed `opencode` CLI as
    ///    `opencode run --format json {prompt}`.
    ///
    /// An explicit environment override always wins. A malformed environment
    /// command or saved config is reported and skipped rather than silently
    /// running the wrong thing.
    fn default() -> Self {
        if let Ok(spec) = std::env::var("ZOU_AGENT_CMD") {
            match AgentCommand::from_spec(&spec) {
                Ok(cmd) => return cmd,
                Err(e) => {
                    // Don't silently run the wrong thing on a typo.
                    eprintln!("warning: ignoring ZOU_AGENT_CMD ({e}); checking saved config");
                }
            }
        }

        match Config::load() {
            Ok(config) => {
                if let Some(cmd) = AgentCommand::from_config(&config) {
                    return cmd;
                }
            }
            Err(e) => {
                eprintln!("warning: ignoring saved agent config ({e}); using default agent");
            }
        }

        Self::builtin()
    }
}

impl AgentCommand {
    /// The built-in agent invocation (`opencode run --format json {prompt}`).
    ///
    /// Verified against the installed CLI: `opencode run [flags] [<message...>]`
    /// takes the prompt as a trailing argument. The earlier
    /// `opencode --task <prompt>` form was rejected by the real binary.
    ///
    /// `--format json` keeps tool calls and the final assistant message
    /// distinguishable, so the worker can store a clean Markdown response in
    /// `agent_output` without sacrificing the technical transcript.
    fn builtin() -> Self {
        AgentCommand {
            program: "opencode".to_string(),
            args: vec![
                "run".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "{prompt}".to_string(),
            ],
        }
    }

    /// Build the configured agent invocation.
    ///
    /// OpenCode uses `--auto` because Zunesha runs headlessly with stdin
    /// closed; an interactive permission prompt would hang the task. Its JSON
    /// format lets the worker separate the final Markdown response from tool
    /// activity.
    ///
    /// Antigravity's headless mode needs `--dangerously-skip-permissions` for
    /// the same reason: without it, shell commands are soft-denied. Zunesha
    /// still runs the agent in an isolated workspace and keeps the human merge
    /// gate; use `ZOU_AGENT_CMD` if you need a narrower permission policy.
    pub fn from_config(config: &Config) -> Option<Self> {
        match config.provider() {
            AgentProvider::Opencode => {
                let model = config.model()?;
                Some(AgentCommand {
                    program: "opencode".to_string(),
                    args: vec![
                        "run".to_string(),
                        "--auto".to_string(),
                        "--format".to_string(),
                        "json".to_string(),
                        "--model".to_string(),
                        model.to_string(),
                        "{prompt}".to_string(),
                    ],
                })
            }
            AgentProvider::Antigravity => Some(Self::antigravity(config.model())),
        }
    }

    /// The built-in Antigravity headless invocation.
    ///
    /// `stream-json` is the only format that exposes both tool activity and
    /// the final response, so the Output and Logs tabs stay separate. The
    /// print timeout mirrors the worker's default 30-minute agent budget; the
    /// worker's own timeout is the backstop if `agy` ignores it.
    fn antigravity(model: Option<&str>) -> Self {
        let mut args = vec![
            "-p".to_string(),
            "{prompt}".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--print-timeout".to_string(),
            "30m".to_string(),
        ];
        if let Some(model) = model {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        AgentCommand {
            program: "agy".to_string(),
            args,
        }
    }

    /// Parse a full command line (program + args) from a string.
    ///
    /// Honors single quotes, double quotes, and backslash escapes so callers
    /// can pass arguments containing spaces. Errors on unbalanced quoting.
    pub fn from_spec(spec: &str) -> Result<Self> {
        let argv = split_command_line(spec)?;
        match argv.split_first() {
            Some((program, args)) if !program.is_empty() => Ok(AgentCommand {
                program: program.clone(),
                args: args.to_vec(),
            }),
            _ => Err(Error::Workspace("empty agent command".into())),
        }
    }

    /// A shell command, for tests and ad-hoc agents.
    pub fn shell(script: &str) -> Self {
        AgentCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    /// Substitute placeholders to produce the final `(program, args)`.
    ///
    /// The prompt is appended as a final argument only when neither the
    /// program name nor any argument actually referenced `{prompt}`.
    fn render(&self, prompt: &str, workspace: &str) -> (String, Vec<String>) {
        let wants_prompt = (self.program.contains("{prompt}")
            || self.args.iter().any(|a| a.contains("{prompt}")))
        .then_some(());
        let subst = |s: &str| {
            s.replace("{prompt}", prompt)
                .replace("{workspace}", workspace)
        };

        let program = subst(&self.program);
        let mut args = self.args.iter().map(|a| subst(a)).collect::<Vec<_>>();

        if wants_prompt.is_none() && !prompt.is_empty() {
            args.push(prompt.to_string());
        }
        (program, args)
    }
}

/// Split a command line into argv, POSIX-shell-style.
///
/// * single quotes: everything literal until the closing quote
/// * double quotes: backslash escapes only for `"` and `\`
/// * backslash outside quotes escapes the next character
/// * whitespace separates arguments; adjacent pieces concatenate
///   (`a"b c"` → `ab c`)
/// * empty quotes yield empty arguments (`''` → `""`)
pub(crate) fn split_command_line(spec: &str) -> Result<Vec<String>> {
    let mut argv: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = spec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\')) => current.push(esc),
                            Some(esc) => {
                                // POSIX: inside double quotes, backslash is
                                // literal unless it precedes `"` or `\`.
                                current.push('\\');
                                current.push(esc);
                            }
                            None => {
                                return Err(Error::Workspace(
                                    "unterminated double quote in agent command".into(),
                                ))
                            }
                        },
                        Some(other) => current.push(other),
                        None => {
                            return Err(Error::Workspace(
                                "unterminated double quote in agent command".into(),
                            ))
                        }
                    }
                }
            }
            '\'' => {
                in_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(other) => current.push(other),
                        None => {
                            return Err(Error::Workspace(
                                "unterminated single quote in agent command".into(),
                            ))
                        }
                    }
                }
            }
            '\\' => {
                in_token = true;
                match chars.next() {
                    Some(esc) => current.push(esc),
                    None => {
                        return Err(Error::Workspace(
                            "trailing backslash in agent command".into(),
                        ))
                    }
                }
            }
            c if c.is_whitespace() => {
                if in_token || !current.is_empty() {
                    argv.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            c => {
                in_token = true;
                current.push(c);
            }
        }
    }

    if in_token || !current.is_empty() {
        argv.push(current);
    }
    Ok(argv)
}

/// Which mode a worker runs a task in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunMode {
    /// Normal Phase 4/8 run: create the workspace, run the agent, validate.
    Normal,
    /// Phase 9 conflict resolution: the workspace already exists (rebased,
    /// conflict markers in place) and the agent reconciles them.
    ResolveConflicts,
}

impl RunMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::ResolveConflicts => "resolve_conflicts",
        }
    }
}

/// How one agent attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentOutcome {
    /// The process exited; the bool is whether the exit status was success.
    Exited(bool),
    /// The per-attempt wall-clock limit elapsed and the tree was killed.
    TimedOut,
    /// A human requested cancellation and the tree was killed.
    Cancelled,
}

enum WaitOutcome {
    Exited(std::process::ExitStatus),
    Cancelled,
    TimedOut,
}

/// Read `ZOU_AGENT_TIMEOUT_SECS`, defaulting to 30 minutes.
fn agent_timeout_from_env() -> Duration {
    let secs = std::env::var("ZOU_AGENT_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_AGENT_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// A bounded pool of concurrently running coding agents.
pub struct WorkerPool {
    max_concurrency: usize,
    semaphore: Arc<Semaphore>,
    db_path: PathBuf,
    paths: ZuneshaPaths,
    agent: AgentCommand,
    eval: EvalCommand,
    eval_timeout: Duration,
    agent_timeout: Duration,
    sandbox_mode: SandboxMode,
    controls: RunRegistry,
    events: broadcast::Sender<WorkerEvent>,
}

impl WorkerPool {
    /// Build a pool bound to an initialized installation.
    ///
    /// `max_concurrency` caps how many agents run at once (defaults to 3 when
    /// given 0). The validation command is read from `ZOU_EVAL_CMD` (with
    /// auto-detection as the fallback), and its timeout from
    /// `ZOU_EVAL_TIMEOUT_SECS`; use [`Self::with_eval`] /
    /// [`Self::with_eval_timeout`] to override programmatically.
    pub fn new(paths: ZuneshaPaths, max_concurrency: usize, agent: AgentCommand) -> Self {
        let max_concurrency = if max_concurrency == 0 {
            DEFAULT_MAX_CONCURRENCY
        } else {
            max_concurrency
        };
        let (events, _) = broadcast::channel(1024);
        Self {
            db_path: paths.state_db(),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_concurrency,
            paths,
            agent,
            eval: EvalCommand::from_env(),
            eval_timeout: eval::timeout_from_env(),
            agent_timeout: agent_timeout_from_env(),
            sandbox_mode: SandboxMode::from_env(),
            controls: RunRegistry::default(),
            events,
        }
    }

    /// Override the validation command (tests, programmatic config).
    pub fn with_eval(mut self, eval: EvalCommand) -> Self {
        self.eval = eval;
        self
    }

    /// Override the per-evaluation wall-clock limit.
    pub fn with_eval_timeout(mut self, timeout: Duration) -> Self {
        self.eval_timeout = timeout;
        self
    }

    /// Override the per-agent-attempt wall-clock limit.
    pub fn with_agent_timeout(mut self, timeout: Duration) -> Self {
        self.agent_timeout = timeout;
        self
    }

    /// Override the containment mode (tests, programmatic config).
    pub fn with_sandbox_mode(mut self, mode: SandboxMode) -> Self {
        self.sandbox_mode = mode;
        self
    }

    /// The cap on simultaneous agents.
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
    }

    /// The configured per-agent-attempt timeout.
    pub fn agent_timeout(&self) -> Duration {
        self.agent_timeout
    }

    /// Cancel a task owned by a live worker in this process.
    ///
    /// Returns `false` when the task is not running here. A task owned by a
    /// different process (for example a separate `zou run`) cannot be
    /// cancelled through this pool; use `zou recover` for a dead claim.
    pub fn cancel(&self, task_id: &str) -> bool {
        self.controls.cancel(task_id)
    }

    /// Ids currently owned by live workers in this process.
    pub fn active_tasks(&self) -> Vec<String> {
        self.controls.active()
    }

    /// Subscribe to this pool's events (logs, status changes, completions).
    ///
    /// Each caller gets its own independently-buffered receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkerEvent> {
        self.events.subscribe()
    }

    /// Open a fresh database handle (each worker gets its own connection).
    fn open_db(&self) -> Result<Database> {
        Database::open(&self.db_path)
    }

    /// Claim every `TODO` task and run it under the concurrency cap.
    ///
    /// Returns the ids of the tasks that were claimed and started, in
    /// creation order. Readiness is checked by the DAG engine
    /// ([`crate::dag::promote_ready`]), not here — by the time a task is
    /// `TODO` all its prerequisites are done.
    pub async fn run_ready(&self) -> Result<Vec<String>> {
        let ready = {
            let db = self.open_db()?;
            db.list_tasks_by_status(TaskStatus::Todo)?
        };

        let mut started = Vec::new();
        for task in ready {
            self.spawn_task(task.id.clone()).await?;
            started.push(task.id);
        }
        Ok(started)
    }

    /// Release the tasks a dead worker left claimed and re-run them.
    ///
    /// [`crate::dag::recover_stuck`] releases every `IN_PROGRESS`,
    /// `EVALUATING`, and `RESOLVING_CONFLICTS` task back to `TODO`; this
    /// re-spawns *exactly those* tasks — never other `TODO` work nobody has
    /// started yet — so a restarted daemon picks an interrupted run up where
    /// it stopped. The workspace is kept, so a released agent continues from
    /// the partial work instead of starting over.
    ///
    /// Returns the ids of the tasks that were released and re-started, in
    /// creation order.
    ///
    /// Only the human knows whether a claim belongs to a worker that is
    /// actually dead (see [`crate::dag::recover_stuck`]); at daemon boot the
    /// previous process is gone by construction, which is what makes this
    /// resumption safe there.
    pub async fn resume_stuck(&self) -> Result<Vec<String>> {
        let released = {
            let db = self.open_db()?;
            crate::dag::recover_stuck(&db, None)?
        };

        let mut resumed = Vec::with_capacity(released.len());
        for id in released {
            self.spawn_task(id.clone()).await?;
            resumed.push(id);
        }
        Ok(resumed)
    }

    /// Claim one task and hand it to a worker, respecting the concurrency cap.
    ///
    /// The returned handle resolves when that specific agent exits.
    pub async fn spawn_task(
        &self,
        task_id: String,
    ) -> Result<tokio::task::JoinHandle<Result<bool>>> {
        // Claim the task up-front (outside the semaphore) so the UI flips to
        // IN_PROGRESS immediately even while the worker waits for a slot.
        let task = {
            let db = self.open_db()?;
            let task = db.require_task(&task_id)?;
            if task.status != TaskStatus::Todo {
                return Err(Error::Workspace(format!(
                    "task {task_id} is {} (only TODO tasks can start)",
                    task.status
                )));
            }
            db.set_status(&task_id, TaskStatus::InProgress)?;
            // Fresh run: clear the previous attempt's output so a snapshot
            // taken before the first flush (or a run that never produces
            // output) can't show stale logs/output/diff against the new attempt.
            db.set_worker_logs(&task_id, "")?;
            db.set_agent_output(&task_id, "")?;
            db.set_diff(&task_id, "")?;
            task
        };
        self.emit(WorkerEvent::StatusChanged {
            task_id: task_id.clone(),
            from: TaskStatus::Todo,
            to: TaskStatus::InProgress,
        });

        let control = self.controls.register(&task_id);

        // Semaphore permit is acquired *inside* the worker so this method
        // returns immediately; the task simply waits its turn.
        Ok(self.launch(task, RunMode::Normal, control))
    }

    /// Resolve a merge conflict for an already-prepared task (Phase 9).
    ///
    /// The task must be `RESOLVING_CONFLICTS` — [`crate::approve_task`] moved
    /// it there and rebased its workspace onto the repository's current
    /// content with conflict markers. Unlike [`Self::spawn_task`] this neither
    /// claims the task (its status is already worker-owned) nor creates a
    /// workspace: rebuilding one would discard exactly the markers the agent
    /// is here to resolve.
    ///
    /// The returned handle resolves when the resolution attempt — including
    /// its [`crate::eval`] loop — finishes: green validation lands the task
    /// back in `REVIEW` for a second human approval, an unresolvable conflict
    /// parks it in `FAILED_NEEDS_HUMAN`.
    pub async fn spawn_resolution(
        &self,
        task_id: String,
    ) -> Result<tokio::task::JoinHandle<Result<bool>>> {
        let task = {
            let db = self.open_db()?;
            let task = db.require_task(&task_id)?;
            if task.status != TaskStatus::ResolvingConflicts {
                return Err(Error::Workspace(format!(
                    "task {task_id} is {} (only RESOLVING_CONFLICTS tasks can auto-resolve)",
                    task.status
                )));
            }
            // Fresh attempt: drop the pre-conflict logs/output/diff so the
            // reviewer sees only this run (the diff is regenerated on success,
            // now against the rebased base).
            db.set_worker_logs(&task_id, "")?;
            db.set_agent_output(&task_id, "")?;
            db.set_diff(&task_id, "")?;
            task
        };
        let control = self.controls.register(&task_id);
        Ok(self.launch(task, RunMode::ResolveConflicts, control))
    }

    /// Wrap a claimed task in the per-worker state and spawn it under the
    /// concurrency cap.
    fn launch(
        &self,
        task: Task,
        mode: RunMode,
        control: RunControl,
    ) -> tokio::task::JoinHandle<Result<bool>> {
        let pool = Arc::new(WorkerHandle {
            db_path: self.db_path.clone(),
            paths: self.paths.clone(),
            agent: self.agent.clone(),
            eval: self.eval.clone(),
            eval_timeout: self.eval_timeout,
            agent_timeout: self.agent_timeout,
            sandbox_mode: self.sandbox_mode,
            events: self.events.clone(),
        });
        let semaphore = self.semaphore.clone();
        let controls = self.controls.clone();
        let task_id = task.id.clone();

        tokio::spawn(async move {
            let result = async {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|_| Error::Workspace("worker pool closed".into()))?;
                run_one(&pool, task, mode, control).await
            }
            .await;
            controls.remove(&task_id);
            result
        })
    }

    fn emit(&self, event: WorkerEvent) {
        // A send fails only when nobody is listening, which is fine.
        let _ = self.events.send(event);
    }
}

/// The pieces a spawned worker needs, cheaply cloneable via `Arc`.
struct WorkerHandle {
    db_path: PathBuf,
    paths: ZuneshaPaths,
    agent: AgentCommand,
    eval: EvalCommand,
    eval_timeout: Duration,
    agent_timeout: Duration,
    sandbox_mode: SandboxMode,
    events: broadcast::Sender<WorkerEvent>,
}

impl WorkerHandle {
    fn open_db(&self) -> Result<Database> {
        Database::open(&self.db_path)
    }

    fn emit(&self, event: WorkerEvent) {
        let _ = self.events.send(event);
    }
}

/// A durable, append-only record of one worker run.
///
/// The journal is best-effort: a failure to write an audit event must never
/// take down the agent. The run row itself is created before work starts so
/// even a workspace setup failure leaves a visible record.
struct RunJournal {
    db_path: PathBuf,
    run_id: String,
    task_id: String,
}

impl RunJournal {
    fn start(pool: &WorkerHandle, task: &Task, mode: RunMode) -> Result<Self> {
        let run_id = new_run_id(&task.id);
        let db = pool.open_db()?;
        db.start_run(&RunRecord {
            id: run_id.clone(),
            task_id: task.id.clone(),
            mode: mode.as_str().to_string(),
            state: "RUNNING".to_string(),
            started_at: String::new(),
            ended_at: None,
            error_class: None,
            detail: String::new(),
        })?;
        let journal = Self {
            db_path: pool.db_path.clone(),
            run_id,
            task_id: task.id.clone(),
        };
        journal.event(
            "run_started",
            serde_json::json!({
                "mode": mode.as_str(),
                "retry_count": task.retry_count,
                "agent_timeout_secs": pool.agent_timeout.as_secs(),
                "sandbox": pool.sandbox_mode.as_str(),
            }),
        );
        Ok(journal)
    }

    fn event(&self, kind: &str, payload: serde_json::Value) {
        if let Ok(db) = Database::open(&self.db_path) {
            let _ = db.append_run_event(&self.run_id, &self.task_id, kind, &payload.to_string());
        }
    }

    fn finish(&self, state: &str, error_class: Option<&str>, detail: &str, success: bool) {
        if let Ok(db) = Database::open(&self.db_path) {
            let _ = db.finish_run(&self.run_id, state, error_class, detail);
        }
        self.event(
            "run_finished",
            serde_json::json!({ "state": state, "success": success }),
        );
    }
}

fn new_run_id(task_id: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{task_id}-{nanos}")
}

/// Append a human-readable note to a task description without losing the
/// existing text. Used for cancellation and timeout explanations so the next
/// attempt (and the reviewer) sees why the run stopped.
fn append_run_note(description: &str, heading: &str, detail: &str) -> String {
    let mut updated = description.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str("## ");
    updated.push_str(heading);
    updated.push('\n');
    updated.push_str(detail.trim());
    updated
}

impl AgentOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Exited(true) => "success",
            Self::Exited(false) => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Run one task through the whole worker pipeline: workspace, agent attempt,
/// validation, and (on failure) agent retries with the error output appended.
/// Returns `Ok(true)` when the task lands in `REVIEW`.
///
/// The phase sequence, until one of the exits is reached:
///
/// ```text
/// agent ──exit≠0──▶ TODO            (human-guided retry, as before)
/// agent ──exit=0──▶ EVALUATING ──tests pass──▶ REVIEW (+ diff)
///                       │
///                       └─tests fail─▶ retry_count++ ─▶ IN_PROGRESS (loop)
///                                        └─≥3 failures─▶ FAILED_NEEDS_HUMAN
/// ```
///
/// In [`RunMode::ResolveConflicts`] (Phase 9) the workspace is *not* created
/// (it exists, holding the conflict markers), the prompt is the conflict
/// prompt, and both failure exits become `FAILED_NEEDS_HUMAN` /
/// `RESOLVING_CONFLICTS` instead of `TODO` / `IN_PROGRESS`.
/// In both modes the workspace's Phase 10 `CODEBASE_MAP.md` is appended last
/// to the prompt, and the Phase 12 `PRIOR_AGENT_CONTEXT.md` rides just before
/// it; both are read once during setup.
async fn run_one(
    pool: &Arc<WorkerHandle>,
    task: Task,
    mode: RunMode,
    control: RunControl,
) -> Result<bool> {
    let task_id = task.id.clone();
    let journal = RunJournal::start(pool, &task, mode)?;

    // Workspace setup does blocking filesystem + git work; keep it off the
    // async runtime. The Phase 10 / 12 reference documents are read here too —
    // once per run, identical for every prompt of that run.
    let setup = {
        let pool = pool.clone();
        let task_id = task_id.clone();
        tokio::task::spawn_blocking(
            move || -> Result<(String, Option<String>, Option<String>)> {
                let db = pool.open_db()?;
                let mgr = WorkspaceManager::new(pool.paths.clone());
                let ws_dir = match mode {
                    RunMode::Normal => mgr.create(&db, &task_id)?.path,
                    RunMode::ResolveConflicts => {
                        // The workspace (and its conflict markers / rebased base)
                        // must be used exactly as `approve_task` left it.
                        let record = db.get_workspace(&task_id)?.ok_or_else(|| {
                            Error::Workspace(format!(
                                "no workspace to resolve conflicts for task {task_id}"
                            ))
                        })?;
                        if !Path::new(&record.path).is_dir() {
                            return Err(Error::Workspace(format!(
                                "workspace missing for conflict resolution: {}",
                                record.path
                            )));
                        }
                        record.path
                    }
                };
                let map = crate::codebase_map::read(Path::new(&ws_dir));
                let prior = crate::prior_context::read(Path::new(&ws_dir));
                Ok((ws_dir, map, prior))
            },
        )
        .await
        .map_err(|e| Error::Workspace(format!("workspace setup panicked: {e}")))
    };
    let (ws_dir, codebase_map, prior_context) = match setup {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            journal.finish("ERROR", Some("workspace_setup"), &error.to_string(), false);
            release_after_setup_error(pool, &task_id, mode).await;
            return Err(error);
        }
        Err(error) => {
            journal.finish("ERROR", Some("workspace_setup"), &error.to_string(), false);
            release_after_setup_error(pool, &task_id, mode).await;
            return Err(error);
        }
    };

    // One log collector for the entire run: the agent attempts and every
    // evaluation share it, so `worker_logs` and the live stream stay one
    // continuous, densely-numbered sequence across phases.
    let (line_tx, line_rx) = mpsc::unbounded_channel::<(StreamKind, String)>();
    let collector = tokio::spawn(collect_logs(pool.clone(), task_id.clone(), line_rx));

    // Mutable run state: the description grows a `## Validation failure`
    // section on each failed evaluation (the next agent attempt reads it),
    // and failures accumulate across the run.
    let mut description = task.description.clone();
    let mut retry_count = task.retry_count;
    // Where the task stands right now: claimed to `IN_PROGRESS` by
    // `spawn_task`, or already `RESOLVING_CONFLICTS` (set by `approve_task`)
    // for a resolution run.
    let mut status = match mode {
        RunMode::Normal => TaskStatus::InProgress,
        RunMode::ResolveConflicts => TaskStatus::ResolvingConflicts,
    };

    let drive = async {
        loop {
            // --- Agent attempt --------------------------------------------
            let mut prompt = match mode {
                RunMode::Normal => render_prompt(&task.title, &description),
                RunMode::ResolveConflicts => render_conflict_prompt(&task.title, &description),
            };
            // Phase 12: prior work from the source checkout, then Phase 10's
            // static repository map last — reference material, identical for
            // every attempt of this run.
            if let Some(prior) = &prior_context {
                prompt.push_str("\n\n");
                prompt.push_str(prior);
            }
            if let Some(map) = &codebase_map {
                prompt.push_str("\n\n");
                prompt.push_str(map);
            }
            journal.event(
                "agent_started",
                serde_json::json!({
                    "attempt": retry_count + 1,
                    "timeout_secs": pool.agent_timeout.as_secs(),
                }),
            );
            let outcome = run_agent_once(pool, &ws_dir, &prompt, &line_tx, &control).await?;
            journal.event(
                "agent_finished",
                serde_json::json!({ "outcome": outcome.label() }),
            );
            match outcome {
                AgentOutcome::Exited(true) => {}
                AgentOutcome::Exited(false) => {
                    // Normal run: the agent failed; a human decides the next
                    // attempt. Conflict run: the overlap could not be
                    // reconciled, and there is no from-scratch re-plan — park
                    // for a human.
                    return Ok::<(TaskStatus, bool), Error>(match mode {
                        RunMode::Normal => (TaskStatus::Todo, false),
                        RunMode::ResolveConflicts => (TaskStatus::FailedNeedsHuman, false),
                    });
                }
                AgentOutcome::TimedOut | AgentOutcome::Cancelled => {
                    let (heading, detail, to) = match (outcome, mode) {
                        (AgentOutcome::TimedOut, RunMode::Normal) => (
                            "Agent timeout",
                            format!(
                                "The agent exceeded the {}s wall-clock limit and was stopped. \
                                 Retry when you are ready.",
                                pool.agent_timeout.as_secs()
                            ),
                            TaskStatus::Todo,
                        ),
                        (AgentOutcome::TimedOut, RunMode::ResolveConflicts) => (
                            "Agent timeout",
                            format!(
                                "Conflict resolution exceeded the {}s wall-clock limit and was \
                                 stopped. Inspect the workspace and retry.",
                                pool.agent_timeout.as_secs()
                            ),
                            TaskStatus::FailedNeedsHuman,
                        ),
                        (AgentOutcome::Cancelled, RunMode::Normal) => (
                            "Run cancelled",
                            "The run was cancelled by a human. Retry when you are ready."
                                .to_string(),
                            TaskStatus::Todo,
                        ),
                        (AgentOutcome::Cancelled, RunMode::ResolveConflicts) => (
                            "Run cancelled",
                            "Conflict resolution was cancelled by a human. Inspect the workspace \
                             and retry."
                                .to_string(),
                            TaskStatus::FailedNeedsHuman,
                        ),
                        (AgentOutcome::Exited(_), _) => unreachable!("handled above"),
                    };
                    description = append_run_note(&description, heading, &detail);
                    {
                        let db = pool.open_db()?;
                        db.update_task(&task_id, None, Some(&description))?;
                    }
                    journal.event(
                        "run_interrupted",
                        serde_json::json!({
                            "reason": outcome.label(),
                            "status": to.as_str(),
                        }),
                    );
                    return Ok((to, false));
                }
            }

            // --- Validation (Phase 8 self-healing loop) -------------------
            let from = status;
            transition(pool, &task_id, &mut status, TaskStatus::Evaluating).await?;
            journal.event(
                "status_changed",
                serde_json::json!({ "from": from.as_str(), "to": status.as_str() }),
            );
            let Some(argv) = pool.eval.resolve(Path::new(&ws_dir)) else {
                // Nothing to validate in this project: exit-0 is as good as
                // a green test run, so go straight to human review.
                journal.event("validation_skipped", serde_json::json!({}));
                return Ok((TaskStatus::Review, true));
            };
            journal.event("validation_started", serde_json::json!({ "command": argv }));
            let outcome = eval::run_evaluation_with_control(
                &argv,
                Path::new(&ws_dir),
                pool.eval_timeout,
                &line_tx,
                Some(&control),
            )
            .await;
            journal.event(
                "validation_finished",
                serde_json::json!({
                    "success": outcome.success,
                    "cancelled": outcome.cancelled,
                }),
            );
            if outcome.cancelled {
                let (heading, detail, to) = match mode {
                    RunMode::Normal => (
                        "Run cancelled",
                        "The run was cancelled by a human during validation. Retry when you are \
                         ready."
                            .to_string(),
                        TaskStatus::Todo,
                    ),
                    RunMode::ResolveConflicts => (
                        "Run cancelled",
                        "Conflict resolution was cancelled by a human during validation. \
                         Inspect the workspace and retry."
                            .to_string(),
                        TaskStatus::FailedNeedsHuman,
                    ),
                };
                description = append_run_note(&description, heading, &detail);
                {
                    let db = pool.open_db()?;
                    db.update_task(&task_id, None, Some(&description))?;
                }
                journal.event(
                    "run_interrupted",
                    serde_json::json!({
                        "reason": "cancelled",
                        "phase": "validation",
                        "status": to.as_str(),
                    }),
                );
                return Ok((to, false));
            }
            if outcome.success {
                // Clean the failure section (if any) so the reviewer sees
                // the task as originally written.
                let clean = eval::strip_validation_section(&description);
                if clean != description {
                    let db = pool.open_db()?;
                    db.update_task(&task_id, None, Some(&clean))?;
                    description = clean;
                }
                return Ok((TaskStatus::Review, true));
            }

            // --- Failed validation: retry, or park for a human -------------
            retry_count += 1;
            {
                let db = pool.open_db()?;
                db.set_retry_count(&task_id, retry_count)?;
            }
            if retry_count >= MAX_EVAL_RETRIES {
                return Ok((TaskStatus::FailedNeedsHuman, false));
            }
            description = eval::build_fix_description(&description, &outcome.output);
            {
                let db = pool.open_db()?;
                db.update_task(&task_id, None, Some(&description))?;
            }
            let retry_to = match mode {
                RunMode::Normal => TaskStatus::InProgress,
                RunMode::ResolveConflicts => TaskStatus::ResolvingConflicts,
            };
            let from = status;
            transition(pool, &task_id, &mut status, retry_to).await?;
            journal.event(
                "status_changed",
                serde_json::json!({ "from": from.as_str(), "to": status.as_str() }),
            );
            // Loop: the agent re-runs in the same workspace with the errors.
        }
    }
    .await;

    // Stop the collector (closes the channel) and take its final flush.
    drop(line_tx);
    let _ = collector.await;

    match &drive {
        Ok((to, success)) => journal.finish(to.as_str(), None, "", *success),
        Err(error) => journal.finish("ERROR", Some("internal"), &error.to_string(), false),
    }
    let (to, success) = drive?;

    if success {
        // Capture the agent's changes as a unified diff for the review UI.
        // This runs `git add`/`git diff` (blocking), so keep it off the async
        // runtime. Best-effort: a failure here shouldn't abort the transition
        // — the diff would just show as empty in the dashboard.
        let pool = pool.clone();
        let diff_task_id = task_id.clone();
        let _ = tokio::task::spawn_blocking(move || -> Result<()> {
            let db = pool.open_db()?;
            let mgr = WorkspaceManager::new(pool.paths.clone());
            match mgr.generate_diff(&db, &diff_task_id) {
                Ok(diff) => db.set_diff(&diff_task_id, &diff)?,
                Err(e) => {
                    // Non-fatal: the dashboard shows an empty diff; note why.
                    eprintln!("warning: diff capture failed for {diff_task_id}: {e}");
                }
            }
            Ok(())
        })
        .await;
    }

    let from = status;
    transition(pool, &task_id, &mut status, to).await?;
    journal.event(
        "status_changed",
        serde_json::json!({ "from": from.as_str(), "to": status.as_str() }),
    );
    pool.emit(WorkerEvent::TaskFinished { task_id, success });

    Ok(success)
}

/// Release a task whose workspace setup failed after it was claimed.
///
/// Leaving it `IN_PROGRESS` would strand the task until a manual recovery, so
/// release a normal run to `TODO` and park a conflict-resolution run for a
/// human (its workspace state needs inspection). Best-effort: the journal row
/// already records the setup error even if this transition fails.
async fn release_after_setup_error(pool: &WorkerHandle, task_id: &str, mode: RunMode) {
    let to = match mode {
        RunMode::Normal => TaskStatus::Todo,
        RunMode::ResolveConflicts => TaskStatus::FailedNeedsHuman,
    };
    let Ok(db) = pool.open_db() else {
        return;
    };
    let Ok(Some(task)) = db.get_task(task_id) else {
        return;
    };
    if task.status == to || db.set_status(task_id, to).is_err() {
        return;
    }
    pool.emit(WorkerEvent::StatusChanged {
        task_id: task_id.to_string(),
        from: task.status,
        to,
    });
}

/// Move a task to `to`, persisting and broadcasting the transition.
///
/// `status` tracks where the task currently is so every event carries the
/// true `(from, to)` pair even through the retry loop's back-and-forth.
async fn transition(
    pool: &WorkerHandle,
    task_id: &str,
    status: &mut TaskStatus,
    to: TaskStatus,
) -> Result<()> {
    if *status == to {
        return Ok(());
    }
    let from = *status;
    {
        let db = pool.open_db()?;
        db.set_status(task_id, to)?;
    }
    pool.emit(WorkerEvent::StatusChanged {
        task_id: task_id.to_string(),
        from,
        to,
    });
    *status = to;
    Ok(())
}

/// One agent attempt: spawn the agent in the workspace, stream its output
/// into the run's shared log channel, and wait for it to exit, time out, or
/// be cancelled.
async fn run_agent_once(
    pool: &WorkerHandle,
    ws_dir: &str,
    prompt: &str,
    line_tx: &mpsc::UnboundedSender<(StreamKind, String)>,
    control: &RunControl,
) -> Result<AgentOutcome> {
    // A cancel can arrive while this task was waiting for a concurrency slot
    // or during workspace setup. Do not spawn an agent just to kill it.
    if control.is_cancelled() {
        let _ = line_tx.send((
            StreamKind::Stderr,
            "[zou] run cancelled by user before the agent started".to_string(),
        ));
        return Ok(AgentOutcome::Cancelled);
    }
    let policy = SandboxPolicy::new(ws_dir, pool.sandbox_mode)?;
    policy.validate()?;

    let (program, args) = pool.agent.render(prompt, ws_dir);
    let (program, args) = policy.wrap(&program, &args)?;

    // Spawn the agent with piped output, running inside the workspace. The
    // environment is pinned too: inheriting a parent OpenCode/Claude session
    // can make the CLI resolve the caller's project instead of this workspace.
    let mut command = Command::new(&program);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    crate::child_env::prepare(&mut command, Path::new(ws_dir));

    // Put the agent in its own process group so cancellation kills its
    // children too, not just the direct process. Windows uses `taskkill /T`
    // in `terminate_child` instead.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|e| Error::Workspace(format!("failed to spawn agent `{program}`: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Workspace("agent stdout missing".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Workspace("agent stderr missing".into()))?;

    // Fan both streams into the shared channel. These readers hold clones;
    // when both finish (after the agent exits and we await them below) the
    // phase is fully drained, but the channel itself stays open for later
    // phases — the run's collector only stops when `run_one` drops the
    // original sender.
    let out_task = tokio::spawn(read_lines(stdout, StreamKind::Stdout, line_tx.clone()));
    let err_task = tokio::spawn(read_lines(stderr, StreamKind::Stderr, line_tx.clone()));

    let wait = {
        let wait = child.wait();
        tokio::pin!(wait);
        tokio::select! {
            _ = control.cancelled() => WaitOutcome::Cancelled,
            _ = tokio::time::sleep(pool.agent_timeout) => WaitOutcome::TimedOut,
            status = &mut wait => WaitOutcome::Exited(
                status.map_err(|e| Error::Workspace(format!("failed to wait on agent: {e}")))?,
            ),
        }
    };

    let outcome = match wait {
        WaitOutcome::Exited(status) => AgentOutcome::Exited(status.success()),
        WaitOutcome::Cancelled => {
            terminate_child(&mut child).await;
            let _ = line_tx.send((
                StreamKind::Stderr,
                "[zou] run cancelled by user; stopping the agent process tree".to_string(),
            ));
            AgentOutcome::Cancelled
        }
        WaitOutcome::TimedOut => {
            terminate_child(&mut child).await;
            let _ = line_tx.send((
                StreamKind::Stderr,
                format!(
                    "[zou] agent timed out after {}s; stopping the process tree",
                    pool.agent_timeout.as_secs()
                ),
            ));
            AgentOutcome::TimedOut
        }
    };

    // Drain the streams fully before reporting the exit. A killed process can
    // leave a grandchild holding a pipe open, so cap the drain and abort the
    // readers rather than letting one orphan wedge the worker forever.
    let mut out_task = out_task;
    let mut err_task = err_task;
    let drain = async {
        let _ = (&mut out_task).await;
        let _ = (&mut err_task).await;
    };
    if tokio::time::timeout(Duration::from_secs(2), drain)
        .await
        .is_err()
    {
        out_task.abort();
        err_task.abort();
    }

    Ok(outcome)
}

/// Kill an agent and its descendants.
///
/// On Unix the child was started in its own process group (`process_group(0)`),
/// so signalling `-pid` reaches every process it spawned. On Windows,
/// `taskkill /T /F` walks the process tree.
async fn terminate_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        let group = format!("-{pid}");
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        tokio::time::sleep(KILL_GRACE).await;
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &group])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// The prompt handed to the agent: title, then description.
///
/// The description is passed freshly on every attempt — after a failed
/// validation it contains the `## Validation failure` section with the test
/// errors the agent must fix.
fn render_prompt(title: &str, description: &str) -> String {
    if description.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{description}")
    }
}

/// The fixed `## Merge conflict` section appended to every conflict-resolution
/// attempt (Phase 9).
const CONFLICT_INSTRUCTION: &str = "## Merge conflict\n\
     There are Git merge conflict markers in the code. Resolve them by keeping \
     the new logic but ensuring it integrates correctly with the updated base code.";

/// The prompt for a conflict-resolution attempt: the normal prompt plus the
/// strict resolution instruction.
///
/// Kept out of the stored description on purpose — it is the same fixed text
/// every attempt, so only the dynamic `## Validation failure` section (when a
/// re-validation fails) ever needs stripping back out for the reviewer.
fn render_conflict_prompt(title: &str, description: &str) -> String {
    format!(
        "{}\n\n{CONFLICT_INSTRUCTION}",
        render_prompt(title, description)
    )
}

/// Read a stream line-by-line, forwarding each line to the collector.
async fn read_lines<R>(
    reader: R,
    stream: StreamKind,
    tx: mpsc::UnboundedSender<(StreamKind, String)>,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send((stream, line)).is_err() {
            break; // collector gone
        }
    }
}

/// Turn OpenCode's `--format json` event stream into a readable transcript
/// while keeping the final assistant message separate.
///
/// The CLI emits one JSON object per line. `text` events are the assistant's
/// Markdown response; `tool_use` events describe tool activity. We keep both
/// in the transcript (so debugging still shows the full run) and remember the
/// last message's text as the clean `agent_output` for the Output tab.
///
/// Anything that is not a recognized OpenCode event — including every stderr
/// line, plain-text agents, and CLI warnings — passes through unchanged.
#[derive(Debug, Default)]
struct AgentStream {
    output: String,
    output_message: Option<String>,
    /// Antigravity streams response text in deltas; keep a fallback for runs
    /// that never emit a terminal `result` event.
    streamed: String,
}

impl AgentStream {
    fn observe(&mut self, stream: StreamKind, line: &str) -> Vec<String> {
        if stream == StreamKind::Stderr {
            return vec![line.to_string()];
        }
        if let Some(event) = parse_antigravity_event(line) {
            return self.observe_antigravity(&event);
        }
        let Some(event) = parse_opencode_event(line) else {
            return vec![line.to_string()];
        };
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                let text = event
                    .pointer("/part/text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if text.is_empty() {
                    return Vec::new();
                }
                let message = event
                    .pointer("/part/messageID")
                    .and_then(serde_json::Value::as_str);
                self.record_text(message, text);
                text.lines().map(str::to_string).collect()
            }
            Some("tool_use") => tool_log_lines(&event),
            Some("error") => vec![format!("[error] {}", error_message(&event))],
            Some("reasoning") => event
                .pointer("/part/text")
                .and_then(serde_json::Value::as_str)
                .map(|text| {
                    text.lines()
                        .map(|line| format!("[thinking] {line}"))
                        .collect()
                })
                .unwrap_or_default(),
            // Step boundaries are useful to the CLI's UI but add noise here.
            Some("step_start" | "step_finish") => Vec::new(),
            _ => vec![line.to_string()],
        }
    }

    /// Translate Antigravity's `--output-format stream-json` events.
    ///
    /// `agent_response` deltas become transcript lines; `tool` steps become
    /// `[tool]` lines with their parameters/output; the terminal `result`
    /// event supplies the clean Markdown response for the Output tab.
    fn observe_antigravity(&mut self, event: &serde_json::Value) -> Vec<String> {
        match event.get("event").and_then(serde_json::Value::as_str) {
            Some("init") => {
                let init = &event["init"];
                let mode = init
                    .get("permission_mode")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown");
                let model = init
                    .get("model")
                    .and_then(serde_json::Value::as_str)
                    .map(|model| format!(" model={model}"))
                    .unwrap_or_default();
                vec![format!("[agent] antigravity permission={mode}{model}")]
            }
            Some("step_update") => self.observe_antigravity_step(&event["step_update"]),
            Some("result") => {
                let result = &event["result"];
                let status = result
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("UNKNOWN");
                let mut lines = Vec::new();
                if status != "SUCCESS" {
                    let error = result
                        .get("error")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("no error detail");
                    lines.push(format!("[error] antigravity {status}: {error}"));
                }
                if let Some(response) = result
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .filter(|response| !response.trim().is_empty())
                {
                    self.output = response.trim().to_string();
                } else if self.output.trim().is_empty() {
                    self.output = self.streamed.trim().to_string();
                }
                lines
            }
            // Unknown event types are forward-compatible noise.
            _ => Vec::new(),
        }
    }

    fn observe_antigravity_step(&mut self, step: &serde_json::Value) -> Vec<String> {
        let step_type = step
            .get("step_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match step_type {
            "agent_response" => {
                let Some(delta) = step
                    .get("text_delta")
                    .and_then(serde_json::Value::as_str)
                    .filter(|delta| !delta.is_empty())
                else {
                    return Vec::new();
                };
                self.streamed.push_str(delta);
                delta.lines().map(str::to_string).collect()
            }
            "tool" => {
                let tool = step
                    .get("tool_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool");
                let mut header = format!("[tool] {tool}");
                if let Some(parameters) = step.pointer("/tool_info/parameters") {
                    if let Some(map) = parameters.as_object() {
                        for (key, value) in map {
                            header.push_str(&format!(" {key}={value}"));
                        }
                    } else if !parameters.is_null() {
                        header.push_str(&format!(" {parameters}"));
                    }
                }
                let mut lines = vec![header];
                if let Some(output) = step
                    .pointer("/tool_info/output")
                    .and_then(serde_json::Value::as_str)
                {
                    lines.extend(output.lines().map(|line| format!("  {line}")));
                }
                if let Some(error) = step
                    .pointer("/tool_info/error/message")
                    .and_then(serde_json::Value::as_str)
                {
                    lines.push(format!("  error: {error}"));
                }
                lines
            }
            "subagent" => {
                let name = step
                    .pointer("/subagent_info/type_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("subagent");
                vec![format!("[subagent] {name}")]
            }
            // `user_input`, `checkpoint`, and future step types are not
            // useful in the technical transcript.
            _ => Vec::new(),
        }
    }

    /// Keep the text of the most recent assistant message.
    ///
    /// OpenCode starts a new message for the final answer after any tool
    /// steps, so replacing the buffer on a new `messageID` drops intermediate
    /// narration but keeps all parts of the final Markdown response.
    fn record_text(&mut self, message: Option<&str>, text: &str) {
        let message = message.unwrap_or_default();
        if self.output_message.as_deref() != Some(message) {
            self.output.clear();
            self.output_message = Some(message.to_string());
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.output.push_str(text);
    }

    fn output(&self) -> &str {
        if self.output.trim().is_empty() {
            self.streamed.trim()
        } else {
            self.output.trim()
        }
    }
}

fn parse_antigravity_event(line: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = value.get("event")?.as_str()?;
    matches!(kind, "init" | "step_update" | "result").then_some(value)
}

fn parse_opencode_event(line: &str) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let kind = value.get("type")?.as_str()?;
    matches!(
        kind,
        "text" | "tool_use" | "step_start" | "step_finish" | "error" | "reasoning"
    )
    .then_some(value)
}

fn tool_log_lines(event: &serde_json::Value) -> Vec<String> {
    let part = &event["part"];
    let tool = part
        .get("tool")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("tool");
    let state = &part["state"];
    let status = state
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let title = state
        .get("title")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    let mut header = format!("[tool] {tool}");
    if !status.is_empty() && status != "completed" {
        header.push_str(&format!(" ({status})"));
    }
    if !title.is_empty() && title != tool {
        header.push_str(": ");
        header.push_str(title);
    }
    let mut lines = vec![header];
    if let Some(output) = state.get("output").and_then(serde_json::Value::as_str) {
        lines.extend(output.lines().map(|line| format!("  {line}")));
    }
    lines
}

fn error_message(event: &serde_json::Value) -> &str {
    event
        .pointer("/error/message")
        .or_else(|| event.pointer("/part/error/message"))
        .or_else(|| event.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("agent error")
}

/// Persist and broadcast agent output.
///
/// Accumulates lines and flushes to the database on a short timer so the UI
/// stays live without hammering SQLite on every line.
async fn collect_logs(
    pool: Arc<WorkerHandle>,
    task_id: String,
    mut rx: mpsc::UnboundedReceiver<(StreamKind, String)>,
) {
    let mut buffer = String::new();
    let mut stream = AgentStream::default();
    let mut ticker = tokio::time::interval(LOG_FLUSH_INTERVAL);
    // Skip the immediate first tick so we only flush when there is something
    // new.
    ticker.tick().await;
    let mut dirty = false;
    // Number of lines pushed into `buffer` so far == the index of the next
    // line. Mirrors what a consumer reconstructing `worker_logs.split('\n')`
    // would see, so streamed lines and persisted lines line up exactly.
    let mut seq: usize = 0;

    loop {
        tokio::select! {
            line = rx.recv() => {
                match line {
                    Some((kind, line)) => {
                        for line in stream.observe(kind, &line) {
                            let line = redact_secrets(&line);
                            pool.emit(WorkerEvent::Log {
                                task_id: task_id.clone(),
                                stream: kind,
                                line: line.clone(),
                                seq,
                            });
                            seq += 1;
                            buffer.push_str(&line);
                            buffer.push('\n');
                            dirty = true;
                        }
                    }
                    None => break, // both streams finished
                }
            }
            _ = ticker.tick(), if dirty => {
                flush_logs(&pool, &task_id, &buffer, stream.output()).await;
                dirty = false;
            }
        }
    }

    // Final flush of anything left over.
    if dirty || !stream.output().is_empty() {
        flush_logs(&pool, &task_id, &buffer, stream.output()).await;
    }
}

async fn flush_logs(pool: &WorkerHandle, task_id: &str, buffer: &str, output: &str) {
    let db_path = pool.db_path.clone();
    let task_id = task_id.to_string();
    let logs = buffer.to_string();
    let output = redact_secrets(output);
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(db) = Database::open(&db_path) {
            let _ = db.set_worker_logs(&task_id, &logs);
            let _ = db.set_agent_output(&task_id, &output);
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    /// Scratch git repo + initialized install, returning (repo, paths).
    fn scratch_install() -> (tempfile::TempDir, ZuneshaPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let st = std::process::Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg(tmp.path())
            .status()
            .unwrap();
        assert!(st.success());
        std::fs::write(tmp.path().join("app.rs"), "fn main() {}\n").unwrap();
        for args in [
            vec![
                "-C",
                tmp.path().to_str().unwrap(),
                "config",
                "user.email",
                "t@t.local",
            ],
            vec![
                "-C",
                tmp.path().to_str().unwrap(),
                "config",
                "user.name",
                "T",
            ],
            vec!["-C", tmp.path().to_str().unwrap(), "add", "--all"],
            vec![
                "-C",
                tmp.path().to_str().unwrap(),
                "commit",
                "--quiet",
                "-m",
                "baseline",
            ],
        ] {
            let st = std::process::Command::new("git")
                .args(&args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} should succeed");
        }

        let paths = ZuneshaPaths::new(tmp.path());
        std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
        // Real DB on disk: workers each open their own connection.
        Database::open(&paths.state_db()).unwrap();
        (tmp, paths)
    }

    /// Insert `n` tasks, all TODO (ready to run).
    fn ready_tasks(db: &Database, n: usize) -> Vec<String> {
        let mut ids = Vec::new();
        for i in 0..n {
            let id = format!("t{i}");
            db.insert_task(&Task::new(&id, format!("task {i}"), "desc"))
                .unwrap();
            db.set_status(&id, TaskStatus::Todo).unwrap();
            ids.push(id);
        }
        ids
    }

    fn status_of(db: &Database, id: &str) -> TaskStatus {
        db.get_task(id).unwrap().unwrap().status
    }

    #[test]
    fn split_command_line_honors_quoting() {
        // No quoting: simple whitespace split.
        assert_eq!(
            split_command_line("opencode --task go").unwrap(),
            vec!["opencode", "--task", "go"]
        );
        // Single quotes keep spaces in one argument.
        assert_eq!(
            split_command_line("sh -c 'echo agent-ran-ok'").unwrap(),
            vec!["sh", "-c", "echo agent-ran-ok"]
        );
        // Double quotes too.
        assert_eq!(
            split_command_line(r#"prog "arg with spaces""#).unwrap(),
            vec!["prog", "arg with spaces"]
        );
        // Adjacent pieces concatenate: a"b c" -> ab c
        assert_eq!(split_command_line(r#"a"b c"d"#).unwrap(), vec!["ab cd"]);
        // Empty quotes produce an empty argument.
        assert_eq!(split_command_line("prog ''").unwrap(), vec!["prog", ""]);
        // Backslash escapes outside quotes.
        assert_eq!(
            split_command_line(r"prog a\ b").unwrap(),
            vec!["prog", "a b"]
        );
        // Escaped quote inside double quotes is literal.
        assert_eq!(
            split_command_line(r#"prog "say \"hi\"""#).unwrap(),
            vec!["prog", r#"say "hi""#]
        );
        // Multiple runs of whitespace collapse.
        assert_eq!(
            split_command_line("  a   b\tc \n d ").unwrap(),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn split_command_line_rejects_unbalanced_quotes() {
        assert!(split_command_line("prog 'unclosed").is_err());
        assert!(split_command_line(r#"prog "unclosed"#).is_err());
        assert!(split_command_line("prog trailing\\").is_err());
        // Empty spec parses to empty argv (then from_spec errors).
        assert!(split_command_line("   ").unwrap().is_empty());
        assert!(AgentCommand::from_spec("   ").is_err());
    }

    #[test]
    fn opencode_events_split_markdown_output_from_tool_transcript() {
        let mut stream = AgentStream::default();

        // A tool step: no final output yet, but the transcript records it.
        let tool = serde_json::json!({
            "type": "tool_use",
            "part": {
                "messageID": "msg-1",
                "tool": "read",
                "state": {
                    "status": "completed",
                    "title": "read",
                    "output": "1: hello"
                }
            }
        })
        .to_string();
        let lines = stream.observe(StreamKind::Stdout, &tool);
        assert_eq!(lines, vec!["[tool] read", "  1: hello"]);
        assert_eq!(stream.output(), "");

        // The final answer arrives in a new message; it becomes the clean
        // Markdown output while also staying visible in the transcript.
        let text = serde_json::json!({
            "type": "text",
            "part": {
                "messageID": "msg-2",
                "text": "## Result\n\n- no files changed"
            }
        })
        .to_string();
        let lines = stream.observe(StreamKind::Stdout, &text);
        assert_eq!(lines, vec!["## Result", "", "- no files changed"]);
        assert_eq!(stream.output(), "## Result\n\n- no files changed");

        // Step boundaries are intentionally quiet.
        let step = serde_json::json!({"type": "step_start", "part": {}}).to_string();
        assert!(stream.observe(StreamKind::Stdout, &step).is_empty());
    }

    #[test]
    fn antigravity_events_split_response_from_tool_transcript() {
        let mut stream = AgentStream::default();

        let init = serde_json::json!({
            "event": "init",
            "init": {
                "permission_mode": "request-review",
                "model": "gemini-3.8-flash-medium"
            }
        })
        .to_string();
        assert_eq!(
            stream.observe(StreamKind::Stdout, &init),
            vec!["[agent] antigravity permission=request-review model=gemini-3.8-flash-medium"]
        );

        let tool = serde_json::json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "state": "DONE",
                "tool_name": "run_command",
                "tool_info": {
                    "parameters": {"CommandLine": "echo hi"},
                    "output": "hi\n"
                }
            }
        })
        .to_string();
        assert_eq!(
            stream.observe(StreamKind::Stdout, &tool),
            vec!["[tool] run_command CommandLine=\"echo hi\"", "  hi"]
        );

        // Deltas are visible live; the terminal result is the clean output.
        let active = serde_json::json!({
            "event": "step_update",
            "step_update": {
                "step_type": "agent_response",
                "state": "ACTIVE",
                "text_delta": "Hello "
            }
        })
        .to_string();
        assert_eq!(stream.observe(StreamKind::Stdout, &active), vec!["Hello "]);
        let done = serde_json::json!({
            "event": "step_update",
            "step_update": {
                "step_type": "agent_response",
                "state": "DONE",
                "text_delta": "world\n"
            }
        })
        .to_string();
        stream.observe(StreamKind::Stdout, &done);
        assert_eq!(stream.output(), "Hello world");

        let result = serde_json::json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "Hello world\n"
            }
        })
        .to_string();
        assert!(stream.observe(StreamKind::Stdout, &result).is_empty());
        assert_eq!(stream.output(), "Hello world");
    }

    #[test]
    fn antigravity_error_result_is_reported_in_the_transcript() {
        let mut stream = AgentStream::default();
        let result = serde_json::json!({
            "event": "result",
            "result": {"status": "ERROR", "response": "", "error": "authentication required"}
        })
        .to_string();
        assert_eq!(
            stream.observe(StreamKind::Stdout, &result),
            vec!["[error] antigravity ERROR: authentication required"]
        );
        assert_eq!(stream.output(), "");
    }

    #[test]
    fn only_the_last_opencode_text_message_is_the_output() {
        let mut stream = AgentStream::default();
        for (message, text) in [("msg-1", "intermediate"), ("msg-2", "final answer")] {
            let event = serde_json::json!({
                "type": "text",
                "part": {"messageID": message, "text": text}
            })
            .to_string();
            stream.observe(StreamKind::Stdout, &event);
        }
        assert_eq!(stream.output(), "final answer");
    }

    #[test]
    fn plain_output_and_stderr_pass_through_unchanged() {
        let mut stream = AgentStream::default();
        assert_eq!(
            stream.observe(StreamKind::Stdout, "plain agent line"),
            vec!["plain agent line"]
        );
        assert_eq!(
            stream.observe(StreamKind::Stderr, "warning: beep"),
            vec!["warning: beep"]
        );
        assert_eq!(stream.output(), "");
    }

    #[test]
    fn saved_model_builds_an_auto_opencode_command() {
        let config = Config {
            agent: crate::config::AgentConfig {
                provider: AgentProvider::Opencode,
                model: Some("opencode/deepseek-v4.1-flash-free".into()),
                models: Default::default(),
            },
        };
        let cmd = AgentCommand::from_config(&config).expect("saved model");
        assert_eq!(cmd.program, "opencode");
        assert_eq!(
            cmd.args,
            vec![
                "run",
                "--auto",
                "--format",
                "json",
                "--model",
                "opencode/deepseek-v4.1-flash-free",
                "{prompt}"
            ]
        );
        assert_eq!(
            AgentCommand::from_config(&Config::default()),
            None,
            "no saved model leaves the built-in command in charge"
        );
    }

    #[test]
    fn saved_antigravity_provider_builds_a_headless_agy_command() {
        let config = Config {
            agent: crate::config::AgentConfig {
                provider: AgentProvider::Antigravity,
                model: Some("gemini-3.8-flash-medium".into()),
                models: Default::default(),
            },
        };
        let cmd = AgentCommand::from_config(&config).expect("antigravity provider");
        assert_eq!(cmd.program, "agy");
        assert!(cmd.args.contains(&"-p".to_string()));
        assert!(cmd.args.contains(&"stream-json".to_string()));
        assert!(cmd
            .args
            .contains(&"--dangerously-skip-permissions".to_string()));
        assert!(cmd.args.contains(&"gemini-3.8-flash-medium".to_string()));

        let config = Config {
            agent: crate::config::AgentConfig {
                provider: AgentProvider::Antigravity,
                model: None,
                models: Default::default(),
            },
        };
        let cmd = AgentCommand::from_config(&config).expect("provider default");
        assert_eq!(cmd.program, "agy");
        assert!(!cmd.args.contains(&"--model".to_string()));
    }

    #[test]
    fn from_spec_builds_program_and_args() {
        let cmd = AgentCommand::from_spec("sh -c 'echo hi'").unwrap();
        assert_eq!(cmd.program, "sh");
        assert_eq!(cmd.args, vec!["-c", "echo hi"]);

        // The exact form the smoke test needs: one -c argument.
        let cmd = AgentCommand::from_spec("sh -c echo-ran").unwrap();
        assert_eq!(cmd.program, "sh");
        assert_eq!(cmd.args, vec!["-c", "echo-ran"]);
    }

    #[test]
    fn render_substitutes_placeholders_and_appends_prompt() {
        // {prompt} referenced => not appended again.
        let cmd = AgentCommand {
            program: "opencode".into(),
            args: vec!["run".into(), "{prompt}".into()],
        };
        let (prog, args) = cmd.render("do the thing", "/ws");
        assert_eq!(prog, "opencode");
        assert_eq!(args, vec!["run", "do the thing"]);

        // No {prompt} anywhere => prompt appended as final arg.
        let cmd = AgentCommand {
            program: "echo".into(),
            args: vec!["--flag".into(), "{workspace}".into()],
        };
        let (prog, args) = cmd.render("the prompt", "/ws");
        assert_eq!(prog, "echo");
        assert_eq!(args, vec!["--flag", "/ws", "the prompt"]);

        // Empty prompt => nothing appended.
        let cmd = AgentCommand {
            program: "echo".into(),
            args: vec!["x".into()],
        };
        let (_, args) = cmd.render("", "/ws");
        assert_eq!(args, vec!["x"]);
    }

    #[tokio::test]
    async fn successful_agent_moves_task_to_review_and_captures_logs() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo hello-from-agent; echo oops-on-stderr >&2"),
        );
        let mut rx = pool.subscribe();

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        let success = handle.await.unwrap().unwrap();
        assert!(success, "agent exit 0 => success");

        // Logs persisted to the DB.
        let db = Database::open(&paths.state_db()).unwrap();
        let logs = db.get_task("t0").unwrap().unwrap().worker_logs;
        assert!(
            logs.contains("hello-from-agent"),
            "stdout captured: {logs:?}"
        );
        assert!(logs.contains("oops-on-stderr"), "stderr captured: {logs:?}");

        // Status: TODO -> IN_PROGRESS -> REVIEW.
        assert_eq!(status_of(&db, "t0"), TaskStatus::Review);

        // Events were broadcast: two log lines + status changes + finished.
        let mut saw_log = false;
        let mut saw_finished = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                WorkerEvent::Log { .. } => saw_log = true,
                WorkerEvent::TaskFinished { success, .. } => saw_finished = success,
                _ => {}
            }
        }
        assert!(saw_log, "log lines broadcast");
        assert!(saw_finished, "completion broadcast");
    }

    #[tokio::test]
    async fn structured_agent_response_is_saved_when_no_files_change() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // The shape `opencode run --format json` produces: a text event in the
        // final assistant message. Nothing is written to the workspace.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell(
                r###"printf '%s\n' '{"type":"step_start","part":{"messageID":"msg-1"}}' '{"type":"text","part":{"messageID":"msg-2","text":"## Answer\n\n- no files changed"}}'"###,
            ),
        );

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        assert!(handle.await.unwrap().unwrap(), "agent exit 0 => success");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.get_task("t0").unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(
            task.agent_output, "## Answer\n\n- no files changed",
            "final Markdown response is stored separately from the transcript"
        );
        assert!(
            task.diff.is_empty(),
            "an analysis task changes no files, so the diff is empty"
        );
        assert!(
            task.worker_logs.contains("## Answer"),
            "the transcript still carries the text: {:?}",
            task.worker_logs
        );
        assert!(
            !task.worker_logs.contains(r#"{"type":"text""#),
            "raw JSON events must not leak into the transcript: {:?}",
            task.worker_logs
        );
    }

    #[tokio::test]
    async fn agent_prompts_carry_the_codebase_map() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // Phase 10: every prompt ends with the workspace's `CODEBASE_MAP.md`.
        // `sh -c <script> <prompt>` puts the appended prompt in `$0` (the
        // script never mentions `{prompt}`, so render hands it over as the
        // final argv element), and the agent reports what it saw.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell(
                "P=\"$0\"; \
                 echo \"map-in-prompt=$(printf '%s' \"$P\" | grep -c 'Codebase map')\"; \
                 echo \"tree-in-prompt=$(printf '%s' \"$P\" | grep -c 'app.rs (1 line)')\"; \
                 echo \"map-at-end=$(printf '%s' \"$P\" | tail -n 1 | grep -c 'directories')\"",
            ),
        );

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        assert!(handle.await.unwrap().unwrap(), "agent exit 0 => success");

        let db = Database::open(&paths.state_db()).unwrap();
        let logs = db.get_task("t0").unwrap().unwrap().worker_logs;
        assert!(logs.contains("map-in-prompt=1"), "map section: {logs:?}");
        assert!(logs.contains("tree-in-prompt=1"), "tree entry: {logs:?}");
        assert!(
            logs.contains("map-at-end=1"),
            "the map is reference material and rides last: {logs:?}"
        );
        assert_eq!(status_of(&db, "t0"), TaskStatus::Review);
    }

    #[tokio::test]
    async fn agents_run_with_workspace_pwd_and_without_parent_session_markers() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        std::env::set_var("OPENCODE_ZOU_WORKER_LEAK", "parent-session");

        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell(
                "echo \"pwd-ok=$(test \\\"$PWD\\\" = \\\"$(pwd)\\\" && echo yes || echo no)\"; \
                 echo \"leak=${OPENCODE_ZOU_WORKER_LEAK-unset}\"",
            ),
        );

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        assert!(handle.await.unwrap().unwrap(), "agent exit 0 => success");

        let db = Database::open(&paths.state_db()).unwrap();
        let logs = db.get_task("t0").unwrap().unwrap().worker_logs;
        assert!(logs.contains("pwd-ok=yes"), "PWD must match cwd: {logs:?}");
        assert!(
            logs.contains("leak=unset"),
            "parent session env leaked into the agent: {logs:?}"
        );

        std::env::remove_var("OPENCODE_ZOU_WORKER_LEAK");
    }

    #[tokio::test]
    async fn agent_prompts_carry_prior_agent_context() {
        let (repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // Work another agent left in the source checkout before this run.
        std::fs::write(
            repo.path().join("app.rs"),
            "fn main() { println!(\"prior agent\"); }\n",
        )
        .unwrap();
        std::fs::write(repo.path().join("CLAUDE.md"), "Use tabs, not spaces.\n").unwrap();
        std::fs::write(repo.path().join("plan.md"), "remaining steps\n").unwrap();

        // The agent writes a real change while reporting what its prompt saw.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell(
                "P=\"$0\"; \
                 echo \"prior-in-prompt=$(printf '%s' \"$P\" | grep -c 'Prior agent context')\"; \
                 echo \"instructions-in-prompt=$(printf '%s' \"$P\" | grep -c 'Use tabs, not spaces')\"; \
                 echo \"diff-in-prompt=$(printf '%s' \"$P\" | grep -c 'prior agent')\"; \
                 printf '%s' \"$P\" | grep -q -- '- plan.md (' && echo 'untracked-in-prompt=yes' || echo 'untracked-in-prompt=no'; \
                 printf '%s' \"$P\" | tail -n 1 | grep -q 'directories' && echo 'map-still-last=yes' || echo 'map-still-last=no'; \
                 printf 'fn feature() {}\n' > feature.rs",
            ),
        );

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        assert!(handle.await.unwrap().unwrap(), "agent exit 0 => success");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.get_task("t0").unwrap().unwrap();
        let logs = &task.worker_logs;
        for marker in [
            "prior-in-prompt=1",
            "instructions-in-prompt=1",
            "diff-in-prompt=1",
            "untracked-in-prompt=yes",
            "map-still-last=yes",
        ] {
            assert!(logs.contains(marker), "missing {marker}: {logs:?}");
        }
        assert_eq!(task.status, TaskStatus::Review);
        assert!(
            task.diff.contains("feature.rs"),
            "the task's own change is captured: {}",
            task.diff
        );
        assert!(
            !task.diff.contains("PRIOR_AGENT_CONTEXT.md"),
            "scaffolding never reaches the reviewed diff: {}",
            task.diff
        );
    }

    #[tokio::test]
    async fn log_events_carry_a_dense_zero_based_seq() {
        // The dashboard merges streamed lines with snapshot `worker_logs` by
        // index, so `seq` must be exactly 0..n-1 in emission order — a gap or
        // overlap would misplace or duplicate lines in the live log view.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("for i in 1 2 3 4 5; do echo line-$i; done"),
        );
        let mut rx = pool.subscribe();

        let handle = pool.spawn_task("t0".into()).await.unwrap();
        handle.await.unwrap().unwrap();

        let mut seqs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let WorkerEvent::Log { seq, line, .. } = ev {
                seqs.push((seq, line));
            }
        }
        // Dense and ordered: 0,1,2,3,4.
        let indices: Vec<usize> = seqs.iter().map(|(s, _)| *s).collect();
        assert_eq!(indices, vec![0, 1, 2, 3, 4], "seq is dense and ordered");
        // And each seq lines up with the persisted worker_logs split.
        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        let persisted: Vec<&str> = task.worker_logs.lines().collect();
        for (seq, line) in &seqs {
            assert_eq!(
                persisted.get(*seq).copied(),
                Some(line.as_str()),
                "seq {seq} must match persisted worker_logs line"
            );
        }
    }

    #[tokio::test]
    async fn rerun_clears_stale_logs_and_regenerates_diff() {
        // Two invariants for a retry (workspace is kept across attempts, by
        // design — a retry builds on the previous diff):
        //  1. Logs are per-run: attempt 1's output must NOT leak into attempt 2.
        //  2. The diff is regenerated from the live workspace on each success,
        //     so it reflects attempt 2's changes too — not a stale copy.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // Attempt 1: writes a file (diff captured) and prints a marker.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo attempt-one-marker; echo 'fn one() {}' >> app.rs"),
        );
        pool.spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let db = Database::open(&paths.state_db()).unwrap();
        let before = db.require_task("t0").unwrap();
        assert!(
            before.worker_logs.contains("attempt-one-marker"),
            "first attempt logged"
        );
        assert!(
            before.diff.contains("fn one()"),
            "first attempt produced a diff"
        );
        db.set_status("t0", TaskStatus::Todo).unwrap();

        // Attempt 2: builds on the workspace (adds another function), silent.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo 'fn two() {}' >> app.rs"),
        );
        pool.spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let db = Database::open(&paths.state_db()).unwrap();
        let after = db.require_task("t0").unwrap();
        assert_eq!(after.status, TaskStatus::Review);
        // 1. Logs are fresh: attempt 1's marker is gone.
        assert!(
            !after.worker_logs.contains("attempt-one-marker"),
            "stale logs from previous attempt must not leak: {:?}",
            after.worker_logs
        );
        // 2. Diff regenerated: it now includes attempt 2's change (and still
        //    the workspace's prior edit, since retries keep the workspace).
        assert!(
            after.diff.contains("fn two()"),
            "diff must reflect attempt 2's work, regenerated not stale: {:?}",
            after.diff
        );
        assert!(
            after.diff.contains("fn one()"),
            "workspace persists across retries, so its edits remain in the diff"
        );
    }

    #[tokio::test]
    async fn failing_agent_returns_task_to_todo() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("exit 1"));
        let handle = pool.spawn_task("t0".into()).await.unwrap();
        let success = handle.await.unwrap().unwrap();
        assert!(!success, "non-zero exit => not success");

        let db = Database::open(&paths.state_db()).unwrap();
        assert_eq!(
            status_of(&db, "t0"),
            TaskStatus::Todo,
            "retries are possible"
        );
    }

    #[tokio::test]
    async fn runs_agents_concurrently_up_to_cap_then_serializes_the_rest() {
        // 6 tasks, cap 3, each sleeps 150ms. If the cap were ignored they'd
        // all run in ~150ms; with cap 3 it takes >= 2 waves (~300ms).
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 6);

        let pool = WorkerPool::new(paths.clone(), 3, AgentCommand::shell("sleep 0.15"));
        let started = std::time::Instant::now();
        let handles = {
            let mut hs = Vec::new();
            for id in 0..6 {
                hs.push(pool.spawn_task(format!("t{id}")).await.unwrap());
            }
            hs
        };
        for h in handles {
            h.await.unwrap().unwrap();
        }
        let elapsed = started.elapsed();

        assert_eq!(pool.max_concurrency(), 3);
        // >= 2 waves of 3 => at least ~300ms; allow generous slack for slow CI.
        assert!(
            elapsed >= Duration::from_millis(250),
            "cap should force >= 2 waves, took {elapsed:?}"
        );

        let db = Database::open(&paths.state_db()).unwrap();
        for id in 0..6 {
            assert_eq!(
                status_of(&db, &format!("t{id}")),
                TaskStatus::Review,
                "all agents ran to completion"
            );
        }
    }

    #[tokio::test]
    async fn never_exceeds_the_concurrency_cap_in_practice() {
        // Track the number of simultaneously-running agents from inside the
        // agents themselves: each appends a marker, sleeps (making overlap
        // likely if the cap leaks), then removes it. The count of markers
        // present at any moment must never exceed the cap.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 5);

        let marker_dir = _repo.path().join("markers");
        std::fs::create_dir_all(&marker_dir).unwrap();
        // Keep the counts log OUTSIDE marker_dir: if it lived alongside the
        // markers, `ls | wc -l` would count it too and inflate the peak.
        let counts_file = _repo.path().join("peak_counts");
        let max = 2;

        // Each agent: add marker, record the in-flight count, sleep, remove.
        let script = format!(
            r#"
            m="{marker}/{pid}"
            touch "$m"
            cnt=$(ls -1 "{marker}" 2>/dev/null | wc -l)
            echo "$cnt" >> "{counts}"
            sleep 0.08
            rm -f "$m"
            exit 0
        "#,
            marker = marker_dir.display(),
            counts = counts_file.display(),
            pid = "$$"
        );

        let pool = WorkerPool::new(paths.clone(), max, AgentCommand::shell(&script));
        let handles = {
            let mut hs = Vec::new();
            for id in 0..5 {
                hs.push(pool.spawn_task(format!("t{id}")).await.unwrap());
            }
            hs
        };
        for h in handles {
            h.await.unwrap().unwrap();
        }

        // No leaked permits: every agent released its slot.
        assert_eq!(pool.max_concurrency(), max);
        assert_eq!(
            pool.semaphore.available_permits(),
            max,
            "all permits released"
        );

        // The observed in-flight count never exceeded the cap.
        let counts = std::fs::read_to_string(&counts_file).unwrap();
        let peak: usize = counts
            .split_whitespace()
            .filter_map(|n| n.parse::<usize>().ok())
            .max()
            .unwrap_or(0);
        assert!(
            peak <= max,
            "observed {peak} concurrent agents, cap is {max}"
        );
        assert!(peak >= 1, "agents should have run at all");
    }

    #[tokio::test]
    async fn non_todo_task_cannot_be_spawned() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        db.insert_task(&Task::new("t0", "backlog task", ""))
            .unwrap();
        // still BACKLOG

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("true"));
        let err = pool.spawn_task("t0".into()).await.unwrap_err();
        assert!(err.to_string().contains("only TODO tasks can start"));

        // Still BACKLOG — nothing was claimed.
        let db = Database::open(&paths.state_db()).unwrap();
        assert_eq!(status_of(&db, "t0"), TaskStatus::Backlog);
    }

    #[tokio::test]
    async fn run_ready_picks_up_every_todo_task() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 4);

        let pool = WorkerPool::new(paths.clone(), 4, AgentCommand::shell("true"));
        let started = pool.run_ready().await.unwrap();
        assert_eq!(started.len(), 4);
        assert!(started.iter().all(|id| id.starts_with("t")));
    }

    /// Poll until `id` reaches `want` (agents run detached after
    /// `resume_stuck`), or panic after ~5s.
    async fn await_status(db: &Database, id: &str, want: TaskStatus) {
        for _ in 0..200 {
            if status_of(db, id) == want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "task {id} never reached {want:?}, stuck at {:?}",
            status_of(db, id)
        );
    }

    #[tokio::test]
    async fn resume_stuck_re_runs_exactly_the_crashed_claims() {
        // A daemon killed mid-run leaves claims behind (Phase 11): a fresh
        // pool must release and re-run *those* tasks — and must not start
        // planned-but-never-run TODO work.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();

        for (id, status) in [
            ("midval", TaskStatus::Evaluating),
            ("crashed", TaskStatus::InProgress),
        ] {
            db.insert_task(&Task::new(id, id, "desc")).unwrap();
            db.set_status(id, TaskStatus::Todo).unwrap();
            db.force_set_status(id, status).unwrap();
        }
        // Never started by anyone: stays TODO through a resume.
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 2, AgentCommand::shell("true"));
        let resumed = pool.resume_stuck().await.unwrap();
        assert_eq!(resumed, vec!["crashed".to_string(), "midval".to_string()]);

        // The re-run agents complete the pipeline without any further nudge.
        await_status(&db, "crashed", TaskStatus::Review).await;
        await_status(&db, "midval", TaskStatus::Review).await;
        assert_eq!(status_of(&db, "t0"), TaskStatus::Todo);
    }

    #[tokio::test]
    async fn resume_stuck_continues_a_crashed_conflict_resolution() {
        // A resolution interrupted by a crash is re-queued like `zou recover`
        // does: a normal attempt in the *kept* workspace, so the markers and
        // the already-rebased base survive the restart.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();

        db.insert_task(&Task::new("conflicted", "conflicted", "desc"))
            .unwrap();
        db.set_status("conflicted", TaskStatus::Todo).unwrap();
        let ws = WorkspaceManager::new(paths.clone())
            .create(&db, "conflicted")
            .unwrap();
        std::fs::write(
            Path::new(&ws.path).join("app.rs"),
            "<<<<<<< agent\nfn main() {}\n",
        )
        .unwrap();
        db.force_set_status("conflicted", TaskStatus::ResolvingConflicts)
            .unwrap();

        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("printf 'resolved\n' > app.rs"),
        );
        let resumed = pool.resume_stuck().await.unwrap();
        assert_eq!(resumed, vec!["conflicted".to_string()]);

        await_status(&db, "conflicted", TaskStatus::Review).await;
        // The agent ran inside the same workspace (not a fresh clone).
        let content = std::fs::read_to_string(Path::new(&ws.path).join("app.rs")).unwrap();
        assert_eq!(content, "resolved\n");
    }

    #[tokio::test]
    async fn resume_stuck_with_nothing_crashed_is_a_no_op() {
        // Booting a daemon over a healthy plan must not touch it — in
        // particular it must not start TODO work nobody asked to run.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 2);

        let pool = WorkerPool::new(paths.clone(), 2, AgentCommand::shell("true"));
        let resumed = pool.resume_stuck().await.unwrap();
        assert!(resumed.is_empty());
        assert_eq!(status_of(&db, "t0"), TaskStatus::Todo);
        assert_eq!(status_of(&db, "t1"), TaskStatus::Todo);
    }

    #[tokio::test]
    async fn agent_runs_inside_the_isolated_workspace() {
        // The agent writes a file; it must land in the workspace, never in
        // the user's real repo.
        let (repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo touched > AGENT_WAS_HERE.txt"),
        );
        let h = pool.spawn_task("t0".into()).await.unwrap();
        h.await.unwrap().unwrap();

        let ws = paths.workspace("t0");
        assert!(
            ws.join("AGENT_WAS_HERE.txt").is_file(),
            "agent wrote into its workspace"
        );
        // Main repo untouched.
        assert!(!repo.path().join("AGENT_WAS_HERE.txt").exists());
    }

    #[tokio::test]
    async fn successful_run_captures_diff_for_review() {
        // The Phase 6 review modal renders `task.diff`; it must be populated
        // when an agent lands a task in REVIEW, not just by `zou diff`.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // Modify an existing tracked file so `git diff` has something real.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo 'fn added() {}' >> app.rs"),
        );
        let h = pool.spawn_task("t0".into()).await.unwrap();
        assert!(h.await.unwrap().unwrap(), "agent should succeed");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert!(
            task.diff.contains("fn added()"),
            "diff must show the agent's change: {:?}",
            task.diff
        );
        assert!(
            task.diff.starts_with("diff --git"),
            "diff should be a unified diff: {:?}",
            &task.diff[..task.diff.len().min(40)]
        );
    }

    #[tokio::test]
    async fn failed_run_leaves_diff_empty() {
        // Failure path: no REVIEW, and no diff from a run that changed nothing.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("exit 1"));
        let h = pool.spawn_task("t0".into()).await.unwrap();
        assert!(!h.await.unwrap().unwrap());

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Todo, "back to TODO for retry");
        assert!(task.diff.is_empty(), "no diff captured on failure");
    }

    // ------------------------------------------- Phase 8: the eval loop

    #[tokio::test]
    async fn green_evaluation_moves_task_through_evaluating_to_review() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("echo agent-ok"))
            .with_eval(EvalCommand::fixed("sh -c 'echo eval-output-ok'"));
        let mut rx = pool.subscribe();

        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(success);

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 0);
        // Both phases' output is in the persisted logs, eval prefixed.
        assert!(task.worker_logs.contains("agent-ok"));
        assert!(task.worker_logs.contains("[eval] eval-output-ok"));

        // The full transition chain was broadcast, in order.
        let mut transitions = Vec::new();
        let mut seqs = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            match ev {
                WorkerEvent::StatusChanged { from, to, .. } => transitions.push((from, to)),
                WorkerEvent::Log { seq, .. } => seqs.push(seq),
                _ => {}
            }
        }
        assert_eq!(
            transitions,
            vec![
                (TaskStatus::Todo, TaskStatus::InProgress),
                (TaskStatus::InProgress, TaskStatus::Evaluating),
                (TaskStatus::Evaluating, TaskStatus::Review),
            ],
            "the UI sees the EVALUATING stage"
        );
        // Log seq stays dense ACROSS phases: 1 agent line + 1 eval line.
        assert_eq!(seqs, vec![0, 1], "one continuous sequence for the run");
    }

    #[tokio::test]
    async fn failed_evaluation_retries_the_agent_with_the_errors_in_its_prompt() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        // The agent "fixes" the code on its second attempt (FIXED appears
        // only after ATTEMPTED exists, i.e. after one failed validation).
        // `$0` is the prompt (render appends it as a trailing argument).
        let agent = AgentCommand::shell(
            r#"if [ -f FIXED ]; then :; elif [ -f ATTEMPTED ]; then echo fixed > FIXED; else echo 1 > ATTEMPTED; fi
               echo "prompt-mentions-assertion=$(echo "$0" | grep -c 'ASSERTION: FIXED is missing')""#,
        );
        // Validation fails until FIXED exists in the workspace.
        let eval = EvalCommand::fixed(
            "sh -c 'test -f FIXED || { echo \"ASSERTION: FIXED is missing\"; exit 1; }'",
        );
        let pool = WorkerPool::new(paths.clone(), 1, agent).with_eval(eval);

        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(success, "the retry fixed the code");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 1, "one failed validation");
        // The retry attempt's prompt carried the failure (grep count == 1),
        // and the passing run cleaned the section back out of the task text.
        assert!(
            task.worker_logs.contains("prompt-mentions-assertion=1"),
            "second agent attempt saw the validation errors: {}",
            task.worker_logs
        );
        assert!(
            !task.description.contains("Validation failure"),
            "clean description for the reviewer: {}",
            task.description
        );
        assert!(
            task.worker_logs
                .contains("[eval] ASSERTION: FIXED is missing"),
            "the failing eval streamed to the logs"
        );
    }

    #[tokio::test]
    async fn exhausted_retries_park_the_task_for_a_human() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("echo agent-ran"))
            .with_eval(EvalCommand::fixed(
                "sh -c 'echo \"BOOM: always broken\"; exit 1'",
            ));

        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(!success);

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::FailedNeedsHuman);
        assert_eq!(task.retry_count, MAX_EVAL_RETRIES);
        // The agent ran once + twice more to fix it, then gave up.
        assert_eq!(
            task.worker_logs.matches("agent-ran").count(),
            MAX_EVAL_RETRIES as usize,
            "one attempt per allowed validation failure"
        );
        // The last failure text is kept for the human.
        assert!(task.description.contains("BOOM: always broken"));
    }

    #[tokio::test]
    async fn a_timed_out_evaluation_counts_as_a_failed_validation() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("true"))
            .with_eval(EvalCommand::fixed("sh -c 'echo starting; sleep 30'"))
            .with_eval_timeout(Duration::from_millis(300));

        let started = std::time::Instant::now();
        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(!success, "a hanging test suite never passes");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the sleep must be killed each round"
        );

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::FailedNeedsHuman);
        assert!(task.description.contains("timed out"));
        assert!(task.worker_logs.contains("[eval] starting"));
    }

    #[tokio::test]
    async fn an_unparsable_eval_spec_is_ignored_at_construction() {
        // EvalCommand::fixed falls back to auto-detect on garbage; with
        // nothing to detect in the workspace, the task goes straight to
        // REVIEW instead of failing every run for a config reason.
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("true"))
            .with_eval(EvalCommand::fixed("   "));
        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(success);
        assert_eq!(
            status_of(&Database::open(&paths.state_db()).unwrap(), "t0"),
            TaskStatus::Review
        );
    }

    // ------------------------------- Phase 9: auto-merge conflict resolution

    use crate::{approve_task, MergeOutcome};

    /// The workspace's resolved content: keeps the user's line *and* the
    /// agent's new logic, so both sides of the conflict survive.
    const RESOLVED_APP: &str = "fn main() { /* user was here */ println!(\"done\"); }\n";

    /// Drive `t0` into the post-approval-conflict state: workspace holding the
    /// agent's work, repository diverged on the same line, and
    /// `approve_task` having queued auto-resolution (`RESOLVING_CONFLICTS`).
    fn conflicted(paths: &ZuneshaPaths, db: &Database) {
        db.insert_task(&Task::new("t0", "Add the feature", "do it"))
            .unwrap();
        db.set_status("t0", TaskStatus::Todo).unwrap();
        db.set_status("t0", TaskStatus::InProgress).unwrap();
        WorkspaceManager::new(paths.clone())
            .create(db, "t0")
            .unwrap();
        // The agent's (pre-conflict) work.
        std::fs::write(
            paths.workspace("t0").join("app.rs"),
            "fn main() { println!(\"done\"); }\n",
        )
        .unwrap();
        db.set_status("t0", TaskStatus::Evaluating).unwrap();
        db.set_status("t0", TaskStatus::Review).unwrap();
        // The repository moves under us: same line, different content.
        std::fs::write(
            paths.root().join("app.rs"),
            "fn main() { /* user was here */ }\n",
        )
        .unwrap();

        let outcome = approve_task(paths, "t0").unwrap();
        assert!(
            matches!(
                outcome.integration,
                MergeOutcome::ConflictResolutionQueued { .. }
            ),
            "conflict queued for auto-resolution: {:?}",
            outcome.integration
        );
        assert_eq!(
            db.require_task("t0").unwrap().status,
            TaskStatus::ResolvingConflicts
        );
    }

    #[test]
    fn conflict_prompt_carries_the_strict_resolution_instruction() {
        let prompt = render_conflict_prompt("Add the feature", "some desc");
        assert!(
            prompt.starts_with("Add the feature\n\nsome desc"),
            "{prompt}"
        );
        assert!(
            prompt.contains(
                "There are Git merge conflict markers in the code. Resolve them by \
                 keeping the new logic but ensuring it integrates correctly with the \
                 updated base code."
            ),
            "the exact Phase 9 instruction: {prompt}"
        );
        // Empty descriptions still yield title-first output.
        let bare = render_conflict_prompt("t", "");
        assert!(bare.starts_with("t\n\n## Merge conflict"), "{bare}");
    }

    #[tokio::test]
    async fn conflict_resolution_round_trips_back_to_review_and_applies_cleanly() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        conflicted(&paths, &db);

        // The resolution agent: refuses to run unless the workspace really
        // holds markers, reports whether its prompt carried the strict
        // instruction, then merges both sides.
        let agent = AgentCommand::shell(
            r#"grep -q '<<<<<<<' app.rs || { echo "no markers in workspace" >&2; exit 1; }
               echo "prompt-instructs=$(echo "$0" | grep -c 'Resolve them by keeping the new logic')"
               printf 'fn main() { /* user was here */ println!("done"); }\n' > app.rs"#,
        );
        let pool = WorkerPool::new(paths.clone(), 1, agent)
            .with_eval(EvalCommand::fixed("sh -c 'echo eval-ok'"));

        let success = pool
            .spawn_resolution("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(success, "resolution + validation succeeded");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Review, "back at the human gate");
        assert!(
            task.worker_logs.contains("prompt-instructs=1"),
            "agent got the strict conflict prompt: {}",
            task.worker_logs
        );
        assert!(
            task.worker_logs.contains("[eval] eval-ok"),
            "the result went through EVALUATING: {}",
            task.worker_logs
        );
        assert!(
            !task.diff.contains("<<<<<<<"),
            "the reviewed diff is marker-free: {}",
            task.diff
        );

        // The human approves again — now the diff applies cleanly.
        let outcome = approve_task(&paths, "t0").unwrap();
        assert!(
            matches!(outcome.integration, MergeOutcome::Applied { .. }),
            "second approval lands: {:?}",
            outcome.integration
        );
        assert_eq!(outcome.task.status, TaskStatus::Done);
        let landed = std::fs::read_to_string(paths.root().join("app.rs")).unwrap();
        assert!(!landed.contains("<<<<<<<"), "clean merge in the tree");
        assert_eq!(landed, RESOLVED_APP, "both sides of the conflict kept");
    }

    #[tokio::test]
    async fn resolution_failure_parks_the_task_instead_of_looping_or_requeueing() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        conflicted(&paths, &db);

        // The agent cannot reconcile the overlap and bails out.
        let pool = WorkerPool::new(
            paths.clone(),
            1,
            AgentCommand::shell("echo giving-up >&2; exit 1"),
        );
        let success = pool
            .spawn_resolution("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(!success);

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(
            task.status,
            TaskStatus::FailedNeedsHuman,
            "not TODO (no from-scratch re-plan exists) and not a silent loop"
        );
        assert!(task.worker_logs.contains("giving-up"));
        // The markers are still in the workspace for the human to fix.
        assert!(
            std::fs::read_to_string(paths.workspace("t0").join("app.rs"))
                .unwrap()
                .contains("<<<<<<<")
        );
    }

    #[tokio::test]
    async fn failed_validation_during_resolution_retries_in_place_then_reviews() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        conflicted(&paths, &db);

        // Agent: asserts markers are present on every attempt (attempt 1 only
        // notes them; attempt 2 merges both sides), and "fixes" the code only
        // after one failed validation (Phase 8 pattern). It reports what its
        // prompt contained — the conflict instruction must survive retries.
        let agent = AgentCommand::shell(
            r#"grep -q '<<<<<<<' app.rs || { echo "markers vanished" >&2; exit 1; }
               if [ -f ATTEMPTED ]; then
                   echo fixed > FIXED
                   printf 'fn main() { /* user was here */ println!("done"); }\n' > app.rs
               else
                   echo 1 > ATTEMPTED
               fi
               echo "conflict-in-prompt=$(echo "$0" | grep -c 'conflict markers in the code')"
               echo "validation-in-prompt=$(echo "$0" | grep -c 'ASSERTION: FIXED')"
"#,
        );
        let eval = EvalCommand::fixed(
            "sh -c 'test -f FIXED || { echo \"ASSERTION: FIXED is missing\"; exit 1; }'",
        );
        let pool = WorkerPool::new(paths.clone(), 1, agent).with_eval(eval);
        let mut rx = pool.subscribe();

        let success = pool
            .spawn_resolution("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert!(
            success,
            "the retry resolved and validated; status={} retries={} logs={}",
            task.status, task.retry_count, task.worker_logs
        );
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 1, "one failed validation");
        // Both prompts carried the conflict instruction (attempt 1 and the
        // retry), and the retry additionally carried the validation errors.
        assert_eq!(
            task.worker_logs.matches("conflict-in-prompt=1").count(),
            2,
            "instruction present on every attempt: {}",
            task.worker_logs
        );
        assert!(
            task.worker_logs.contains("validation-in-prompt=0"),
            "first attempt had no failures yet: {}",
            task.worker_logs
        );
        assert!(
            task.worker_logs.contains("validation-in-prompt=1"),
            "retry prompt carried the validation errors: {}",
            task.worker_logs
        );
        assert!(
            !task.description.contains("Validation failure"),
            "clean description for the reviewer: {}",
            task.description
        );
        // The whole Phase 9 loop, with no TODO/IN_PROGRESS claim anywhere:
        // resolution runs are entered and re-entered from RESOLVING_CONFLICTS.
        let mut transitions = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let WorkerEvent::StatusChanged { from, to, .. } = ev {
                transitions.push((from, to));
            }
        }
        assert_eq!(
            transitions,
            vec![
                (TaskStatus::ResolvingConflicts, TaskStatus::Evaluating),
                (TaskStatus::Evaluating, TaskStatus::ResolvingConflicts),
                (TaskStatus::ResolvingConflicts, TaskStatus::Evaluating),
                (TaskStatus::Evaluating, TaskStatus::Review),
            ],
            "conflict retries loop RESOLVING_CONFLICTS <-> EVALUATING"
        );
    }

    #[tokio::test]
    async fn agent_timeout_stops_the_run_and_returns_to_todo() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("sleep 10"))
            .with_agent_timeout(Duration::from_millis(200))
            .with_sandbox_mode(SandboxMode::Off);
        let started = std::time::Instant::now();
        let success = pool
            .spawn_task("t0".into())
            .await
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        assert!(!success, "a timed-out attempt is not a successful run");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout must not wait for the sleeping agent"
        );

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Todo);
        assert!(
            task.description.contains("## Agent timeout"),
            "the retry sees why the run stopped: {}",
            task.description
        );
        assert!(
            task.worker_logs.contains("timed out"),
            "the live log explains the timeout: {}",
            task.worker_logs
        );

        let run = db.latest_run("t0").unwrap().expect("run row");
        assert_eq!(run.state, "TODO");
        let kinds: Vec<String> = db
            .list_run_events("t0", 100)
            .unwrap()
            .into_iter()
            .map(|event| event.kind)
            .collect();
        for expected in [
            "run_started",
            "agent_started",
            "agent_finished",
            "run_interrupted",
            "run_finished",
        ] {
            assert!(
                kinds.iter().any(|kind| kind == expected),
                "missing {expected} in {kinds:?}"
            );
        }
    }

    #[tokio::test]
    async fn cancellation_stops_the_run_and_releases_the_claim() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("sleep 10"))
            .with_agent_timeout(Duration::from_secs(60))
            .with_sandbox_mode(SandboxMode::Off);
        let handle = pool.spawn_task("t0".into()).await.unwrap();
        // Give the worker a moment to reach the agent wait, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(pool.cancel("t0"), "the live worker should be registered");

        let success = handle.await.unwrap().unwrap();
        assert!(!success, "cancelled is not success");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Todo);
        assert!(
            task.description.contains("## Run cancelled"),
            "the retry sees the cancellation: {}",
            task.description
        );
        assert!(
            task.worker_logs.contains("cancelled by user"),
            "the live log explains the cancellation: {}",
            task.worker_logs
        );
        assert!(
            pool.active_tasks().is_empty(),
            "the control is removed when the worker finishes"
        );
    }

    #[tokio::test]
    async fn cancellation_during_validation_stops_eval_and_releases_the_claim() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1);

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("true"))
            .with_eval(EvalCommand::fixed("sh -c 'echo eval-started; sleep 30'"))
            .with_eval_timeout(Duration::from_secs(60))
            .with_sandbox_mode(SandboxMode::Off);
        let handle = pool.spawn_task("t0".into()).await.unwrap();

        // Wait until the evaluation command is actually running, then cancel.
        let mut saw_eval = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let db = Database::open(&paths.state_db()).unwrap();
            if db
                .require_task("t0")
                .unwrap()
                .worker_logs
                .contains("eval-started")
            {
                saw_eval = true;
                break;
            }
        }
        assert!(saw_eval, "the evaluation should have started");
        assert!(pool.cancel("t0"), "the live worker should be registered");

        let success = handle.await.unwrap().unwrap();
        assert!(!success, "cancelled validation is not success");

        let db = Database::open(&paths.state_db()).unwrap();
        let task = db.require_task("t0").unwrap();
        assert_eq!(task.status, TaskStatus::Todo);
        assert!(
            task.description.contains("## Run cancelled"),
            "the retry sees the cancellation: {}",
            task.description
        );
        assert!(
            task.worker_logs.contains("cancelled by user"),
            "the live log explains the cancellation: {}",
            task.worker_logs
        );
        let run = db.latest_run("t0").unwrap().expect("run row");
        assert_eq!(run.state, "TODO");
        let events = db.list_run_events("t0", 100).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.kind == "validation_started"),
            "missing validation_started"
        );
        assert!(
            events.iter().any(|event| {
                event.kind == "validation_finished" && event.payload.contains("\"cancelled\":true")
            }),
            "missing cancelled validation_finished: {:?}",
            events
                .iter()
                .map(|event| &event.payload)
                .collect::<Vec<_>>()
        );
        assert!(
            events.iter().any(|event| event.kind == "run_interrupted"),
            "missing run_interrupted"
        );
        assert!(pool.active_tasks().is_empty());
    }

    #[tokio::test]
    async fn only_conflicted_tasks_can_spawn_a_resolution() {
        let (_repo, paths) = scratch_install();
        let db = Database::open(&paths.state_db()).unwrap();
        ready_tasks(&db, 1); // TODO, not RESOLVING_CONFLICTS

        let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::shell("true"));
        let err = pool.spawn_resolution("t0".into()).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("only RESOLVING_CONFLICTS tasks can auto-resolve"),
            "{err}"
        );

        // Nothing was claimed or built.
        let db = Database::open(&paths.state_db()).unwrap();
        assert_eq!(status_of(&db, "t0"), TaskStatus::Todo);
        assert!(!paths.workspace("t0").exists());
    }
}
