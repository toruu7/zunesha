//! Planner: turn a feature prompt into a DAG of tasks.
//!
//! Two implementations share one signature:
//!
//! * **Built-in mock** ([`Planner::Mock`], the default) — a deterministic
//!   three-step chain, enough to exercise the whole pipeline without an LLM.
//! * **External command** ([`Planner::Command`]) — `ZOU_PLANNER_CMD` names any
//!   command (an LLM CLI such as `llm`, `ollama run …`, `claude -p`, or a
//!   script) that receives the full planning prompt on **stdin** and prints one
//!   JSON object on **stdout**:
//!
//!   ```json
//!   {"tasks":[
//!     {"key":"schema","title":"Add the rate-limit schema","description":"…","depends_on":[]},
//!     {"key":"middleware","title":"Enforce the limit","description":"…","depends_on":["schema"]}
//!   ]}
//!   ```
//!
//!   The plan is validated *before* anything is written — unique keys, known
//!   dependencies, no self-edges, an acyclic graph, at most [`MAX_PLAN_TASKS`]
//!   tasks — so a confused model yields one clear error instead of a broken
//!   DAG. Markdown fences and surrounding prose are tolerated: the first JSON
//!   value in the output wins.
//!
//! The planning prompt includes a bounded repository map (the same one agents
//! receive, see [`crate::codebase_map`]) so the model can reference real files.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::codebase_map;
use crate::dag;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::prior_context;
use crate::task::Task;
use crate::worker::split_command_line;

/// Default wall-clock limit for one external planner run, overridable with
/// `ZOU_PLANNER_TIMEOUT_SECS`.
pub const DEFAULT_PLANNER_TIMEOUT_SECS: u64 = 120;

/// Hard cap on tasks a single plan may create.
pub const MAX_PLAN_TASKS: usize = 32;

/// Cap on the feature request embedded in the planning prompt (bytes).
///
/// Kept below a pipe buffer so writing the prompt to a slow reader can never
/// deadlock the planner.
const MAX_REQUEST_BYTES: usize = 24 * 1024;

/// Cap on a title stored from an external plan (chars).
const MAX_TITLE_CHARS: usize = 200;

/// Cap on a description stored from an external plan (chars).
const MAX_DESCRIPTION_CHARS: usize = 8_000;

/// Cap on command output quoted back in planner errors (chars).
const MAX_ERROR_DETAIL_CHARS: usize = 2_000;

/// Monotonic counter so ids generated within the same nanosecond stay unique.
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Which planner to use.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Planner {
    /// Deterministic three-step chain; the default when nothing is configured.
    #[default]
    Mock,
    /// External command: prompt on stdin, one JSON plan on stdout.
    Command {
        argv: Vec<String>,
        timeout: Duration,
    },
}

impl Planner {
    /// Resolve the planner from the environment.
    ///
    /// `ZOU_PLANNER_CMD` selects an external command (shell-style quoting, like
    /// `ZOU_AGENT_CMD`); `ZOU_PLANNER_TIMEOUT_SECS` bounds one run. An unset or
    /// empty variable keeps the built-in mock; an unparsable command warns and
    /// falls back to it rather than silently planning with the wrong tool.
    pub fn from_env() -> Self {
        let timeout = timeout_from_env();
        match std::env::var("ZOU_PLANNER_CMD") {
            Ok(spec) if !spec.trim().is_empty() => Self::parse(&spec, timeout),
            _ => Self::Mock,
        }
    }

    /// An explicit command spec with the default timeout (tests, embedding).
    pub fn command(spec: &str) -> Self {
        Self::command_with_timeout(spec, Duration::from_secs(DEFAULT_PLANNER_TIMEOUT_SECS))
    }

    /// An explicit command spec with an explicit wall-clock limit.
    pub fn command_with_timeout(spec: &str, timeout: Duration) -> Self {
        Self::parse(spec, timeout)
    }

    fn parse(spec: &str, timeout: Duration) -> Self {
        match split_command_line(spec) {
            Ok(argv) if !argv.is_empty() => Self::Command { argv, timeout },
            Ok(_) | Err(_) => {
                eprintln!(
                    "warning: ignoring ZOU_PLANNER_CMD (invalid command line); using the built-in planner"
                );
                Self::Mock
            }
        }
    }

    /// Human-readable name for CLI output and errors.
    pub fn label(&self) -> &'static str {
        match self {
            Planner::Mock => "built-in planner",
            Planner::Command { .. } => "ZOU_PLANNER_CMD",
        }
    }
}

/// Plan a feature with the planner selected by the environment.
///
/// Inserts the resulting tasks and dependency edges, then runs one engine pass
/// so the tasks with no unmet prerequisites land in `TODO`. Returns the created
/// task ids in topological order (prerequisites first). An empty prompt plans
/// nothing.
pub fn plan(db: &Database, prompt: &str, repo_root: &Path) -> Result<Vec<String>> {
    plan_with(db, prompt, &Planner::from_env(), repo_root)
}

/// Plan a feature with attachments, storing the files under `.zou/attachments/`.
///
/// The planner sees the attachment section, and every resulting task gets the
/// same section appended (unless the planner already copied it) so the agent
/// can read screenshots/specs from their absolute paths.
pub fn plan_with_attachments(
    db: &Database,
    prompt: &str,
    planner: &Planner,
    repo_root: &Path,
    uploads: &[crate::attachments::AttachmentUpload],
) -> Result<Vec<String>> {
    if prompt.trim().is_empty() {
        return Ok(Vec::new());
    }
    let saved = crate::attachments::save_attachments(repo_root, uploads)?;
    let planner_prompt = crate::attachments::append_attachments(prompt, &saved);
    let ids = plan_with(db, &planner_prompt, planner, repo_root)?;
    if !saved.is_empty() {
        for id in &ids {
            let task = db.require_task(id)?;
            if !task.description.contains("## Attachments") {
                let description = crate::attachments::append_attachments(&task.description, &saved);
                db.update_task(id, None, Some(&description))?;
            }
        }
    }
    Ok(ids)
}

/// Plan with an explicit [`Planner`] (tests, embedding, daemon reuse).
///
/// The draft is fully validated before the first row is written: a rejected
/// plan leaves the database untouched.
pub fn plan_with(
    db: &Database,
    prompt: &str,
    planner: &Planner,
    repo_root: &Path,
) -> Result<Vec<String>> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        // A planner with nothing to plan should be a loud no-op rather than
        // silently creating empty tasks.
        return Ok(Vec::new());
    }

    let draft = match planner {
        Planner::Mock => mock_draft(prompt),
        Planner::Command { argv, timeout } => {
            let output = run_planner(argv, *timeout, prompt, repo_root)?;
            parse_plan(&output)?
        }
    };

    let tasks = validate(draft)?;
    insert_plan(db, &tasks)
}

// ---------------------------------------------------------------------------
// Built-in planner
// ---------------------------------------------------------------------------

/// The deterministic mock: analyze → implement → verify, chained.
fn mock_draft(prompt: &str) -> PlanDraft {
    PlanDraft {
        tasks: vec![
            PlannedTask {
                key: "design".to_string(),
                title: format!("Analyze & design: {prompt}"),
                description: plan_step_description(prompt, 1),
                depends_on: Vec::new(),
            },
            PlannedTask {
                key: "implement".to_string(),
                title: format!("Implement the change: {prompt}"),
                description: plan_step_description(prompt, 2),
                depends_on: vec!["design".to_string()],
            },
            PlannedTask {
                key: "verify".to_string(),
                title: format!("Test & document: {prompt}"),
                description: plan_step_description(prompt, 3),
                depends_on: vec!["implement".to_string()],
            },
        ],
    }
}

/// Description for step N of the mock plan.
fn plan_step_description(prompt: &str, step: u8) -> String {
    match step {
        1 => format!("Understand the request and sketch an implementation plan for: {prompt}"),
        2 => format!("Implement the core of: {prompt}"),
        _ => format!("Add tests and docs covering: {prompt}"),
    }
}

// ---------------------------------------------------------------------------
// External planner: prompt → subprocess → JSON draft
// ---------------------------------------------------------------------------

/// Run the external planner and return its raw stdout.
fn run_planner(
    argv: &[String],
    timeout: Duration,
    request: &str,
    repo_root: &Path,
) -> Result<String> {
    let Some((program, args)) = argv.split_first() else {
        return Err(Error::Planner("the planner command is empty".into()));
    };
    if program.is_empty() {
        return Err(Error::Planner("the planner command is empty".into()));
    }

    // Files, not pipes: a planner that leaves a grandchild running (e.g.
    // `sh script` whose `sleep` outlives the shell) must not be able to hold a
    // pipe open and stall the wait. The child's output is small and bounded by
    // the model's answer.
    let scratch = tempfile::tempdir()?;
    let out_path = scratch.path().join("stdout");
    let err_path = scratch.path().join("stderr");

    let prompt = build_prompt(request, repo_root);
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(std::fs::File::create(&out_path)?))
        .stderr(Stdio::from(std::fs::File::create(&err_path)?));
    crate::child_env::prepare(&mut command, repo_root);
    let mut child = command
        .spawn()
        .map_err(|e| Error::Planner(format!("failed to start planner `{program}`: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        // A planner that exits before consuming the prompt is not an error by
        // itself: its exit status and output decide.
        let _ = stdin.write_all(prompt.as_bytes());
        let _ = stdin.flush();
    }

    let status = wait_with_timeout(&mut child, timeout)?;
    let stdout = read_lossy(&out_path);
    let stderr = read_lossy(&err_path);

    if !status.success() {
        return Err(Error::Planner(format!(
            "planner `{program}` {status}{}",
            detail_of(&stderr, &stdout)
        )));
    }
    if stdout.trim().is_empty() {
        return Err(Error::Planner(format!(
            "planner `{program}` produced no output on stdout{}",
            detail_of(&stderr, "")
        )));
    }
    Ok(stdout)
}

/// Wait for `child`, killing it once `timeout` elapses.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Planner(format!(
                "planner timed out after {}s and was killed",
                timeout.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Read a scratch output file lossily (planner stderr need not be UTF-8).
fn read_lossy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The planning prompt: instructions, the request, prior work, and the
/// repository map.
fn build_prompt(request: &str, repo_root: &Path) -> String {
    let request = truncate_bytes(request.trim(), MAX_REQUEST_BYTES);
    let map = codebase_map::generate(repo_root);
    let map = if map.trim().is_empty() {
        "(the repository is empty)".to_string()
    } else {
        map
    };
    let prior = prior_context::generate(repo_root)
        .map(|context| {
            format!(
                "Prior agent context (work already present in the checkout; plan around\n\
                 it, do not ask an agent to redo it):\n\
                 \"\"\"\n\
                 {context}\n\
                 \"\"\"\n\
                 \n\
                 "
            )
        })
        .unwrap_or_default();

    format!(
        "You are the planner stage of Zunesha, a local-first coding orchestrator.\n\
         Decompose the feature request below into a small DAG of tasks for autonomous\n\
         coding agents. Each task is executed by one agent in an isolated git\n\
         workspace, validated by the project's tests, and merged only after a human\n\
         approves it.\n\
         \n\
         Feature request:\n\
         \"\"\"\n\
         {request}\n\
         \"\"\"\n\
         \n\
         {prior}\
         Repository map (paths relative to the repo root):\n\
         \"\"\"\n\
         {map}\n\
         \"\"\"\n\
         \n\
         Rules:\n\
         - 1-8 tasks. Prefer the fewest tasks that can be reviewed independently.\n\
         - Every task needs a short `key` (unique in this plan), a `title`, and a\n\
           self-contained `description` with enough context for an agent that has\n\
           not seen this message.\n\
         - Maximize parallelism: start independent tasks with `depends_on: []` so\n\
           they can be worked on concurrently. Only declare dependencies where one\n\
           task strictly requires code, types, or schemas produced by an earlier\n\
           task, or when two tasks must touch the exact same files to avoid merge\n\
           conflicts.\n\
         - Do not serialize tasks unnecessarily; independent components, orthogonal\n\
           tests, or documentation can proceed in parallel.\n\
         - Treat prior agent context as existing work. Do not plan tasks that\n\
           revert or duplicate it; plan only the remaining delta.\n\
         - Do not add review or merge tasks; humans do that.\n\
         - The graph must be acyclic.\n\
         \n\
         Answer with one JSON object and nothing else (no markdown fence, no prose):\n\
         {{\"tasks\":[{{\"key\":\"short-slug\",\"title\":\"...\",\"description\":\"...\",\"depends_on\":[\"other-key\"]}}]}}\n"
    )
}

/// Planner timeout from `ZOU_PLANNER_TIMEOUT_SECS` (default
/// [`DEFAULT_PLANNER_TIMEOUT_SECS`]; `0` and garbage fall back to the default).
fn timeout_from_env() -> Duration {
    let secs = std::env::var("ZOU_PLANNER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(DEFAULT_PLANNER_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Draft parsing & validation
// ---------------------------------------------------------------------------

/// The wire shape a planner must print (extra fields are ignored).
#[derive(Debug, Deserialize)]
struct PlanDraft {
    #[serde(default)]
    tasks: Vec<PlannedTask>,
}

/// One task in a planner draft.
#[derive(Debug, Deserialize)]
struct PlannedTask {
    key: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    /// `null` is treated as "no dependencies" so a loose model still works.
    #[serde(default, alias = "dependsOn", deserialize_with = "string_list")]
    depends_on: Vec<String>,
}

/// Deserialize `Vec<String>` from a present array or from `null`/absent.
fn string_list<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Parse stdout into a draft, tolerating fences and prose around the JSON.
fn parse_plan(output: &str) -> Result<PlanDraft> {
    let json = extract_json(output)?;
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        Error::Planner(format!(
            "planner output is not valid JSON ({e}); output began: {}",
            snippet(json)
        ))
    })?;

    let tasks_value = match value {
        serde_json::Value::Object(mut object) => object.remove("tasks").ok_or_else(|| {
            Error::Planner(
                "planner output has no `tasks` array (expected {\"tasks\":[...]})".into(),
            )
        })?,
        array @ serde_json::Value::Array(_) => array,
        _ => {
            return Err(Error::Planner(
                "planner output must be a JSON object with a `tasks` array (or a bare array)"
                    .into(),
            ))
        }
    };

    let array = tasks_value
        .as_array()
        .ok_or_else(|| Error::Planner("planner `tasks` must be an array of task objects".into()))?;

    let mut tasks = Vec::with_capacity(array.len());
    for (i, task) in array.iter().enumerate() {
        tasks.push(
            serde_json::from_value::<PlannedTask>(task.clone())
                .map_err(|e| Error::Planner(format!("planner task #{} is invalid: {e}", i + 1)))?,
        );
    }
    Ok(PlanDraft { tasks })
}

/// The outermost JSON value in `text` (first `{`/`[` through the last
/// `}`/`]`), so markdown fences and chatter do not defeat parsing.
fn extract_json(text: &str) -> Result<&str> {
    let start = text.find(['{', '[']);
    let end = text.rfind(['}', ']']);
    match (start, end) {
        (Some(start), Some(end)) if end > start => Ok(&text[start..=end]),
        _ => Err(Error::Planner(format!(
            "planner produced no JSON; output began: {}",
            snippet(text)
        ))),
    }
}

/// Normalize and validate a draft; returns tasks in topological order
/// (prerequisites first) with duplicate edges removed.
fn validate(draft: PlanDraft) -> Result<Vec<PlannedTask>> {
    if draft.tasks.is_empty() {
        return Err(Error::Planner("planner returned no tasks".into()));
    }
    if draft.tasks.len() > MAX_PLAN_TASKS {
        return Err(Error::Planner(format!(
            "planner returned {} tasks, more than the {MAX_PLAN_TASKS}-task limit; \
             ask for a coarser decomposition",
            draft.tasks.len()
        )));
    }

    let mut tasks: Vec<PlannedTask> = draft
        .tasks
        .into_iter()
        .map(|mut task| {
            task.key = task.key.trim().to_string();
            task.title = truncate_chars(task.title.trim(), MAX_TITLE_CHARS);
            task.description = truncate_chars(task.description.trim(), MAX_DESCRIPTION_CHARS);
            task.depends_on = task
                .depends_on
                .into_iter()
                .map(|dep| dep.trim().to_string())
                .filter(|dep| !dep.is_empty())
                .collect();
            task
        })
        .collect();

    let mut index: HashMap<String, usize> = HashMap::with_capacity(tasks.len());
    for (i, task) in tasks.iter().enumerate() {
        if task.key.is_empty() {
            return Err(Error::Planner(format!(
                "planner task #{} has an empty `key`",
                i + 1
            )));
        }
        if task.key.chars().count() > 64
            || !task
                .key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(Error::Planner(format!(
                "planner task key `{}` must be 1-64 characters of [A-Za-z0-9._-]",
                task.key
            )));
        }
        if task.title.is_empty() {
            return Err(Error::Planner(format!(
                "planner task `{}` has an empty `title`",
                task.key
            )));
        }
        if let Some(previous) = index.insert(task.key.clone(), i) {
            return Err(Error::Planner(format!(
                "planner returned duplicate task key `{}` (#{} and #{})",
                task.key,
                previous + 1,
                i + 1
            )));
        }
    }

    for task in &mut tasks {
        let mut seen = HashSet::new();
        task.depends_on.retain(|dep| seen.insert(dep.clone()));
        for dep in &task.depends_on {
            if dep == &task.key {
                return Err(Error::Planner(format!(
                    "planner task `{}` depends on itself",
                    task.key
                )));
            }
            if !index.contains_key(dep) {
                return Err(Error::Planner(format!(
                    "planner task `{}` depends on unknown task `{dep}`",
                    task.key
                )));
            }
        }
    }

    topological_order(tasks, &index)
}

/// Kahn's algorithm: reject cycles, return prerequisites first.
fn topological_order(
    tasks: Vec<PlannedTask>,
    index: &HashMap<String, usize>,
) -> Result<Vec<PlannedTask>> {
    let count = tasks.len();
    let mut pending: Vec<usize> = tasks.iter().map(|task| task.depends_on.len()).collect();
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (i, task) in tasks.iter().enumerate() {
        for dep in &task.depends_on {
            dependents[index[dep]].push(i);
        }
    }

    let mut ready: VecDeque<usize> = (0..count).filter(|&i| pending[i] == 0).collect();
    let mut order = Vec::with_capacity(count);
    while let Some(i) = ready.pop_front() {
        order.push(i);
        for &dependent in &dependents[i] {
            pending[dependent] -= 1;
            if pending[dependent] == 0 {
                ready.push_back(dependent);
            }
        }
    }

    if order.len() != count {
        let mut stuck: Vec<&str> = (0..count)
            .filter(|&i| pending[i] > 0)
            .map(|i| tasks[i].key.as_str())
            .collect();
        stuck.sort_unstable();
        return Err(Error::Planner(format!(
            "planner returned a cycle (tasks involved: {})",
            stuck.join(", ")
        )));
    }

    let mut slots: Vec<Option<PlannedTask>> = tasks.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots[i].take()).collect())
}

// ---------------------------------------------------------------------------
// Insertion
// ---------------------------------------------------------------------------

/// Insert validated tasks and their edges, then promote the ready ones.
fn insert_plan(db: &Database, tasks: &[PlannedTask]) -> Result<Vec<String>> {
    let mut ids: HashMap<&str, String> = HashMap::with_capacity(tasks.len());
    let mut created = Vec::with_capacity(tasks.len());

    for task in tasks {
        let id = next_task_id();
        // A description is what the agent actually reads; a plan that omits it
        // still yields a usable task from the title alone.
        let description = if task.description.is_empty() {
            task.title.clone()
        } else {
            task.description.clone()
        };
        db.insert_task(&Task::new(&id, task.title.clone(), description))?;
        ids.insert(task.key.as_str(), id.clone());
        created.push(id);
    }

    for task in tasks {
        let id = &ids[task.key.as_str()];
        for dep in &task.depends_on {
            db.add_dependency(id, &ids[dep.as_str()])?;
        }
    }

    dag::promote_ready(db)?;
    Ok(created)
}

/// Generate a unique task id: `task-<unix_nanos>-<counter>`.
fn next_task_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("task-{nanos}-{n}")
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// First `max` chars, with a marker when cut.
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// First `max` bytes (on a char boundary), with a marker when cut.
fn truncate_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

/// One-line excerpt of command output for error messages.
fn snippet(text: &str) -> String {
    let flat = text.trim().replace('\n', " ");
    truncate_chars(&flat, 200)
}

/// `: <first line-ish of the command's output>` for error messages.
fn detail_of(stderr: &str, stdout: &str) -> String {
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    if text.trim().is_empty() {
        String::new()
    } else {
        let flat = text.trim().replace('\n', " ");
        format!(": {}", truncate_chars(&flat, MAX_ERROR_DETAIL_CHARS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;
    use std::path::PathBuf;

    fn db() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hello.txt"), "hi\n").unwrap();
        let path = dir.path().to_path_buf();
        (dir, path)
    }

    fn status_of(db: &Database, id: &str) -> TaskStatus {
        db.get_task(id).unwrap().unwrap().status
    }

    /// Write a shell script and return a planner spec that runs it.
    fn script(dir: &Path, body: &str) -> String {
        let path = dir.join("planner.sh");
        std::fs::write(&path, body).unwrap();
        format!("sh {}", path.display())
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn mock_plan(db: &Database, prompt: &str) -> Vec<String> {
        plan_with(db, prompt, &Planner::Mock, Path::new(".")).unwrap()
    }

    /// Run a planner script against a scratch repo.
    fn external(db: &Database, body: &str) -> Result<Vec<String>> {
        let (dir, _) = scratch();
        let spec = script(dir.path(), body);
        plan_with(db, "do the thing", &Planner::command(&spec), dir.path())
    }

    // -- built-in planner (unchanged behaviour) -----------------------------

    #[test]
    fn plan_creates_a_three_task_chain() {
        let db = db();
        let ids = mock_plan(&db, "add a dark mode toggle");

        assert_eq!(ids.len(), 3);
        assert_eq!(db.task_count().unwrap(), 3);
        assert_eq!(db.dependency_count().unwrap(), 2, "a 3-chain has 2 edges");

        assert_eq!(db.dependencies_of(&ids[1]).unwrap(), vec![ids[0].clone()]);
        assert_eq!(db.dependencies_of(&ids[2]).unwrap(), vec![ids[1].clone()]);
        assert!(db.dependencies_of(&ids[0]).unwrap().is_empty());
    }

    #[test]
    fn plan_head_is_ready_and_rest_are_backlog() {
        let db = db();
        let ids = mock_plan(&db, "add a dark mode toggle");

        assert_eq!(status_of(&db, &ids[0]), TaskStatus::Todo);
        assert_eq!(status_of(&db, &ids[1]), TaskStatus::Backlog);
        assert_eq!(status_of(&db, &ids[2]), TaskStatus::Backlog);
    }

    #[test]
    fn plan_tasks_reference_the_prompt() {
        let db = db();
        let ids = mock_plan(&db, "add a dark mode toggle");
        for id in &ids {
            let task = db.require_task(id).unwrap();
            assert!(
                task.title.contains("dark mode toggle")
                    || task.description.contains("dark mode toggle"),
                "prompt should appear in task text: {task:?}"
            );
        }
    }

    #[test]
    fn empty_prompt_plans_nothing() {
        let db = db();
        let ids = mock_plan(&db, "   ");
        assert!(ids.is_empty());
        assert_eq!(db.task_count().unwrap(), 0);
    }

    #[test]
    fn two_plans_produce_distinct_ids() {
        let db = db();
        let a = mock_plan(&db, "feature A");
        let b = mock_plan(&db, "feature B");
        assert_eq!(db.task_count().unwrap(), 6);
        for id_a in &a {
            assert!(!b.contains(id_a), "ids must be globally unique");
        }
    }

    #[test]
    fn plan_integrates_with_dag_engine_end_to_end() {
        let db = db();
        let ids = mock_plan(&db, "ship the feature");

        for id in &ids {
            assert_eq!(status_of(&db, id), TaskStatus::Todo, "{id} should be TODO");
            db.force_set_status(id, TaskStatus::Done).unwrap();
            dag::promote_ready(&db).unwrap();
        }

        let counts = db.task_counts_by_status().unwrap();
        let done = counts.iter().find(|(s, _)| *s == TaskStatus::Done).unwrap();
        assert_eq!(done.1, 3);
    }

    // -- external planner ---------------------------------------------------

    #[test]
    fn external_plan_creates_tasks_edges_and_promotes_the_head() {
        let db = db();
        let ids = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[
  {"key":"api","title":"Add the API","description":"Implement the endpoint","depends_on":[]},
  {"key":"ui","title":"Add the UI","description":"Wire the dashboard","depends_on":["api"]},
  {"key":"docs","title":"Write the docs","description":"Document it","depends_on":["api"]}
]}'
"#,
        )
        .unwrap();

        assert_eq!(ids.len(), 3);
        assert_eq!(db.task_count().unwrap(), 3);
        assert_eq!(db.dependency_count().unwrap(), 2);

        // Topological order: the root comes first, and it is the only ready one.
        let root = db.require_task(&ids[0]).unwrap();
        assert_eq!(root.title, "Add the API");
        assert_eq!(root.status, TaskStatus::Todo);
        assert_eq!(status_of(&db, &ids[1]), TaskStatus::Backlog);
        assert_eq!(status_of(&db, &ids[2]), TaskStatus::Backlog);
        assert!(db.dependencies_of(&ids[0]).unwrap().is_empty());
        for id in &ids[1..] {
            assert_eq!(db.dependencies_of(id).unwrap(), vec![ids[0].clone()]);
        }
    }

    #[test]
    fn external_plan_receives_the_request_and_repository_map_on_stdin() {
        let (dir, _) = scratch();
        let marker = dir.path().join("prompt.txt");
        let spec = script(
            dir.path(),
            &format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s' '{{\"tasks\":[{{\"key\":\"a\",\"title\":\"One\",\"description\":\"D\"}}]}}'\n",
                marker.display()
            ),
        );

        let db = db();
        let ids = plan_with(
            &db,
            "add rate limiting",
            &Planner::command(&spec),
            dir.path(),
        )
        .unwrap();
        assert_eq!(ids.len(), 1);

        let prompt = std::fs::read_to_string(&marker).unwrap();
        assert!(
            prompt.contains("add rate limiting"),
            "request missing: {prompt}"
        );
        assert!(
            prompt.contains("hello.txt"),
            "repository map missing: {prompt}"
        );
        assert!(
            prompt.contains("\"tasks\""),
            "schema hint missing: {prompt}"
        );
    }

    #[test]
    fn external_plan_receives_prior_agent_context_on_stdin() {
        let (dir, _) = scratch();
        let root = dir.path();
        git(root, &["init", "--quiet"]);
        git(root, &["config", "user.email", "t@t.local"]);
        git(root, &["config", "user.name", "T"]);
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "-m", "baseline"]);
        std::fs::write(root.join("hello.txt"), "hi from a prior agent\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Keep the public API stable.\n").unwrap();
        std::fs::write(root.join("plan.md"), "scratch plan\n").unwrap();

        let marker = root.join("prompt.txt");
        let spec = script(
            root,
            &format!(
                "#!/bin/sh\ncat > '{}'\nprintf '%s' '{{\"tasks\":[{{\"key\":\"a\",\"title\":\"One\",\"description\":\"D\"}}]}}'\n",
                marker.display()
            ),
        );

        let db = db();
        let ids = plan_with(&db, "add rate limiting", &Planner::command(&spec), root).unwrap();
        assert_eq!(ids.len(), 1);

        let prompt = std::fs::read_to_string(&marker).unwrap();
        assert!(
            prompt.contains("Prior agent context"),
            "prior context missing: {prompt}"
        );
        assert!(
            prompt.contains("Keep the public API stable."),
            "CLAUDE.md not included: {prompt}"
        );
        assert!(
            prompt.contains("+hi from a prior agent"),
            "uncommitted diff not included: {prompt}"
        );
        assert!(
            prompt.contains("plan.md"),
            "untracked file list not included: {prompt}"
        );
        assert!(
            prompt.contains("hello.txt"),
            "repository map still present: {prompt}"
        );
        assert!(
            prompt.contains("Maximize parallelism"),
            "parallelism instructions missing: {prompt}"
        );
        assert!(
            prompt.rfind("Prior agent context").unwrap() < prompt.find("Repository map").unwrap(),
            "prior context should precede the map"
        );
    }

    #[test]
    fn external_plan_tolerates_fences_and_prose() {
        let db = db();
        let ids = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf 'Here is the plan:\n```json\n{"tasks":[{"key":"a","title":"One","description":"D","depends_on":[]}]}\n```\nDone.\n'
"#,
        )
        .unwrap();
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn external_plan_accepts_a_bare_task_array_and_null_deps() {
        let db = db();
        let ids = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '[{"key":"a","title":"One","description":"D","depends_on":null}]'
"#,
        )
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert!(db.dependencies_of(&ids[0]).unwrap().is_empty());
    }

    #[test]
    fn a_description_missing_from_the_plan_falls_back_to_the_title() {
        let db = db();
        let ids = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[{"key":"a","title":"Only a title"}]}'
"#,
        )
        .unwrap();
        let task = db.require_task(&ids[0]).unwrap();
        assert_eq!(task.description, "Only a title");
    }

    #[test]
    fn external_plan_rejects_cycles_and_writes_nothing() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[
  {"key":"a","title":"A","description":"d","depends_on":["b"]},
  {"key":"b","title":"B","description":"d","depends_on":["a"]}
]}'
"#,
        )
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("cycle"), "{message}");
        assert!(message.contains("a, b"), "cycle members named: {message}");
        assert_eq!(
            db.task_count().unwrap(),
            0,
            "a rejected plan writes nothing"
        );
        assert_eq!(db.dependency_count().unwrap(), 0);
    }

    #[test]
    fn external_plan_rejects_unknown_dependencies() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[{"key":"a","title":"A","description":"d","depends_on":["ghost"]}]}'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unknown task `ghost`"), "{err}");
    }

    #[test]
    fn external_plan_rejects_duplicate_keys() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[{"key":"a","title":"A"},{"key":"a","title":"B"}]}'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate task key"), "{err}");
    }

    #[test]
    fn external_plan_rejects_self_dependencies() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[{"key":"a","title":"A","depends_on":["a"]}]}'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("depends on itself"), "{err}");
    }

    #[test]
    fn external_plan_rejects_an_empty_task_list() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks":[]}'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no tasks"), "{err}");
    }

    #[test]
    fn external_plan_caps_runaway_plans() {
        let db = db();
        let mut tasks = String::new();
        for i in 0..=MAX_PLAN_TASKS {
            if i > 0 {
                tasks.push(',');
            }
            tasks.push_str(&format!(
                "{{\"key\":\"t{i}\",\"title\":\"T{i}\",\"description\":\"d\"}}"
            ));
        }
        let body = format!("#!/bin/sh\ncat > /dev/null\nprintf '%s' '{{\"tasks\":[{tasks}]}}'\n");
        let err = external(&db, &body).unwrap_err();
        assert!(err.to_string().contains("task limit"), "{err}");
        assert_eq!(db.task_count().unwrap(), 0);
    }

    #[test]
    fn external_plan_reports_a_failing_command() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
echo "model unavailable" >&2
exit 3
"#,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("exit status: 3"), "{message}");
        assert!(message.contains("model unavailable"), "{message}");
    }

    #[test]
    fn external_plan_reports_invalid_json() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf 'sorry, I cannot do that'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no JSON"), "{err}");
    }

    #[test]
    fn external_plan_reports_malformed_json() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
printf '%s' '{"tasks": [}'
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not valid JSON"), "{err}");
    }

    #[test]
    fn external_plan_reports_empty_output() {
        let db = db();
        let err = external(
            &db,
            r#"#!/bin/sh
cat > /dev/null
"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("no output"), "{err}");
    }

    #[test]
    fn external_plan_times_out() {
        let (dir, _) = scratch();
        let spec = script(dir.path(), "#!/bin/sh\ncat > /dev/null\nsleep 30\n");
        let planner = Planner::command_with_timeout(&spec, Duration::from_millis(150));

        let started = Instant::now();
        let err = plan_with(&db(), "anything", &planner, dir.path()).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the sleep must be killed, not waited out"
        );
    }

    #[test]
    fn planner_label_names_the_source() {
        assert_eq!(Planner::Mock.label(), "built-in planner");
        assert_eq!(Planner::command("sh -c true").label(), "ZOU_PLANNER_CMD");
    }
}
