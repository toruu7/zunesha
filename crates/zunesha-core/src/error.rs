//! Error type shared by the whole core crate.

use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// All the ways core operations can fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The target path is not inside a git work tree.
    #[error(
        "not inside a git repository: {} is not within a git work tree (use --force to initialize anyway)",
        path.display()
    )]
    NotAGitRepository { path: PathBuf },

    /// The orchestrator has not been initialized in this project yet.
    #[error(
        "orchestrator is not initialized in {} (run `zou init` first)",
        path.display()
    )]
    NotInitialized { path: PathBuf },

    /// The given path does not exist on disk.
    #[error("path does not exist: {}", path.display())]
    PathNotFound { path: PathBuf },

    /// A database operation failed.
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// An IO operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A `git` subprocess failed.
    #[error("git command failed: {0}")]
    Git(String),

    /// A task status string from the database was not recognized.
    #[error("unknown task status in database: {0}")]
    UnknownTaskStatus(String),

    /// A referenced task does not exist.
    #[error("task not found: {0}")]
    TaskNotFound(String),

    /// A dependency edge would have created a cycle in the DAG.
    #[error("dependency `{task_id}` -> `{depends_on_id}` would create a cycle in the task DAG")]
    CycleDetected {
        task_id: String,
        depends_on_id: String,
    },

    /// The planner failed: a bad command, a timeout, or an invalid plan.
    #[error("planner error: {0}")]
    Planner(String),

    /// A workspace operation failed (invalid state, missing directory, ...).
    #[error("workspace error: {0}")]
    Workspace(String),

    /// A status change violated the task lifecycle policy.
    #[error("invalid status transition for task {task_id}: {from} -> {to}")]
    InvalidTransition {
        task_id: String,
        from: crate::task::TaskStatus,
        to: crate::task::TaskStatus,
    },

    /// The approved diff could not be applied to the repository.
    ///
    /// Raised by [`crate::integrator`] when the working tree has changed since
    /// the task's workspace was snapshotted. Nothing is written and the task
    /// stays in `REVIEW`: the human reconciles the changes (or rejects with
    /// feedback) and approves again.
    #[error("{}", conflict_message(.task_id, .files, .detail))]
    MergeConflict {
        task_id: String,
        /// Files git blamed for the failure (`git apply --check` output).
        files: Vec<String>,
        /// Raw stderr from git, kept for the CLI's diagnostics.
        detail: String,
    },
}

/// Render [`Error::MergeConflict`] as an actionable, single-paragraph message.
///
/// The API puts this straight into a `409` body and the dashboard shows it in
/// the review modal, so it has to say both *what* failed and *what to do next*
/// without reading like a stack trace.
fn conflict_message(task_id: &str, files: &[String], detail: &str) -> String {
    let list = if files.is_empty() {
        "(see git output)".to_string()
    } else {
        files.join(", ")
    };
    let mut msg = format!(
        "merge conflict approving {task_id}: the approved diff does not apply cleanly (files: {list}). \
         Your repository changed after this task's workspace was created — reconcile the changes \
         (or apply the diff by hand) and approve again, or reject with feedback. The task stays in REVIEW."
    );
    if let Some(first) = detail.lines().map(str::trim).find(|l| !l.is_empty()) {
        msg.push_str(&format!(" git said: {first}"));
    }
    msg
}
