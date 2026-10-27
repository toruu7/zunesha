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
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{broadcast, mpsc, Semaphore};

use crate::config::Config;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::eval::{self, EvalCommand};
use crate::paths::ZuneshaPaths;
use crate::task::{Task, TaskStatus, MAX_EVAL_RETRIES};
use crate::workspace::WorkspaceManager;

/// Flush accumulated logs to the database at least this often while an agent
/// runs, so the UI shows progress even for a long, quiet step.
const LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(400);

/// Default cap on simultaneously running agents.
pub const DEFAULT_MAX_CONCURRENCY: usize = 3;

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
    /// 2. The model saved by `zou model` in the user-wide config, invoked as
    ///    `opencode run --auto --model <id> {prompt}`.
    /// 3. The installed `opencode` CLI as
    ///    `opencode run {prompt}`.
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
    /// The built-in agent invocation (`opencode run {prompt}`).
    ///
    /// Verified against the installed CLI: `opencode run [flags] [<message...>]`
    /// takes the prompt as a trailing argument. The earlier
    /// `opencode --task <prompt>` form was rejected by the real binary.
    fn builtin() -> Self {
        AgentCommand {
            program: "opencode".to_string(),
            args: vec!["run".to_string(), "{prompt}".to_string()],
        }
    }

    /// Build the default OpenCode agent for a saved model.
    ///
    /// `--auto` is intentional: Zunesha runs headlessly with stdin closed, so
    /// an interactive permission prompt would hang the task.
    pub fn from_config(config: &Config) -> Option<Self> {
        let model = config.model()?;
        Some(AgentCommand {
            program: "opencode".to_string(),
            args: vec![
                "run".to_string(),
                "--auto".to_string(),
                "--model".to_string(),
                model.to_string(),
                "{prompt}".to_string(),
            ],
        })
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

/// A bounded pool of concurrently running coding agents.
pub struct WorkerPool {
    max_concurrency: usize,
    semaphore: Arc<Semaphore>,
    db_path: PathBuf,
    paths: ZuneshaPaths,
    agent: AgentCommand,
    eval: EvalCommand,
    eval_timeout: Duration,
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

    /// The cap on simultaneous agents.
    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency
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
            // output) can't show stale logs/diff against the new attempt.
            db.set_worker_logs(&task_id, "")?;
            db.set_diff(&task_id, "")?;
            task
        };
        self.emit(WorkerEvent::StatusChanged {
            task_id: task_id.clone(),
            from: TaskStatus::Todo,
            to: TaskStatus::InProgress,
        });

        // Semaphore permit is acquired *inside* the worker so this method
        // returns immediately; the task simply waits its turn.
        Ok(self.launch(task, RunMode::Normal))
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
            // Fresh attempt: drop the pre-conflict logs/diff so the reviewer
            // sees only this run (the diff is regenerated on success, now
            // against the rebased base).
            db.set_worker_logs(&task_id, "")?;
            db.set_diff(&task_id, "")?;
            task
        };
        Ok(self.launch(task, RunMode::ResolveConflicts))
    }

    /// Wrap a claimed task in the per-worker state and spawn it under the
    /// concurrency cap.
    fn launch(&self, task: Task, mode: RunMode) -> tokio::task::JoinHandle<Result<bool>> {
        let pool = Arc::new(WorkerHandle {
            db_path: self.db_path.clone(),
            paths: self.paths.clone(),
            agent: self.agent.clone(),
            eval: self.eval.clone(),
            eval_timeout: self.eval_timeout,
            events: self.events.clone(),
        });
        let semaphore = self.semaphore.clone();

        tokio::spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| Error::Workspace("worker pool closed".into()))?;
            run_one(&pool, task, mode).await
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
/// In both modes the workspace's Phase 10 `CODEBASE_MAP.md` is appended to
/// the prompt, read once during setup.
async fn run_one(pool: &Arc<WorkerHandle>, task: Task, mode: RunMode) -> Result<bool> {
    let task_id = task.id.clone();

    // Workspace setup does blocking filesystem + git work; keep it off the
    // async runtime. The Phase 10 codebase map is read here too — once per
    // run, identical for every prompt of that run.
    let (ws_dir, codebase_map) = {
        let pool = pool.clone();
        let task_id = task_id.clone();
        tokio::task::spawn_blocking(move || -> Result<(String, Option<String>)> {
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
            Ok((ws_dir, map))
        })
        .await
        .map_err(|e| Error::Workspace(format!("workspace setup panicked: {e}")))??
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
            // Phase 10: task text first, the static repository map last —
            // reference material, identical for every attempt of this run.
            if let Some(map) = &codebase_map {
                prompt.push_str("\n\n");
                prompt.push_str(map);
            }
            if !run_agent_once(pool, &ws_dir, &prompt, &line_tx).await? {
                // Normal run: the agent failed; a human decides the next
                // attempt. Conflict run: the overlap could not be reconciled,
                // and there is no from-scratch re-plan — park for a human.
                return Ok::<(TaskStatus, bool), Error>(match mode {
                    RunMode::Normal => (TaskStatus::Todo, false),
                    RunMode::ResolveConflicts => (TaskStatus::FailedNeedsHuman, false),
                });
            }

            // --- Validation (Phase 8 self-healing loop) -------------------
            transition(pool, &task_id, &mut status, TaskStatus::Evaluating).await?;
            let Some(argv) = pool.eval.resolve(Path::new(&ws_dir)) else {
                // Nothing to validate in this project: exit-0 is as good as
                // a green test run, so go straight to human review.
                return Ok((TaskStatus::Review, true));
            };
            let outcome =
                eval::run_evaluation(&argv, Path::new(&ws_dir), pool.eval_timeout, &line_tx).await;
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
            transition(pool, &task_id, &mut status, retry_to).await?;
            // Loop: the agent re-runs in the same workspace with the errors.
        }
    }
    .await;

    // Stop the collector (closes the channel) and take its final flush.
    drop(line_tx);
    let _ = collector.await;

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

    transition(pool, &task_id, &mut status, to).await?;
    pool.emit(WorkerEvent::TaskFinished { task_id, success });

    Ok(success)
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
/// into the run's shared log channel, and wait for it to exit.
/// Returns whether the agent exited 0.
async fn run_agent_once(
    pool: &WorkerHandle,
    ws_dir: &str,
    prompt: &str,
    line_tx: &mpsc::UnboundedSender<(StreamKind, String)>,
) -> Result<bool> {
    let (program, args) = pool.agent.render(prompt, ws_dir);

    // Spawn the agent with piped output, running inside the workspace.
    let mut child = Command::new(&program)
        .args(&args)
        .current_dir(ws_dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
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

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Workspace(format!("failed to wait on agent: {e}")))?;

    // Drain the streams fully before reporting the exit.
    let _ = out_task.await;
    let _ = err_task.await;

    Ok(status.success())
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
                    Some((stream, line)) => {
                        pool.emit(WorkerEvent::Log {
                            task_id: task_id.clone(),
                            stream,
                            line: line.clone(),
                            seq,
                        });
                        seq += 1;
                        buffer.push_str(&line);
                        buffer.push('\n');
                        dirty = true;
                    }
                    None => break, // both streams finished
                }
            }
            _ = ticker.tick(), if dirty => {
                flush_logs(&pool, &task_id, &buffer).await;
                dirty = false;
            }
        }
    }

    // Final flush of anything left over.
    if dirty {
        flush_logs(&pool, &task_id, &buffer).await;
    }
}

async fn flush_logs(pool: &WorkerHandle, task_id: &str, buffer: &str) {
    let db_path = pool.db_path.clone();
    let task_id = task_id.to_string();
    let logs = buffer.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        if let Ok(db) = Database::open(&db_path) {
            let _ = db.set_worker_logs(&task_id, &logs);
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
    fn saved_model_builds_an_auto_opencode_command() {
        let config = Config {
            agent: crate::config::AgentConfig {
                model: Some("opencode/deepseek-v4.1-flash-free".into()),
            },
        };
        let cmd = AgentCommand::from_config(&config).expect("saved model");
        assert_eq!(cmd.program, "opencode");
        assert_eq!(
            cmd.args,
            vec![
                "run",
                "--auto",
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
