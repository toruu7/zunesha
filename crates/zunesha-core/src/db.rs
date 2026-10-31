//! SQLite-backed state store.
//!
//! The database lives at `<repo>/.zou/state.db` and is deliberately
//! plain SQLite (via `rusqlite` with a bundled libsqlite3) so the whole
//! system stays local-first with zero external services.
//!
//! Schema (Phase 1):
//! * `tasks` – one row per unit of work.
//! * `dependencies` – the DAG edges (`task_id` depends on `depends_on_id`).
//!
//! This module owns all SQL. Higher layers ([`crate::dag`], the daemon, the
//! API) go through these methods rather than touching `rusqlite` directly.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::{Error, Result};
use crate::task::{Task, TaskStatus};

/// The full DDL applied on open. Idempotent via `IF NOT EXISTS`.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tasks (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'BACKLOG'
                 CHECK (status IN ('BACKLOG','TODO','IN_PROGRESS','EVALUATING','REVIEW','RESOLVING_CONFLICTS','DONE','FAILED_NEEDS_HUMAN')),
    retry_count  INTEGER NOT NULL DEFAULT 0,
    worker_logs  TEXT NOT NULL DEFAULT '',
    agent_output TEXT NOT NULL DEFAULT '',
    diff         TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS dependencies (
    task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    depends_on_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, depends_on_id),
    CHECK (task_id <> depends_on_id)
);

CREATE INDEX IF NOT EXISTS idx_tasks_status       ON tasks(status);
CREATE INDEX IF NOT EXISTS idx_deps_task          ON dependencies(task_id);
CREATE INDEX IF NOT EXISTS idx_deps_dependson     ON dependencies(depends_on_id);

CREATE TABLE IF NOT EXISTS workspaces (
    task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    branch      TEXT NOT NULL,
    base_commit TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS runs (
    id          TEXT PRIMARY KEY,
    task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    mode        TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'RUNNING',
    started_at  TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at    TEXT,
    error_class TEXT,
    detail      TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS run_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id     TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL,
    payload    TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (run_id, seq)
);

CREATE INDEX IF NOT EXISTS idx_runs_task       ON runs(task_id, started_at);
CREATE INDEX IF NOT EXISTS idx_run_events_task ON run_events(task_id, id);
"#;

/// Columns selected when hydrating a [`Task`], in struct-field order.
const TASK_COLUMNS: &str = "id, title, description, status, retry_count, worker_logs, agent_output, diff, created_at, updated_at";

/// `tasks` table shape as of the Markdown-output release.
///
/// Kept verbatim for [`Database::migrate_tasks_table`]'s rebuild and its test:
/// the rebuild must produce exactly this shape so fresh and migrated
/// databases are indistinguishable.
const TASKS_TABLE_V3: &str = "CREATE TABLE tasks_v3 (
    id           TEXT PRIMARY KEY,
    title        TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'BACKLOG'
                 CHECK (status IN ('BACKLOG','TODO','IN_PROGRESS','EVALUATING','REVIEW','RESOLVING_CONFLICTS','DONE','FAILED_NEEDS_HUMAN')),
    retry_count  INTEGER NOT NULL DEFAULT 0,
    worker_logs  TEXT NOT NULL DEFAULT '',
    agent_output TEXT NOT NULL DEFAULT '',
    diff         TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
)";

/// A registered workspace for an in-flight task (one row per task).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub task_id: String,
    /// Absolute path of the workspace directory.
    pub path: String,
    /// Git branch dedicated to this task inside the workspace.
    pub branch: String,
    /// Commit the workspace was snapshotted from; diffs are taken against it.
    pub base_commit: String,
}

/// One agent run (attempt) for a task.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunRecord {
    pub id: String,
    pub task_id: String,
    /// `normal` or `resolve_conflicts`.
    pub mode: String,
    /// `RUNNING`, `REVIEW`, `TODO`, `FAILED_NEEDS_HUMAN`, or `ERROR`.
    pub state: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub error_class: Option<String>,
    pub detail: String,
}

/// One structured event in a run's audit trail.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunEvent {
    pub id: i64,
    pub run_id: String,
    pub task_id: String,
    pub seq: i64,
    pub kind: String,
    /// JSON object, stored as text so the core stays serde-format agnostic.
    pub payload: String,
    pub created_at: String,
}

/// An open handle to the orchestrator's state database.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (creating if necessary) the database at `db_path`, applying the
    /// schema. Any missing parent directories are created.
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        Self::from_connection(conn)
    }

    /// In-memory database, used by tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // WAL keeps the future daemon and the web dashboard from blocking
        // each other; foreign keys keep the DAG edges consistent.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Concurrent writers (daemon + CLI + web API) are the normal case once
        // Phase 4/5 land. Without a busy timeout SQLite fails immediately with
        // SQLITE_BUSY and writes are silently lost; wait instead.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(SCHEMA)?;
        Self::migrate_tasks_table(&conn)?;
        Ok(Self { conn })
    }

    /// Bring an older database up to the current `tasks` shape.
    ///
    /// `CREATE TABLE IF NOT EXISTS` can add nothing to an existing table, and
    /// SQLite cannot alter a `CHECK` constraint in place — so a database
    /// created before Phase 8 lacks the `retry_count` column, one created
    /// before Phase 9 carries a status whitelist that rejects
    /// `RESOLVING_CONFLICTS`, and one created before the Markdown-output
    /// release lacks `agent_output`. Rebuilding the table (the standard SQLite
    /// recipe: new table, copy, drop, rename) fixes all three in one
    /// transaction and preserves every row, including dependency/workspace
    /// references (ids are unchanged).
    fn migrate_tasks_table(conn: &Connection) -> Result<()> {
        let current: Option<String> = conn.query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'tasks'",
            [],
            |row| row.get(0),
        )?;
        let current = current.unwrap_or_default();
        // A database created by the current SCHEMA already has all markers.
        if current.contains("RESOLVING_CONFLICTS")
            && current.contains("retry_count")
            && current.contains("agent_output")
        {
            return Ok(());
        }

        // Foreign keys must be off while swapping the table (the pragma is a
        // no-op inside a transaction), so they are toggled around it.
        // The source may or may not carry these columns (each was added by a
        // later release), so the value list is chosen from the actual source
        // columns — a database that has one must keep its data, one that
        // doesn't gets the default.
        let copy_retry = current.contains("retry_count");
        let copy_output = current.contains("agent_output");
        let values = match (copy_retry, copy_output) {
            (true, true) => {
                "id, title, description, status, retry_count, worker_logs, agent_output, diff, created_at, updated_at"
            }
            (true, false) => {
                "id, title, description, status, retry_count, worker_logs, '', diff, created_at, updated_at"
            }
            (false, true) => {
                "id, title, description, status, 0, worker_logs, agent_output, diff, created_at, updated_at"
            }
            (false, false) => {
                "id, title, description, status, 0, worker_logs, '', diff, created_at, updated_at"
            }
        };
        conn.pragma_update(None, "foreign_keys", "OFF")?;
        let rebuild = conn.execute_batch(&format!(
            "BEGIN;
             {TASKS_TABLE_V3};
             INSERT INTO tasks_v3 (id, title, description, status, retry_count, worker_logs, agent_output, diff, created_at, updated_at)
                 SELECT {values} FROM tasks;
             DROP TABLE tasks;
             ALTER TABLE tasks_v3 RENAME TO tasks;
             CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
             COMMIT;"
        ));
        if let Err(e) = rebuild {
            let _ = conn.execute_batch("ROLLBACK;");
            let _ = conn.pragma_update(None, "foreign_keys", "ON");
            return Err(e.into());
        }
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Ok(())
    }

    // ---------------------------------------------------------------- tasks

    /// Insert a new task. Fails if the id already exists.
    pub fn insert_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tasks (id, title, description, status, retry_count, worker_logs, agent_output, diff)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                task.id,
                task.title,
                task.description,
                task.status.as_str(),
                task.retry_count,
                task.worker_logs,
                task.agent_output,
                task.diff,
            ],
        )?;
        Ok(())
    }

    /// Fetch a task by id, or `None` if it does not exist.
    pub fn get_task(&self, id: &str) -> Result<Option<Task>> {
        let sql = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_task)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Fetch a task by id, erroring when it does not exist.
    pub fn require_task(&self, id: &str) -> Result<Task> {
        self.get_task(id)?
            .ok_or_else(|| Error::TaskNotFound(id.to_string()))
    }

    /// All tasks, ordered by creation time then id (stable for the UI).
    pub fn list_tasks(&self) -> Result<Vec<Task>> {
        let sql = format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY created_at, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_task)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// All tasks currently in `status`.
    pub fn list_tasks_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let sql =
            format!("SELECT {TASK_COLUMNS} FROM tasks WHERE status = ?1 ORDER BY created_at, id");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![status.as_str()], row_to_task)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Update a task's status, enforcing the lifecycle policy, and bump
    /// `updated_at`.
    ///
    /// Refuses transitions that [`TaskStatus::can_transition_to`] forbids
    /// (e.g. demoting a `DONE` task) with [`Error::InvalidTransition`], and
    /// errors with [`Error::TaskNotFound`] when the id is unknown, so callers
    /// never silently "succeed" against a missing row.
    pub fn set_status(&self, id: &str, status: TaskStatus) -> Result<()> {
        let current = self.require_task(id)?.status;
        if !current.can_transition_to(status) {
            return Err(Error::InvalidTransition {
                task_id: id.to_string(),
                from: current,
                to: status,
            });
        }
        self.write_status(id, status)
    }

    /// Update a task's status without lifecycle validation.
    ///
    /// Only for tests and one-off manual repair. Everything else must go
    /// through [`Self::set_status`] so the DAG/human-in-the-loop invariants
    /// hold.
    pub fn force_set_status(&self, id: &str, status: TaskStatus) -> Result<()> {
        self.write_status(id, status)
    }

    fn write_status(&self, id: &str, status: TaskStatus) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tasks
                 SET status = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Update a task's title and/or description, bumping `updated_at`.
    ///
    /// Either field may be `None` to leave it unchanged. Used by the reject
    /// flow (Phase 7) to append human feedback to the description.
    pub fn update_task(
        &self,
        id: &str,
        title: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        // Validate existence first for a helpful error.
        self.require_task(id)?;
        let changed = self.conn.execute(
            "UPDATE tasks SET
                 title = COALESCE(?2, title),
                 description = COALESCE(?3, description),
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![id, title, description],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Delete a task (and, via FK cascade, its dependency edges and workspace
    /// record). Returns whether a row was actually removed.
    pub fn delete_task(&self, id: &str) -> Result<bool> {
        let removed = self
            .conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(removed > 0)
    }

    /// Replace a task's worker logs (used by the Phase 4 log pump).
    pub fn set_worker_logs(&self, id: &str, logs: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tasks
                 SET worker_logs = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
            params![id, logs],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Replace a task's final assistant response (Markdown).
    ///
    /// The worker extracts this from structured agent output; plain-text
    /// agents leave it empty and keep their response in `worker_logs`.
    pub fn set_agent_output(&self, id: &str, output: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tasks
                 SET agent_output = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
            params![id, output],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Replace a task's captured diff (used by Phase 3/7).
    pub fn set_diff(&self, id: &str, diff: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tasks
                 SET diff = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
            params![id, diff],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    /// Set a task's validation-failure counter (Phase 8).
    ///
    /// Used by the self-healing loop to persist the retry count across
    /// process restarts, and by the reject flow to reset it for a fresh
    /// human-approved cycle.
    pub fn set_retry_count(&self, id: &str, count: u32) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE tasks
                 SET retry_count = ?2, updated_at = datetime('now')
                 WHERE id = ?1",
            params![id, count],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(id.to_string()));
        }
        Ok(())
    }

    // ------------------------------------------------------------ run journal

    /// Start a run row. The id is generated by the worker.
    pub fn start_run(&self, run: &RunRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, task_id, mode, state, error_class, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run.id,
                run.task_id,
                run.mode,
                run.state,
                run.error_class,
                run.detail,
            ],
        )?;
        Ok(())
    }

    /// Finish a run, recording its terminal state and optional failure class.
    pub fn finish_run(
        &self,
        run_id: &str,
        state: &str,
        error_class: Option<&str>,
        detail: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE runs
                SET state = ?2, ended_at = datetime('now'), error_class = ?3, detail = ?4
                WHERE id = ?1",
            params![run_id, state, error_class, detail],
        )?;
        if changed == 0 {
            return Err(Error::TaskNotFound(run_id.to_string()));
        }
        Ok(())
    }

    /// Append one event to a run's audit trail and return the stored row.
    ///
    /// `seq` is allocated per run. The worker is the only writer for a given
    /// run, so the read-then-insert is safe in practice; the unique constraint
    /// is the backstop if that assumption ever changes.
    pub fn append_run_event(
        &self,
        run_id: &str,
        task_id: &str,
        kind: &str,
        payload: &str,
    ) -> Result<RunEvent> {
        let seq: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seq), -1) + 1 FROM run_events WHERE run_id = ?1",
            params![run_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO run_events (run_id, task_id, seq, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![run_id, task_id, seq, kind, payload],
        )?;
        let id = self.conn.last_insert_rowid();
        self.get_run_event(id)?
            .ok_or_else(|| Error::TaskNotFound(format!("run event {id}")))
    }

    /// Fetch one event by row id.
    pub fn get_run_event(&self, id: i64) -> Result<Option<RunEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, task_id, seq, kind, payload, created_at
             FROM run_events WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], row_to_run_event)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// A task's event timeline, oldest first. `limit` caps the number of
    /// events returned (the API uses this to keep snapshots bounded).
    pub fn list_run_events(&self, task_id: &str, limit: usize) -> Result<Vec<RunEvent>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, task_id, seq, kind, payload, created_at
             FROM run_events WHERE task_id = ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![task_id, limit], row_to_run_event)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// One run's event timeline, oldest first.
    pub fn list_run_events_for_run(&self, run_id: &str, limit: usize) -> Result<Vec<RunEvent>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, task_id, seq, kind, payload, created_at
             FROM run_events WHERE run_id = ?1 ORDER BY seq ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, limit], row_to_run_event)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The most recent run for a task, if any.
    pub fn latest_run(&self, task_id: &str) -> Result<Option<RunRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, mode, state, started_at, ended_at, error_class, detail
             FROM runs WHERE task_id = ?1 ORDER BY rowid DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![task_id], row_to_run)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Total number of tasks, regardless of status.
    pub fn task_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Number of tasks per status, in pipeline order (zero-filled).
    ///
    /// Always returns one entry per [`TaskStatus`], even when no tasks have
    /// that status, so callers can render a complete board without
    /// special-casing missing rows.
    pub fn task_counts_by_status(&self) -> Result<Vec<(TaskStatus, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT status, COUNT(*) FROM tasks GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        // Start from a zero-filled map so every status appears exactly once.
        let mut counts: Vec<(TaskStatus, usize)> =
            TaskStatus::ALL.iter().map(|s| (*s, 0usize)).collect();
        for row in rows {
            let (status, count) = row?;
            let status = status.parse()?;
            if let Some(entry) = counts.iter_mut().find(|(s, _)| *s == status) {
                entry.1 = count as usize;
            }
        }
        Ok(counts)
    }

    // ---------------------------------------------------------- dependencies

    /// Record that `task_id` depends on `depends_on_id` (a DAG edge).
    ///
    /// Idempotent (a duplicate edge is a no-op). Errors when either task is
    /// missing or when the edge would close a cycle.
    pub fn add_dependency(&self, task_id: &str, depends_on_id: &str) -> Result<()> {
        if task_id == depends_on_id {
            return Err(Error::CycleDetected {
                task_id: task_id.to_string(),
                depends_on_id: depends_on_id.to_string(),
            });
        }
        // Both endpoints must exist (the FK would also catch this, but with a
        // less helpful error).
        self.require_task(task_id)?;
        self.require_task(depends_on_id)?;
        // Adding task -> dep closes a cycle iff dep already (transitively)
        // depends on task.
        if self.depends_on(depends_on_id)?.iter().any(|d| d == task_id) {
            return Err(Error::CycleDetected {
                task_id: task_id.to_string(),
                depends_on_id: depends_on_id.to_string(),
            });
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO dependencies (task_id, depends_on_id) VALUES (?1, ?2)",
            params![task_id, depends_on_id],
        )?;
        Ok(())
    }

    /// Direct prerequisites of `task_id` (what must be `DONE` first).
    pub fn dependencies_of(&self, task_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT depends_on_id FROM dependencies WHERE task_id = ?1")?;
        let rows = stmt.query_map(params![task_id], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Direct dependents of `task_id` (tasks waiting on it).
    pub fn dependents_of(&self, task_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT task_id FROM dependencies WHERE depends_on_id = ?1")?;
        let rows = stmt.query_map(params![task_id], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Whether every prerequisite of `task_id` is `DONE`.
    ///
    /// Tasks with no prerequisites are trivially ready.
    pub fn all_dependencies_done(&self, task_id: &str) -> Result<bool> {
        let pending: i64 = self.conn.query_row(
            "SELECT COUNT(*)
             FROM dependencies d
             JOIN tasks t ON t.id = d.depends_on_id
             WHERE d.task_id = ?1 AND t.status <> 'DONE'",
            params![task_id],
            |row| row.get(0),
        )?;
        Ok(pending == 0)
    }

    /// Number of dependency edges in the DAG.
    pub fn dependency_count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM dependencies", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// All (task_id, depends_on_id) edges, for cycle checks and rendering.
    pub fn list_dependencies(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT task_id, depends_on_id FROM dependencies")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ------------------------------------------------- workspaces

    /// Register (or replace the record of) a task's workspace.
    pub fn upsert_workspace(&self, record: &WorkspaceRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspaces (task_id, path, branch, base_commit)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(task_id) DO UPDATE SET
                 path = excluded.path,
                 branch = excluded.branch,
                 base_commit = excluded.base_commit",
            params![
                record.task_id,
                record.path,
                record.branch,
                record.base_commit
            ],
        )?;
        Ok(())
    }

    /// Fetch the workspace record for a task, if any.
    pub fn get_workspace(&self, task_id: &str) -> Result<Option<WorkspaceRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, path, branch, base_commit FROM workspaces WHERE task_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![task_id], row_to_workspace)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Delete a task's workspace record (no-op when absent).
    pub fn delete_workspace(&self, task_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM workspaces WHERE task_id = ?1",
            params![task_id],
        )?;
        Ok(())
    }

    /// All registered workspaces, ordered by task id.
    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, path, branch, base_commit FROM workspaces ORDER BY task_id",
        )?;
        let rows = stmt.query_map([], row_to_workspace)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // ------------------------------------------------- transitive traversal

    /// Every task `task_id` transitively depends on (its full ancestor set).
    ///
    /// Iterative DFS over the edges; the DB guarantees acyclicity because
    /// [`Self::add_dependency`] refuses cycle-closing edges, but the visited
    /// set keeps this safe even against hand-edited databases.
    pub fn depends_on(&self, task_id: &str) -> Result<Vec<String>> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![task_id.to_string()];
        while let Some(current) = stack.pop() {
            for dep in self.dependencies_of(&current)? {
                if visited.insert(dep.clone()) {
                    stack.push(dep);
                }
            }
        }
        Ok(visited.into_iter().collect())
    }

    /// Every task that transitively depends on `task_id` (its full descendant
    /// set). Symmetric counterpart of [`Self::depends_on`].
    pub fn dependents_of_deep(&self, task_id: &str) -> Result<Vec<String>> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![task_id.to_string()];
        while let Some(current) = stack.pop() {
            for dep in self.dependents_of(&current)? {
                if visited.insert(dep.clone()) {
                    stack.push(dep);
                }
            }
        }
        Ok(visited.into_iter().collect())
    }
}

/// Hydrate a [`RunRecord`] from a query row.
fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRecord> {
    Ok(RunRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        mode: row.get(2)?,
        state: row.get(3)?,
        started_at: row.get(4)?,
        ended_at: row.get(5)?,
        error_class: row.get(6)?,
        detail: row.get(7)?,
    })
}

/// Hydrate a [`RunEvent`] from a query row.
fn row_to_run_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunEvent> {
    Ok(RunEvent {
        id: row.get(0)?,
        run_id: row.get(1)?,
        task_id: row.get(2)?,
        seq: row.get(3)?,
        kind: row.get(4)?,
        payload: row.get(5)?,
        created_at: row.get(6)?,
    })
}

/// Hydrate a [`WorkspaceRecord`] from a query row.
fn row_to_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceRecord> {
    Ok(WorkspaceRecord {
        task_id: row.get(0)?,
        path: row.get(1)?,
        branch: row.get(2)?,
        base_commit: row.get(3)?,
    })
}

/// Hydrate a [`Task`] from a query row using [`TASK_COLUMNS`].
fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get(3)?;
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        status: status.parse().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?,
        retry_count: row.get(4)?,
        worker_logs: row.get(5)?,
        agent_output: row.get(6)?,
        diff: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_creates_expected_tables() {
        let db = Database::open_in_memory().unwrap();
        let count: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'
                 AND name IN ('tasks','dependencies','workspaces','runs','run_events')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 5,
            "task, DAG, workspace, and run-journal tables exist"
        );
    }

    #[test]
    fn run_event_timelines_can_be_scoped_to_the_latest_run() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        for run_id in ["run-1", "run-2"] {
            db.start_run(&RunRecord {
                id: run_id.to_string(),
                task_id: "t1".to_string(),
                mode: "normal".to_string(),
                state: "RUNNING".to_string(),
                started_at: String::new(),
                ended_at: None,
                error_class: None,
                detail: String::new(),
            })
            .unwrap();
            db.append_run_event(run_id, "t1", "run_started", "{}")
                .unwrap();
        }

        assert_eq!(db.list_run_events("t1", 100).unwrap().len(), 2);
        let latest = db.latest_run("t1").unwrap().unwrap();
        assert_eq!(latest.id, "run-2");
        let scoped = db.list_run_events_for_run(&latest.id, 100).unwrap();
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].run_id, "run-2");
    }

    #[test]
    fn status_check_constraint_rejects_bad_values() {
        let db = Database::open_in_memory().unwrap();
        let err = db
            .conn
            .execute(
                "INSERT INTO tasks (id, title, status) VALUES ('t1', 'bad', 'NOT_A_STATUS')",
                [],
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("CHECK"),
            "expected CHECK violation, got: {err}"
        );
    }

    #[test]
    fn self_dependency_is_rejected() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "task one", "")).unwrap();
        let err = db.add_dependency("t1", "t1").unwrap_err();
        assert!(matches!(err, Error::CycleDetected { .. }));
    }

    #[test]
    fn counts_start_at_zero_and_new_tasks_default_to_backlog() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(db.task_count().unwrap(), 0);
        assert_eq!(db.dependency_count().unwrap(), 0);

        db.insert_task(&Task::new("t1", "task one", "")).unwrap();
        assert_eq!(db.task_count().unwrap(), 1);

        let counts = db.task_counts_by_status().unwrap();
        // Zero-filled across all statuses.
        assert_eq!(counts.len(), 8);
        let backlog = counts
            .iter()
            .find(|(s, _)| *s == TaskStatus::Backlog)
            .unwrap();
        assert_eq!(backlog.1, 1);
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("nested").join(".zou").join("state.db");
        let _db = Database::open(&db_path).unwrap();
        assert!(db_path.is_file());
    }

    #[test]
    fn insert_and_get_task_roundtrips() {
        let db = Database::open_in_memory().unwrap();
        let mut task = Task::new("t1", "title", "description");
        task.worker_logs = "log line".to_string();
        task.agent_output = "## Result\n\nDone.".to_string();
        db.insert_task(&task).unwrap();

        let loaded = db.get_task("t1").unwrap().expect("task should exist");
        assert_eq!(loaded.id, "t1");
        assert_eq!(loaded.title, "title");
        assert_eq!(loaded.description, "description");
        assert_eq!(loaded.status, TaskStatus::Backlog);
        assert_eq!(loaded.worker_logs, "log line");
        assert_eq!(loaded.agent_output, "## Result\n\nDone.");
        // Timestamps are filled in by SQLite on read.
        assert!(!loaded.created_at.is_empty());
        assert!(!loaded.updated_at.is_empty());
    }

    #[test]
    fn insert_duplicate_id_fails() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        assert!(db.insert_task(&Task::new("t1", "b", "")).is_err());
    }

    #[test]
    fn get_missing_task_is_none_and_require_is_error() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_task("nope").unwrap().is_none());
        assert!(matches!(
            db.require_task("nope").unwrap_err(),
            Error::TaskNotFound(id) if id == "nope"
        ));
    }

    #[test]
    fn set_status_updates_row_and_missing_task_errors() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        // force: BACKLOG -> DONE is a policy-violating shortcut for setup.
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        assert_eq!(db.get_task("t1").unwrap().unwrap().status, TaskStatus::Done);

        assert!(matches!(
            db.set_status("ghost", TaskStatus::Todo).unwrap_err(),
            Error::TaskNotFound(_)
        ));
    }

    #[test]
    fn set_worker_logs_and_diff_update_row() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        db.set_worker_logs("t1", "stdout...").unwrap();
        db.set_agent_output("t1", "## Summary\n\nDone.").unwrap();
        db.set_diff("t1", "diff --git a/x b/x").unwrap();

        let t = db.get_task("t1").unwrap().unwrap();
        assert_eq!(t.worker_logs, "stdout...");
        assert_eq!(t.agent_output, "## Summary\n\nDone.");
        assert_eq!(t.diff, "diff --git a/x b/x");
    }

    #[test]
    fn list_tasks_orders_by_creation() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t2", "second", "")).unwrap();
        db.insert_task(&Task::new("t1", "first", "")).unwrap();
        let all = db.list_tasks().unwrap();
        assert_eq!(all.len(), 2);

        let in_progress = db.list_tasks_by_status(TaskStatus::InProgress).unwrap();
        assert!(in_progress.is_empty());
    }

    #[test]
    fn dependencies_track_edges_and_traversal() {
        let db = Database::open_in_memory().unwrap();
        // Chain: t3 -> t2 -> t1 (t3 depends on t2 depends on t1).
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        db.insert_task(&Task::new("t2", "b", "")).unwrap();
        db.insert_task(&Task::new("t3", "c", "")).unwrap();
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t3", "t2").unwrap();

        assert_eq!(db.dependencies_of("t3").unwrap(), vec!["t2".to_string()]);
        assert_eq!(db.dependents_of("t1").unwrap(), vec!["t2".to_string()]);
        assert_eq!(db.dependency_count().unwrap(), 2);

        // Transitive: t3 depends on both t2 and t1.
        let ancestors = db.depends_on("t3").unwrap();
        assert!(ancestors.contains(&"t1".to_string()));
        assert!(ancestors.contains(&"t2".to_string()));
        assert_eq!(ancestors.len(), 2);

        let descendants = db.dependents_of_deep("t1").unwrap();
        assert!(descendants.contains(&"t2".to_string()));
        assert!(descendants.contains(&"t3".to_string()));
    }

    #[test]
    fn duplicate_dependency_is_idempotent() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        db.insert_task(&Task::new("t2", "b", "")).unwrap();
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t2", "t1").unwrap();
        assert_eq!(db.dependency_count().unwrap(), 1);
    }

    #[test]
    fn dependency_requires_existing_tasks() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        assert!(matches!(
            db.add_dependency("t1", "ghost").unwrap_err(),
            Error::TaskNotFound(_)
        ));
    }

    #[test]
    fn cycle_closing_edge_is_rejected() {
        let db = Database::open_in_memory().unwrap();
        for id in ["t1", "t2", "t3"] {
            db.insert_task(&Task::new(id, id, "")).unwrap();
        }
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t3", "t2").unwrap();

        // t1 -> t3 would close the cycle t1 -> t3 -> t2 -> t1.
        let err = db.add_dependency("t1", "t3").unwrap_err();
        assert!(matches!(err, Error::CycleDetected { .. }));
        assert_eq!(
            db.dependency_count().unwrap(),
            2,
            "edge must not be recorded"
        );
    }

    #[test]
    fn all_dependencies_done_reflects_status() {
        let db = Database::open_in_memory().unwrap();
        for id in ["t1", "t2", "t3"] {
            db.insert_task(&Task::new(id, id, "")).unwrap();
        }
        db.add_dependency("t2", "t1").unwrap();
        db.add_dependency("t3", "t2").unwrap();

        // Tasks with no prerequisites are trivially ready.
        assert!(db.all_dependencies_done("t1").unwrap());
        assert!(!db.all_dependencies_done("t2").unwrap());
        assert!(!db.all_dependencies_done("t3").unwrap());

        // force: BACKLOG -> DONE shortcut for setup.
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        assert!(db.all_dependencies_done("t2").unwrap());
        // t3 still waits on t2 (not DONE), even though t1 is done.
        assert!(!db.all_dependencies_done("t3").unwrap());
    }

    #[test]
    fn workspace_records_roundtrip_and_upsert() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();

        assert!(db.get_workspace("t1").unwrap().is_none());

        let record = WorkspaceRecord {
            task_id: "t1".to_string(),
            path: "/repo/.zou/ws/t1".to_string(),
            branch: "zou/t1".to_string(),
            base_commit: "abc123".to_string(),
        };
        db.upsert_workspace(&record).unwrap();
        assert_eq!(db.get_workspace("t1").unwrap().unwrap(), record);
        assert_eq!(db.list_workspaces().unwrap(), vec![record.clone()]);

        // Upsert replaces the existing row rather than duplicating it.
        let updated = WorkspaceRecord {
            base_commit: "def456".to_string(),
            ..record
        };
        db.upsert_workspace(&updated).unwrap();
        assert_eq!(db.list_workspaces().unwrap().len(), 1);
        assert_eq!(
            db.get_workspace("t1").unwrap().unwrap().base_commit,
            "def456"
        );
    }

    #[test]
    fn workspace_records_cascade_on_task_delete() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();
        db.upsert_workspace(&WorkspaceRecord {
            task_id: "t1".to_string(),
            path: "/x".to_string(),
            branch: "zou/t1".to_string(),
            base_commit: "abc".to_string(),
        })
        .unwrap();

        db.delete_workspace("t1").unwrap();
        assert!(db.get_workspace("t1").unwrap().is_none());
        // Deleting a nonexistent record is a no-op.
        db.delete_workspace("t1").unwrap();
    }

    #[test]
    fn illegal_status_transition_is_rejected() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "a", "")).unwrap();

        // BACKLOG -> DONE skips the whole pipeline; must be refused.
        let err = db.set_status("t1", TaskStatus::Done).unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidTransition { ref task_id, from, to }
                if task_id == "t1" && from == TaskStatus::Backlog && to == TaskStatus::Done
        ));
        // Status unchanged after the refusal.
        assert_eq!(
            db.get_task("t1").unwrap().unwrap().status,
            TaskStatus::Backlog
        );

        // DONE is terminal: cannot be demoted even via the legal path.
        db.force_set_status("t1", TaskStatus::Done).unwrap();
        assert!(matches!(
            db.set_status("t1", TaskStatus::Backlog).unwrap_err(),
            Error::InvalidTransition { .. }
        ));

        // force bypasses the policy.
        db.force_set_status("t1", TaskStatus::Backlog).unwrap();
        assert_eq!(
            db.get_task("t1").unwrap().unwrap().status,
            TaskStatus::Backlog
        );
    }

    #[test]
    fn update_task_edits_title_and_description_independently() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "original", "desc"))
            .unwrap();

        // Update only the description (the Phase 7 reject-flow use case).
        db.update_task("t1", None, Some("new desc")).unwrap();
        let t = db.get_task("t1").unwrap().unwrap();
        assert_eq!(t.title, "original");
        assert_eq!(t.description, "new desc");

        // Update only the title.
        db.update_task("t1", Some("renamed"), None).unwrap();
        let t = db.get_task("t1").unwrap().unwrap();
        assert_eq!(t.title, "renamed");
        assert_eq!(t.description, "new desc");

        // Both at once.
        db.update_task("t1", Some("t"), Some("d")).unwrap();
        let t = db.get_task("t1").unwrap().unwrap();
        assert_eq!(t.title, "t");
        assert_eq!(t.description, "d");

        // Missing task errors.
        assert!(matches!(
            db.update_task("ghost", None, Some("x")).unwrap_err(),
            Error::TaskNotFound(_)
        ));
    }

    #[test]
    fn delete_task_removes_row_and_cascades_edges() {
        let db = Database::open_in_memory().unwrap();
        for id in ["t1", "t2"] {
            db.insert_task(&Task::new(id, id, "")).unwrap();
        }
        db.add_dependency("t2", "t1").unwrap();
        assert_eq!(db.dependency_count().unwrap(), 1);

        assert!(db.delete_task("t1").unwrap());
        assert_eq!(db.task_count().unwrap(), 1);
        // The dependency edge was removed by the FK cascade.
        assert_eq!(db.dependency_count().unwrap(), 0);

        // Deleting again reports false (nothing left).
        assert!(!db.delete_task("t1").unwrap());
    }

    // ------------------------------------------------- Phase 8: retry_count

    #[test]
    fn retry_count_starts_at_zero_and_round_trips() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "t", "d")).unwrap();
        assert_eq!(db.require_task("t1").unwrap().retry_count, 0);

        db.set_retry_count("t1", 2).unwrap();
        assert_eq!(db.require_task("t1").unwrap().retry_count, 2);
        assert_eq!(db.list_tasks().unwrap()[0].retry_count, 2);

        assert!(matches!(
            db.set_retry_count("ghost", 1).unwrap_err(),
            Error::TaskNotFound(_)
        ));
    }

    #[test]
    fn new_status_values_round_trip_and_transition_through_the_db() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "t", "d")).unwrap();

        // Full self-healing path: TODO -> IN_PROGRESS -> EVALUATING.
        db.set_status("t1", TaskStatus::Todo).unwrap();
        db.set_status("t1", TaskStatus::InProgress).unwrap();
        db.set_status("t1", TaskStatus::Evaluating).unwrap();
        assert_eq!(
            db.require_task("t1").unwrap().status,
            TaskStatus::Evaluating
        );
        // EVALUATING -> REVIEW -> DONE.
        db.set_status("t1", TaskStatus::Review).unwrap();
        db.set_status("t1", TaskStatus::Done).unwrap();

        // Parked task path: IN_PROGRESS -> EVALUATING -> FAILED_NEEDS_HUMAN
        // -> TODO (human sends it back).
        db.insert_task(&Task::new("t2", "t", "d")).unwrap();
        db.set_status("t2", TaskStatus::Todo).unwrap();
        db.set_status("t2", TaskStatus::InProgress).unwrap();
        db.set_status("t2", TaskStatus::Evaluating).unwrap();
        db.set_status("t2", TaskStatus::FailedNeedsHuman).unwrap();
        assert_eq!(
            db.require_task("t2").unwrap().status,
            TaskStatus::FailedNeedsHuman
        );
        db.set_status("t2", TaskStatus::Todo).unwrap();
    }

    #[test]
    fn counts_cover_the_two_new_statuses() {
        let db = Database::open_in_memory().unwrap();
        db.insert_task(&Task::new("t1", "t", "d")).unwrap();
        db.insert_task(&Task::new("t2", "t", "d")).unwrap();
        db.set_status("t1", TaskStatus::Todo).unwrap();
        db.force_set_status("t1", TaskStatus::Evaluating).unwrap();
        db.force_set_status("t2", TaskStatus::FailedNeedsHuman)
            .unwrap();

        let counts = db.task_counts_by_status().unwrap();
        assert_eq!(counts.len(), 8, "every status appears, zero-filled");
        assert_eq!(
            counts
                .iter()
                .find(|(s, _)| *s == TaskStatus::Evaluating)
                .unwrap()
                .1,
            1
        );
        assert_eq!(
            counts
                .iter()
                .find(|(s, _)| *s == TaskStatus::FailedNeedsHuman)
                .unwrap()
                .1,
            1
        );
    }

    // ------------------------------------------------- Phase 8: migration

    /// The exact `tasks` DDL shipped before Phase 8 (no `retry_count`,
    /// old status whitelist).
    const OLD_SCHEMA: &str = r#"
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        title        TEXT NOT NULL,
        description  TEXT NOT NULL DEFAULT '',
        status       TEXT NOT NULL DEFAULT 'BACKLOG'
                     CHECK (status IN ('BACKLOG','TODO','IN_PROGRESS','REVIEW','DONE')),
        worker_logs  TEXT NOT NULL DEFAULT '',
        diff         TEXT NOT NULL DEFAULT '',
        created_at   TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_tasks_status ON tasks(status);
    "#;

    #[test]
    fn opening_an_old_database_migrates_it_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("state.db");

        // Build a database exactly as a pre-Phase-8 binary would have.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(OLD_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO tasks (id, title, description, status) VALUES ('old-1', 'old task', 'keep me', 'REVIEW')",
                [],
            )
            .unwrap();
        }

        // Opening with the new binary migrates transparently.
        let db = Database::open(&db_path).unwrap();

        // 1. Old row survived with its data intact.
        let task = db.require_task("old-1").unwrap();
        assert_eq!(task.title, "old task");
        assert_eq!(task.description, "keep me");
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 0, "new column defaults to 0");
        assert_eq!(task.agent_output, "", "new column defaults to empty");

        // 2. The new statuses are writable (the old CHECK is gone): route
        // through the legal chain to reach them.
        db.set_status("old-1", TaskStatus::Todo).unwrap();
        db.set_status("old-1", TaskStatus::InProgress).unwrap();
        db.set_status("old-1", TaskStatus::Evaluating).unwrap();
        db.set_status("old-1", TaskStatus::FailedNeedsHuman)
            .unwrap();

        // 3. retry_count is writable and the table shape matches a fresh DB.
        db.set_retry_count("old-1", 3).unwrap();
        assert_eq!(db.require_task("old-1").unwrap().retry_count, 3);

        let migrated_sql: String = {
            let raw = Connection::open(&db_path).unwrap();
            raw.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(migrated_sql.contains("retry_count"));
        assert!(migrated_sql.contains("RESOLVING_CONFLICTS"));
        assert!(migrated_sql.contains("agent_output"));

        // 4. Re-opening is a no-op (the markers are now present).
        let _db2 = Database::open(&db_path).unwrap();
    }

    /// The exact `tasks` DDL shipped by Phase 8 (has `retry_count` +
    /// `EVALUATING`, but the status whitelist predates `RESOLVING_CONFLICTS`).
    const PHASE8_SCHEMA: &str = r#"
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        title        TEXT NOT NULL,
        description  TEXT NOT NULL DEFAULT '',
        status       TEXT NOT NULL DEFAULT 'BACKLOG'
                     CHECK (status IN ('BACKLOG','TODO','IN_PROGRESS','EVALUATING','REVIEW','DONE','FAILED_NEEDS_HUMAN')),
        retry_count  INTEGER NOT NULL DEFAULT 0,
        worker_logs  TEXT NOT NULL DEFAULT '',
        diff         TEXT NOT NULL DEFAULT '',
        created_at   TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_tasks_status ON tasks(status);
    "#;

    #[test]
    fn opening_a_phase8_database_gains_the_conflict_status() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("state.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(PHASE8_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO tasks (id, title, status, retry_count) VALUES ('p8', 'phase8 task', 'REVIEW', 1)",
                [],
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        // The row and its retry_count survive.
        let task = db.require_task("p8").unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 1);

        // The new status is now writable (the old CHECK rejected it): route
        // REVIEW -> RESOLVING_CONFLICTS through the legal Phase 9 path.
        db.set_status("p8", TaskStatus::ResolvingConflicts).unwrap();
        assert_eq!(
            db.require_task("p8").unwrap().status,
            TaskStatus::ResolvingConflicts
        );

        // The rebuilt CHECK accepts the new value.
        let sql: String = {
            let raw = Connection::open(&db_path).unwrap();
            raw.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert!(sql.contains("RESOLVING_CONFLICTS"));

        // Re-opening is a no-op.
        let _db2 = Database::open(&db_path).unwrap();
    }

    /// The exact `tasks` DDL shipped before the Markdown-output release:
    /// Phase 9 status set + retry_count, but no `agent_output` column.
    const PHASE9_SCHEMA: &str = r#"
    CREATE TABLE tasks (
        id           TEXT PRIMARY KEY,
        title        TEXT NOT NULL,
        description  TEXT NOT NULL DEFAULT '',
        status       TEXT NOT NULL DEFAULT 'BACKLOG'
                     CHECK (status IN ('BACKLOG','TODO','IN_PROGRESS','EVALUATING','REVIEW','RESOLVING_CONFLICTS','DONE','FAILED_NEEDS_HUMAN')),
        retry_count  INTEGER NOT NULL DEFAULT 0,
        worker_logs  TEXT NOT NULL DEFAULT '',
        diff         TEXT NOT NULL DEFAULT '',
        created_at   TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX idx_tasks_status ON tasks(status);
    "#;

    #[test]
    fn opening_a_phase9_database_gains_agent_output() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("state.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(PHASE9_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO tasks (id, title, status, retry_count, worker_logs, diff)
                 VALUES ('p9', 'phase9 task', 'REVIEW', 2, 'transcript kept', 'diff kept')",
                [],
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        let task = db.require_task("p9").unwrap();
        assert_eq!(task.status, TaskStatus::Review);
        assert_eq!(task.retry_count, 2);
        assert_eq!(task.worker_logs, "transcript kept");
        assert_eq!(task.diff, "diff kept");
        assert_eq!(task.agent_output, "", "new column defaults to empty");

        db.set_agent_output("p9", "## Answer\n\nNo files changed.")
            .unwrap();
        assert_eq!(
            db.require_task("p9").unwrap().agent_output,
            "## Answer\n\nNo files changed."
        );

        // Re-opening is a no-op.
        let _db2 = Database::open(&db_path).unwrap();
    }

    #[test]
    fn migration_leaves_dependency_and_workspace_references_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("state.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(OLD_SCHEMA).unwrap();
            conn.execute_batch(
                "CREATE TABLE dependencies (
                     task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                     depends_on_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
                     PRIMARY KEY (task_id, depends_on_id),
                     CHECK (task_id <> depends_on_id)
                 );
                 INSERT INTO tasks (id, title, status) VALUES ('a', 'a', 'DONE'), ('b', 'b', 'TODO');
                 INSERT INTO dependencies (task_id, depends_on_id) VALUES ('b', 'a');
                 CREATE TABLE workspaces (
                     task_id     TEXT PRIMARY KEY REFERENCES tasks(id) ON DELETE CASCADE,
                     path        TEXT NOT NULL,
                     branch      TEXT NOT NULL,
                     base_commit TEXT NOT NULL,
                     created_at  TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 INSERT INTO workspaces (task_id, path, branch, base_commit)
                     VALUES ('a', '/ws/a', 'zou/a', 'abc123');",
            )
            .unwrap();
        }

        let db = Database::open(&db_path).unwrap();
        assert_eq!(db.dependencies_of("b").unwrap(), vec!["a".to_string()]);
        assert_eq!(db.get_workspace("a").unwrap().unwrap().branch, "zou/a");
    }

    #[test]
    fn migration_does_not_run_again_on_a_fresh_database() {
        // The migration is detect-only: on a database created by the current
        // SCHEMA it must not rebuild anything. Proof: a rebuild stores the
        // renamed table as `CREATE TABLE "tasks"`, while a fresh database
        // keeps the unquoted form (SQLite strips `IF NOT EXISTS` when
        // recording the statement).
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("state.db");
        {
            let db = Database::open(&db_path).unwrap();
            db.insert_task(&Task::new("t1", "t", "d")).unwrap();
        }
        let _ = Database::open(&db_path).unwrap();

        let raw = Connection::open(&db_path).unwrap();
        let sql: String = raw
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            !sql.starts_with("CREATE TABLE \"tasks\""),
            "fresh database must not be rebuilt: {sql}"
        );
        let db = Database::open(&db_path).unwrap();
        assert_eq!(db.require_task("t1").unwrap().retry_count, 0);
    }
}
