//! The wire protocol the dashboard consumes.
//!
//! Every message the API sends to the web UI — over REST responses *and* the
//! WebSocket — is an [`ApiEvent`]. This keeps the "full state" and the "live
//! stream" speaking the same language: a client that missed a few events just
//! waits for the next `Snapshot`, and a client that connects mid-run gets a
//! `Snapshot` immediately so it never renders an empty board.

use serde::{Deserialize, Serialize};

use zunesha_core::{Database, StreamKind, Task, TaskStatus, ZuneshaPaths};

/// The repository this daemon is working on.
///
/// The dashboard is a single-project view, so every snapshot and REST response
/// carries this identity. `root` is canonicalized when possible, which keeps
/// relative `zou --path .` invocations from displaying a bare ".".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Final path component, e.g. `saas-ops-hub`.
    pub name: String,
    /// Absolute (or best-effort) repository root.
    pub root: String,
}

/// Build the display identity for an installation.
pub fn project_info(paths: &ZuneshaPaths) -> ProjectInfo {
    let root = std::fs::canonicalize(paths.root()).unwrap_or_else(|_| paths.root().to_path_buf());
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project")
        .to_string();
    ProjectInfo {
        name,
        root: root.display().to_string(),
    }
}

/// Per-status task count, zero-filled so the UI always shows all seven columns.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusCount {
    pub status: TaskStatus,
    pub count: usize,
}

/// One edge of the task DAG: `task_id` may start only once `depends_on` is
/// `DONE`. Created with the plan and never mutated afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEdge {
    pub task_id: String,
    pub depends_on: String,
}

/// A single message pushed to the dashboard.
///
/// Tagged by `type` so the frontend can `switch` on it without ambiguity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiEvent {
    /// The entire board state: every task, the task DAG, and per-status
    /// counts.
    ///
    /// Sent once when a WebSocket connects and again after every human-driven
    /// mutation (plan / approve / reject). Clients can treat it as a hard
    /// resynchronisation point.
    Snapshot {
        /// Which repository this daemon serves.
        project: ProjectInfo,
        tasks: Vec<Task>,
        /// Dependency edges (`task_id` ← `depends_on`); immutable after
        /// planning, so they only change with the plan itself.
        dependencies: Vec<DependencyEdge>,
        counts: Vec<StatusCount>,
    },

    /// One task changed (typically because a worker touched it).
    TaskUpdated { task: Task },

    /// A live line of agent output. Not persisted here — the worker already
    /// writes it to `worker_logs`; this is purely for streaming.
    Log {
        task_id: String,
        stream: StreamKind,
        line: String,
        /// 0-based line index within the run (see
        /// `zunesha_core::WorkerEvent::Log`), so clients can merge
        /// streams with snapshots without duplicating lines.
        seq: usize,
    },
}

/// Count tasks per status, always emitting all seven statuses in pipeline order
/// (including zeros) so the dashboard's columns are stable.
pub fn zero_filled_counts(db: &Database) -> zunesha_core::Result<Vec<StatusCount>> {
    let raw = db.task_counts_by_status()?;
    Ok(TaskStatus::ALL
        .iter()
        .map(|status| {
            let count = raw
                .iter()
                .find(|(s, _)| s == status)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            StatusCount {
                status: *status,
                count,
            }
        })
        .collect())
}

/// Read every dependency edge from the database (`task_id` ← `depends_on`).
pub fn dependency_edges(db: &Database) -> zunesha_core::Result<Vec<DependencyEdge>> {
    Ok(db
        .list_dependencies()?
        .into_iter()
        .map(|(task_id, depends_on)| DependencyEdge {
            task_id,
            depends_on,
        })
        .collect())
}

/// Build the authoritative `Snapshot` for the current database state.
pub fn snapshot_event(db: &Database, paths: &ZuneshaPaths) -> zunesha_core::Result<ApiEvent> {
    let tasks = db.list_tasks()?;
    let dependencies = dependency_edges(db)?;
    let counts = zero_filled_counts(db)?;
    Ok(ApiEvent::Snapshot {
        project: project_info(paths),
        tasks,
        dependencies,
        counts,
    })
}

/// Bridge the worker pool's internal events into the API's broadcast channel.
///
/// Phase 4's [`zunesha_core::WorkerPool`] emits fine-grained
/// [`zunesha_core::WorkerEvent`]s on its own channel. The API rebroadcasts
/// them as [`ApiEvent`]s so WebSocket clients only ever speak one protocol.
///
/// * `Log` passes straight through.
/// * `StatusChanged` / `TaskFinished` cause a re-read from the database, so the
///   emitted `TaskUpdated` always carries the authoritative row (status, logs,
///   timestamps) rather than a possibly-stale in-memory copy.
///
/// `Lagged` receivers are skipped rather than fatal: if a client can't keep up
/// the next `Snapshot` will correct it, and dropping one task update should
/// never tear down the bridge.
pub fn spawn_event_bridge(
    worker_rx: tokio::sync::broadcast::Receiver<zunesha_core::WorkerEvent>,
    db_path: std::path::PathBuf,
    tx: tokio::sync::broadcast::Sender<ApiEvent>,
) {
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;

        let mut rx = worker_rx;
        loop {
            match rx.recv().await {
                Ok(zunesha_core::WorkerEvent::Log {
                    task_id,
                    stream,
                    line,
                    seq,
                }) => {
                    let _ = tx.send(ApiEvent::Log {
                        task_id,
                        stream,
                        line,
                        seq,
                    });
                }
                Ok(
                    worker_event @ (zunesha_core::WorkerEvent::StatusChanged { .. }
                    | zunesha_core::WorkerEvent::TaskFinished { .. }),
                ) => {
                    let task_id = match worker_event {
                        zunesha_core::WorkerEvent::StatusChanged { task_id, .. }
                        | zunesha_core::WorkerEvent::TaskFinished { task_id, .. } => task_id,
                        _ => unreachable!("matched above"),
                    };
                    // Best-effort: a transient DB error shouldn't kill the
                    // bridge, so we skip this update and let the next event or
                    // Snapshot repair the client.
                    if let Ok(db) = Database::open(&db_path) {
                        if let Ok(Some(task)) = db.get_task(&task_id) {
                            let _ = tx.send(ApiEvent::TaskUpdated { task });
                        }
                    }
                }
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            }
        }
    });
}
