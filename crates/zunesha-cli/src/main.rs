//! `zou` – CLI for the local-first AI coding orchestrator.
//!
//! The subcommands walk the same lifecycle the dashboard drives:
//! * `init`    – bootstrap `.zou/` + SQLite state in a git repo.
//! * `plan`    – decompose a feature request into a task DAG.
//! * `work`    – create a task's isolated workspace.
//! * `run`     – execute the agent on every ready task.
//! * `diff`    – inspect what a workspace changed.
//! * `approve` – land reviewed work in the repository (the human gate); on a
//!   conflict, run the Phase 9 auto-resolution agent in its place.
//! * `recover` – release stuck tasks (mid-run/mid-validation/mid-merge) after
//!   a crash.
//! * `status`  – print task counts and installation info.
//! * `start`   – serve the local API, WebSockets, and dashboard.

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use zunesha_core::{
    approve_task, init_project, normalize_model, recover_stuck, AgentCommand, Config, Database,
    Error, MergeOutcome, StreamKind, TaskStatus, WorkerEvent, WorkerPool, WorkspaceManager,
    ZuneshaPaths,
};

#[derive(Parser, Debug)]
#[command(
    name = "zou",
    version,
    about = "Local-first, parallelized AI coding orchestrator",
    long_about = None
)]
struct Cli {
    /// Path to the target project (defaults to the current directory).
    #[arg(long, global = true, value_name = "PATH")]
    path: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize the orchestrator inside a git repository.
    Init {
        /// Skip the git repository check and initialize anywhere.
        #[arg(long)]
        force: bool,
    },
    /// Plan a feature into a DAG of tasks (built-in, or `ZOU_PLANNER_CMD`).
    Plan {
        /// Feature request to plan.
        prompt: String,
    },
    /// Spin up an isolated workspace for a task and mark it IN_PROGRESS.
    Work {
        /// Id of the task (see `zou status`).
        task_id: String,
        /// Mark the task IN_PROGRESS (required for real work; useful for demos).
        #[arg(long)]
        activate: bool,
    },
    /// Show the captured diff of a task's workspace.
    Diff {
        /// Id of the task (see `zou status`).
        task_id: String,
    },
    /// Run the AI agent on all ready (TODO) tasks, in parallel.
    Run {
        /// Cap on simultaneously running agents.
        #[arg(long, default_value_t = zunesha_core::DEFAULT_MAX_CONCURRENCY)]
        jobs: usize,
        /// Also promote newly-ready tasks and run them after the first wave.
        #[arg(long)]
        follow: bool,
    },
    /// Approve reviewed work: merge its diff into the repo, then mark DONE.
    ///
    /// The human-in-the-loop gate, same code path as the dashboard's Approve
    /// button: the approved diff is applied to the working tree first, and a
    /// patch that no longer applies writes nothing — the task stays in REVIEW.
    Approve {
        /// Id of the task in REVIEW (see `zou status`).
        task_id: String,
    },
    /// Release stuck IN_PROGRESS/EVALUATING tasks back to TODO (crash recovery).
    ///
    /// A killed daemon or interrupted `zou run` leaves its tasks claimed
    /// forever; this frees them so they can be re-run or reviewed. Only use
    /// it when the worker is actually dead — a CLI run and the daemon can
    /// coexist, and nothing here can tell a live agent from a dead one.
    Recover {
        /// Release only this task; omit to release every stuck task.
        task_id: Option<String>,
    },
    /// Show orchestrator status and task counts.
    Status,
    /// Choose the OpenCode model used by the agent (saved user-wide).
    ///
    /// With no arguments, lists the models reported by `opencode models` and
    /// prompts for one. The choice is written to
    /// `${XDG_CONFIG_HOME:-~/.config}/zou/config.json`, so it applies to every
    /// project. `ZOU_AGENT_CMD` remains an explicit per-shell override.
    Model {
        /// Save this model id directly instead of listing and prompting.
        model: Option<String>,
        /// List available models without changing the saved setting.
        #[arg(long)]
        list: bool,
        /// Show the saved model and where it is stored.
        #[arg(long)]
        show: bool,
        /// Forget the saved model and return to the built-in agent command.
        #[arg(long)]
        clear: bool,
    },
    /// Start the local API + WebSocket server (and agent pool).
    ///
    /// Serves the dashboard and REST/WebSocket API on localhost until Ctrl-C.
    Start {
        /// Interface to bind.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port to bind (`--port`, else `ZOU_PORT`, else 57804).
        #[arg(long, env = "ZOU_PORT", default_value_t = zunesha_api::DEFAULT_PORT)]
        port: u16,
        /// Cap on simultaneously running agents.
        #[arg(long, default_value_t = zunesha_core::DEFAULT_MAX_CONCURRENCY)]
        jobs: usize,
        /// Directory of the built dashboard (`web/out`). Defaults to auto-
        /// discovery (env `ZOU_DASHBOARD_DIR`, then CWD/exe-relative).
        #[arg(long)]
        dashboard: Option<std::path::PathBuf>,
        /// Do not automatically re-run tasks left claimed by a previous
        /// (crashed) daemon. Only needed when a concurrent `zou run` may own
        /// those claims; a manual `zou recover` still works either way.
        #[arg(long)]
        no_resume: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let target = match &cli.path {
        Some(p) => p.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };

    let result = match cli.command {
        Commands::Init { force } => cmd_init(&target, force),
        Commands::Plan { prompt } => cmd_plan(&target, &prompt),
        Commands::Work { task_id, activate } => cmd_work(&target, &task_id, activate),
        Commands::Diff { task_id } => cmd_diff(&target, &task_id),
        Commands::Run { jobs, follow } => cmd_run(&target, jobs, follow),
        Commands::Approve { task_id } => cmd_approve(&target, &task_id),
        Commands::Recover { task_id } => cmd_recover(&target, task_id.as_deref()),
        Commands::Status => cmd_status(&target),
        Commands::Model {
            model,
            list,
            show,
            clear,
        } => cmd_model(model.as_deref(), list, show, clear),
        Commands::Start {
            host,
            port,
            jobs,
            dashboard,
            no_resume,
        } => cmd_start(&target, &host, port, jobs, dashboard, no_resume),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `zou init` – create `.zou/` and bootstrap the database.
fn cmd_init(target: &Path, force: bool) -> zunesha_core::Result<()> {
    let report = init_project(target, force)?;

    println!("✔ orchestrator initialized");
    println!("  repo root     : {}", report.repo_root.display());
    println!("  control dir   : {}", report.zou_dir.display());
    println!("  database      : {}", report.state_db.display());
    println!("  workspaces    : {}", report.workspaces_dir.display());
    println!(
        "  gitignore     : {}",
        if report.gitignore_updated {
            "updated (.zou/ added)"
        } else {
            "already ignored"
        }
    );
    if report.already_initialized {
        println!("  note          : installation already existed; state preserved");
    }
    println!();
    println!("Next: `zou plan \"<feature>\"` to create tasks, `zou run` to execute them,");
    println!("      `zou start` for the local dashboard.");
    Ok(())
}

/// `zou plan` – run the planner (built-in; `ZOU_PLANNER_CMD` overrides) over a
/// feature prompt.
fn cmd_plan(target: &Path, prompt: &str) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let db = Database::open(&paths.state_db())?;

    let planner = zunesha_core::Planner::from_env();
    let ids = zunesha_core::plan_with(&db, prompt, &planner, paths.root())?;
    if ids.is_empty() {
        println!("nothing to plan: prompt was empty");
        return Ok(());
    }

    println!("planned {} tasks ({}):", ids.len(), planner.label());
    for (i, id) in ids.iter().enumerate() {
        let task = db.require_task(id)?;
        let deps = db.dependencies_of(id)?;
        let depends_on = if deps.is_empty() {
            String::new()
        } else {
            format!("  (depends on: {})", deps.join(", "))
        };
        println!("  {}. [{}] {}{depends_on}", i + 1, task.status, task.title);
    }
    Ok(())
}

/// `zou status` – show installation info, task counts, and the task list.
fn cmd_status(target: &Path) -> zunesha_core::Result<()> {
    let paths = discover(target)?;

    let db = Database::open(&paths.state_db())?;
    let counts = db.task_counts_by_status()?;
    let total = db.task_count()?;
    let deps = db.dependency_count()?;

    println!("orchestrator status");
    println!("  repo root   : {}", paths.root().display());
    println!("  database    : {}", paths.state_db().display());
    println!("  workspaces  : {}", paths.workspaces_dir().display());
    println!();
    println!("tasks: {total} total, {deps} dependency edges");
    for status in TaskStatus::ALL {
        let count = counts
            .iter()
            .find(|(s, _)| *s == status)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        println!("  {status:<12} {count}");
    }

    // Task list, grouped by status in pipeline order.
    if total > 0 {
        println!();
        for status in TaskStatus::ALL {
            for task in db.list_tasks_by_status(status)? {
                println!("  {:<12} {:<24} {}", status, task.id, task.title);
            }
        }
    }
    Ok(())
}

/// `zou model` – list OpenCode models, save the chosen one user-wide.
///
/// The saved model feeds the default agent command:
///
/// ```text
/// opencode run --auto --model <model> {prompt}
/// ```
///
/// `ZOU_AGENT_CMD`, when set, still wins over the saved value.
fn cmd_model(model: Option<&str>, list: bool, show: bool, clear: bool) -> zunesha_core::Result<()> {
    let selected_actions = [model.is_some(), list, show, clear]
        .into_iter()
        .filter(|selected| *selected)
        .count();
    if selected_actions > 1 {
        return Err(Error::Config(
            "choose only one of: a model id, --list, --show, or --clear".into(),
        ));
    }

    if clear {
        let path = Config::clear_model()?;
        println!("✔ cleared the saved model");
        println!("  config : {}", path.display());
        println!("  agent  : opencode run {{prompt}} (built-in default)");
        println!("  apply  : restart `zou start` if it is running");
        return Ok(());
    }

    if show {
        let path = Config::path()?;
        let config = Config::load()?;
        match config.model() {
            Some(model) => {
                println!("{model}");
                println!("  config : {}", path.display());
                println!("  agent  : opencode run --auto --model {model} {{prompt}}");
            }
            None => {
                println!("no saved model (using `opencode run {{prompt}}`)");
                println!("  config : {}", path.display());
            }
        }
        return Ok(());
    }

    // A direct save is also the recovery path for a corrupt hand-edited
    // config, so do it before trying to read the current setting.
    if let Some(model) = model {
        return save_model(model);
    }

    let current = match Config::load() {
        Ok(config) => config.model().map(str::to_string),
        Err(e) => {
            eprintln!("warning: {e}; choosing a model will replace the config");
            None
        }
    };

    let models = available_models()?;
    if models.is_empty() {
        return Err(Error::Config(
            "`opencode models` returned no models; check `opencode auth` / your provider setup"
                .into(),
        ));
    }

    print_models(&models, current.as_deref());
    if list {
        return Ok(());
    }

    let Some(choice) = choose_model(&models)? else {
        println!("cancelled; no change");
        return Ok(());
    };
    save_model(&choice)
}

/// Persist a model and explain when it takes effect.
fn save_model(model: &str) -> zunesha_core::Result<()> {
    let model = normalize_model(model)?;
    let path = Config::set_model(&model)?;

    println!("✔ saved model {model}");
    println!("  config : {}", path.display());
    println!("  agent  : opencode run --auto --model {model} {{prompt}}");
    if std::env::var("ZOU_AGENT_CMD").is_ok_and(|value| !value.trim().is_empty()) {
        println!(
            "  note   : ZOU_AGENT_CMD is set and overrides this saved model; unset it to use the selection"
        );
    }
    println!("  apply  : new `zou run` / `zou start` processes; restart a running daemon");
    Ok(())
}

/// Ask `opencode models` for the installed/provider model list.
fn available_models() -> zunesha_core::Result<Vec<String>> {
    let output = std::process::Command::new("opencode")
        .arg("models")
        .output()
        .map_err(|e| {
            Error::Config(format!(
                "could not run `opencode models`: {e}. Install OpenCode or use ZOU_AGENT_CMD."
            ))
        })?;

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        let detail = if detail.is_empty() {
            format!("exit status {}", output.status)
        } else {
            detail.lines().next().unwrap_or(detail).to_string()
        };
        return Err(Error::Config(format!("`opencode models` failed: {detail}")));
    }

    Ok(parse_models(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse the newline-separated ids printed by `opencode models`.
///
/// The real CLI prints one `provider/model` id per line. Keep only
/// single-token lines containing `/`, and de-duplicate without reordering.
fn parse_models(output: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && line.contains('/') && !line.chars().any(char::is_whitespace)
        })
        .filter(|line| seen.insert((*line).to_string()))
        .map(str::to_string)
        .collect()
}

/// Print the numbered model list, marking the saved selection.
fn print_models(models: &[String], current: Option<&str>) {
    println!("Available models (`opencode models`):");
    for (index, model) in models.iter().enumerate() {
        let marker = if current == Some(model.as_str()) {
            "*"
        } else {
            " "
        };
        println!(" {marker} {:>2}. {model}", index + 1);
    }
    match (current, models.iter().any(|m| Some(m.as_str()) == current)) {
        (Some(current), false) => {
            println!("   saved model {current} is not in the current list");
        }
        (Some(_), true) => {}
        (None, _) => println!("   no model saved yet"),
    }
    println!();
}

/// Prompt for a list number or a model id. `None` means the user cancelled.
fn choose_model(models: &[String]) -> zunesha_core::Result<Option<String>> {
    loop {
        print!("Select a model by number or id (empty to cancel): ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Ok(None); // EOF: non-interactive input ended
        }
        let choice = input.trim();
        if choice.is_empty() {
            return Ok(None);
        }

        if let Ok(index) = choice.parse::<usize>() {
            if (1..=models.len()).contains(&index) {
                return Ok(Some(models[index - 1].clone()));
            }
            eprintln!(
                "  invalid selection: choose a number from 1 to {}",
                models.len()
            );
            continue;
        }

        match normalize_model(choice) {
            Ok(model) => return Ok(Some(model)),
            Err(e) => eprintln!("  {e}"),
        }
    }
}

/// `zou start` – run the local API + WebSocket server until interrupted.
///
/// A functioning local API bridging the Rust state to the outside world: It serves:
/// * REST endpoints (`/api/...`) for the dashboard to read/mutate task state,
/// * a WebSocket (`/ws`) streaming task updates and live agent logs,
/// * the dashboard's static files (or a placeholder page) at `/`.
///
/// The server owns a [`WorkerPool`], so `POST /api/run` can start agents in
/// parallel (capped at `--jobs`). Graceful shutdown on Ctrl-C lets in-flight
/// database writes finish.
fn cmd_start(
    target: &Path,
    host: &str,
    port: u16,
    jobs: usize,
    dashboard: Option<std::path::PathBuf>,
    no_resume: bool,
) -> zunesha_core::Result<()> {
    let paths = discover(target)?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Workspace(format!("failed to build async runtime: {e}")))?;

    rt.block_on(async move {
        let config = zunesha_api::ServeConfig {
            paths: paths.clone(),
            host: host.to_string(),
            port,
            max_concurrency: jobs,
            agent: AgentCommand::default(),
            dashboard_dir: dashboard,
            resume: !no_resume,
        };

        let handle = zunesha_api::serve(config)
            .await
            .map_err(|e| Error::Workspace(format!("failed to start on {host}:{port}: {e}")))?;

        println!("zou daemon listening on http://{}", handle.address());
        println!("  REST  → http://{}/api/tasks", handle.address());
        println!("  WS    → ws://{}/ws", handle.address());
        // Phase 11: a restarted daemon picks its crashed run back up.
        let resumed = handle.resumed();
        if !resumed.is_empty() {
            println!(
                "  resumed {} interrupted task(s): {}",
                resumed.len(),
                resumed.join(", ")
            );
        }
        println!("  press Ctrl-C to stop");

        // Wait for Ctrl-C, then shut down gracefully.
        shutdown_signal().await;
        println!("\nshutting down…");
        // Dropping the handle detaches the server task; the runtime is torn
        // down when `block_on` returns, closing the listener.
        Ok(())
    })
}

/// Resolve on SIGINT (Ctrl-C) or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// `zou work` – create an isolated workspace for a task and activate it.
fn cmd_work(target: &Path, task_id: &str, activate: bool) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let db = Database::open(&paths.state_db())?;
    let mgr = WorkspaceManager::new(paths.clone());

    // `activate` transitions TODO -> IN_PROGRESS, which is what the daemon
    // will do automatically in Phase 4. Kept explicit here so the CLI can
    // also inspect an already-active workspace.
    if activate {
        let task = db.require_task(task_id)?;
        if task.status == zunesha_core::TaskStatus::Todo {
            db.set_status(task_id, zunesha_core::TaskStatus::InProgress)?;
        }
    }

    let record = mgr.create(&db, task_id)?;
    println!("✔ workspace ready for task {task_id}");
    println!("  directory : {}", record.path);
    println!("  branch    : {}", record.branch);
    println!(
        "  baseline  : {}",
        &record.base_commit[..12.min(record.base_commit.len())]
    );
    Ok(())
}

/// `zou diff` – print the diff captured from a task's workspace.
fn cmd_diff(target: &Path, task_id: &str) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let db = Database::open(&paths.state_db())?;
    let mgr = WorkspaceManager::new(paths);

    // Friendly message (not an error) when the task never got a workspace.
    if db.get_workspace(task_id)?.is_none() {
        println!("task {task_id} has no workspace yet (run `zou work {task_id}` first)");
        return Ok(());
    }

    let diff = mgr.generate_diff(&db, task_id)?;
    if diff.is_empty() {
        println!("no changes in workspace for task {task_id}");
    } else {
        print!("{diff}");
    }
    Ok(())
}

/// `zou run` – execute the agent on every ready (TODO) task, in parallel.
///
/// A bounded worker pool spawns isolated agent subprocesses, streams their
/// output live to this terminal, and lands each finished task in `REVIEW`
/// (or back to `TODO` on failure). With `--follow`, after each wave the DAG
/// engine re-promotes tasks whose prerequisites are now `DONE` and runs those
/// too — chained tasks wait for human approval, so the loop exits as soon as
/// nothing new is ready.
fn cmd_run(target: &Path, jobs: usize, follow: bool) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let pool = WorkerPool::new(paths.clone(), jobs, AgentCommand::default());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Workspace(format!("failed to build async runtime: {e}")))?;

    rt.block_on(async move {
        let mut rx = pool.subscribe();
        let mut total_started = 0usize;
        let mut total_ok = 0usize;

        // Live-relay agent output to the terminal as it happens.
        let relay = tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let WorkerEvent::Log {
                    task_id,
                    stream,
                    line,
                    seq: _,
                } = ev
                {
                    let tag = match stream {
                        StreamKind::Stdout => "out",
                        StreamKind::Stderr => "err",
                    };
                    println!("[{tag}] {task_id}: {line}");
                }
            }
        });

        loop {
            // Run one wave of ready tasks.
            let started_ids = pool.run_ready().await?;
            if started_ids.is_empty() && total_started == 0 {
                println!("no ready (TODO) tasks to run");
                break;
            }
            total_started += started_ids.len();

            // Wait for *this wave only*: drain the ids we claimed. Waiting on
            // the global IN_PROGRESS count would hang forever if some other
            // runner (the daemon, another CLI) holds a task — or if a crashed
            // worker left one stuck (see `zou recover`). A task is in flight
            // while a worker owns it: IN_PROGRESS (agent) *or* EVALUATING
            // (validation) — exiting early would kill the runtime out from
            // under the self-healing loop and strand the task.
            loop {
                let db = Database::open(&paths.state_db())?;
                let in_flight = started_ids
                    .iter()
                    .filter(|id| {
                        db.get_task(id).ok().flatten().is_some_and(|t| {
                            matches!(t.status, TaskStatus::InProgress | TaskStatus::Evaluating)
                        })
                    })
                    .count();
                if in_flight == 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            let db = Database::open(&paths.state_db())?;
            let review = db.list_tasks_by_status(TaskStatus::Review)?.len();
            total_ok = review;

            if !follow {
                break;
            }
            // Phase 2 engine: unlock whatever the completed wave unblocked.
            let promoted = zunesha_core::promote_ready(&db)?;
            if promoted.is_empty() {
                break;
            }
            println!("— promoted {} newly-ready task(s)", promoted.len());
        }

        relay.abort();

        println!();
        println!("ran {total_started} task(s); {total_ok} now in REVIEW awaiting approval");
        Ok(())
    })
}

/// `zou approve` – land a reviewed task's diff, then mark it DONE.
///
/// Same code path as the dashboard's Approve button: integration runs while
/// the task is still in `REVIEW`, so a merge conflict aborts with nothing
/// written and the task left reviewable.
///
/// Phase 9: an ordinary conflict doesn't stop here. The workspace is rebased
/// onto the repository's current content (conflict markers in place), the
/// task moves to `RESOLVING_CONFLICTS`, and this command drives the conflict
/// resolution agent in that workspace to completion — streaming its output
/// like `zou run` does — so the reconciled diff comes back to `REVIEW`
/// ready for a second, clean approval. Only if the rebase itself fails does
/// the old `merge conflict` error surface (nothing written, task still
/// reviewable).
fn cmd_approve(target: &Path, task_id: &str) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let outcome = approve_task(&paths, task_id)?;

    match &outcome.integration {
        MergeOutcome::NoChanges => println!("✔ approved task {task_id} (nothing to merge)"),
        MergeOutcome::Applied { files } => {
            println!("✔ approved task {task_id}");
            println!(
                "  merged    : {} file(s) into {}",
                files.len(),
                paths.root().display()
            );
            for file in files {
                println!("    - {file}");
            }
        }
        MergeOutcome::ConflictResolutionQueued { files } => {
            println!("⚠ merge conflict in {} — auto-resolving", files.join(", "));
            return run_resolution(&paths, task_id);
        }
    }
    if outcome.promoted.is_empty() {
        println!("  promoted  : none");
    } else {
        println!("  promoted  : {}", outcome.promoted.join(", "));
    }
    Ok(())
}

/// Drive a Phase 9 conflict resolution to completion (used by `zou approve`).
///
/// Streams the agent's output live, then reports where the task landed:
/// `REVIEW` means the conflict was reconciled and `zou approve` can land the
/// now-clean diff; `FAILED_NEEDS_HUMAN` means the overlap needs a person.
fn run_resolution(paths: &ZuneshaPaths, task_id: &str) -> zunesha_core::Result<()> {
    let pool = WorkerPool::new(paths.clone(), 1, AgentCommand::default());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Workspace(format!("failed to build async runtime: {e}")))?;

    rt.block_on(async move {
        let mut rx = pool.subscribe();
        let relay = tokio::spawn(async move {
            while let Ok(ev) = rx.recv().await {
                if let WorkerEvent::Log {
                    task_id,
                    stream,
                    line,
                    seq: _,
                } = ev
                {
                    let tag = match stream {
                        StreamKind::Stdout => "out",
                        StreamKind::Stderr => "err",
                    };
                    println!("[{tag}] {task_id}: {line}");
                }
            }
        });

        let handle = pool.spawn_resolution(task_id.to_string()).await?;
        let resolved = handle
            .await
            .map_err(|e| Error::Workspace(format!("resolution task panicked: {e}")))??;
        relay.abort();

        let db = Database::open(&paths.state_db())?;
        let status = db.require_task(task_id)?.status;
        match (resolved, status) {
            (true, TaskStatus::Review) => {
                println!("✔ conflict resolved; task {task_id} is back in REVIEW");
                println!("  next       : `zou approve {task_id}` to land the reconciled diff");
            }
            (_, TaskStatus::FailedNeedsHuman) => {
                println!("✖ conflict could not be resolved automatically: {task_id} needs a human");
                println!("  inspect : `zou diff {task_id}` (the workspace still holds the markers)");
                println!("  fix     : edit the workspace, then re-run `zou run` / approve from the dashboard");
            }
            (_, other) => println!("— resolution finished; task {task_id} is in {other}"),
        }
        Ok(())
    })
}

/// `zou recover` – release stuck IN_PROGRESS/EVALUATING/RESOLVING_CONFLICTS
/// tasks back to TODO.
///
/// Crash recovery: a killed daemon or interrupted run leaves tasks claimed
/// (mid-agent in `IN_PROGRESS`, mid-validation in `EVALUATING`, or mid-merge
/// in `RESOLVING_CONFLICTS`) forever. The workspace is kept, so the retry
/// builds on whatever the crashed agent already produced.
fn cmd_recover(target: &Path, task_id: Option<&str>) -> zunesha_core::Result<()> {
    let paths = discover(target)?;
    let db = Database::open(&paths.state_db())?;
    let released = recover_stuck(&db, task_id)?;

    if released.is_empty() {
        println!(
            "✔ nothing to recover: no task is stuck IN_PROGRESS/EVALUATING/RESOLVING_CONFLICTS"
        );
        return Ok(());
    }
    for id in &released {
        println!("✔ released task {id} (→ TODO), workspace kept");
    }
    println!("  next       : `zou run` to retry");
    Ok(())
}

/// Locate an existing orchestrator installation, or error helpfully.
fn discover(target: &Path) -> zunesha_core::Result<ZuneshaPaths> {
    ZuneshaPaths::discover(target).ok_or(Error::NotInitialized {
        path: target.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::parse_models;

    #[test]
    fn parse_models_keeps_ids_and_discards_noise() {
        let output = "\nopencode/beam-free\nopencode/big-pickle\n\
                      not a model\nprovider/model#variant\nopencode/beam-free\n";
        assert_eq!(
            parse_models(output),
            vec![
                "opencode/beam-free".to_string(),
                "opencode/big-pickle".to_string(),
                "provider/model#variant".to_string(),
            ]
        );
    }
}
