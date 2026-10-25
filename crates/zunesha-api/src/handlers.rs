//! REST endpoints backing the dashboard.
//!
//! Handlers open a short-lived [`Database`] per call — the same pattern the
//! worker pool uses — so no shared `rusqlite::Connection` is ever moved across
//! threads. Blocking SQLite work is pushed onto `spawn_blocking` so the async
//! runtime stays responsive while a statement runs.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use zunesha_core::{
    approve_task, plan, recover_stuck, Database, Error, MergeOutcome, Task, TaskStatus,
};

use crate::events::{
    dependency_edges, snapshot_event, zero_filled_counts, DependencyEdge, StatusCount,
};
use crate::AppState;

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

/// An API failure, carrying the HTTP status it should surface as.
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn internal(err: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

/// Translate core errors into sensible HTTP statuses.
///
/// The important distinctions for a client:
/// * `TaskNotFound` → 404 so the UI can drop the row.
/// * `InvalidTransition` → 409, because the task exists but its lifecycle state
///   forbids the action (e.g. approving a `TODO` task).
/// * `MergeConflict` → 409 too: the approval is well-formed but the approved
///   diff no longer applies *and* could not be rebased automatically (Phase 9
///   fallback, e.g. git unavailable); the task stays in `REVIEW`. An ordinary
///   conflict instead queues [`MergeOutcome::ConflictResolutionQueued`] with a
///   200.
/// * `NotInitialized` → 503, telling the user to run `zou init`.
/// * Validation-ish errors (`CycleDetected`, `Workspace`, `Git`) → 400.
impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        use Error as E;
        let status = match &err {
            E::TaskNotFound(_) => StatusCode::NOT_FOUND,
            E::InvalidTransition { .. } | E::MergeConflict { .. } => StatusCode::CONFLICT,
            E::NotInitialized { .. } | E::NotAGitRepository { .. } => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            E::CycleDetected { .. }
            | E::UnknownTaskStatus(_)
            | E::Workspace(_)
            | E::Git(_)
            | E::PathNotFound { .. } => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self::new(status, err.to_string())
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::internal(format!("background task failed: {err}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // A panic inside a JoinError shouldn't leak a bare 500 with no body.
        let body = Json(json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

type ApiResult<T> = std::result::Result<T, ApiError>;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

impl AppState {
    /// Open a fresh database handle for this installation.
    fn open_db(&self) -> ApiResult<Database> {
        Ok(Database::open(&self.paths.state_db())?)
    }

    /// Broadcast the current board state to every connected WebSocket client.
    ///
    /// Best-effort: a failure to read the DB here shouldn't roll back the
    /// mutation that triggered it, so errors are swallowed (the next mutation
    /// or reconnect will resync the client).
    fn emit_snapshot(&self) {
        if let Ok(db) = Database::open(&self.paths.state_db()) {
            if let Ok(snapshot) = snapshot_event(&db) {
                let _ = self.events.send(snapshot);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response shapes
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TasksResponse {
    pub tasks: Vec<Task>,
    /// Dependency edges (`task_id` ← `depends_on`), immutable after planning.
    pub dependencies: Vec<DependencyEdge>,
    pub counts: Vec<StatusCount>,
}

#[derive(Deserialize)]
pub struct PlanBody {
    pub prompt: String,
}

#[derive(Serialize)]
pub struct PlanResponse {
    pub task_ids: Vec<String>,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub started: usize,
}

#[derive(Deserialize)]
pub struct RejectBody {
    /// Free-form reviewer feedback. Appended to the task description so the
    /// next agent attempt sees why the work was rejected.
    #[serde(default)]
    pub feedback: String,
}

#[derive(Serialize)]
pub struct ApproveResponse {
    pub task: Task,
    /// Tasks the DAG engine promoted to `TODO` as a consequence.
    pub promoted: Vec<String>,
    /// What happened to the approved diff: applied, nothing to land, or
    /// handed to auto-merge conflict resolution (Phase 9).
    pub integration: MergeOutcome,
}

#[derive(Deserialize)]
pub struct RecoverBody {
    /// Release only this task; omit (or send `{}`) to release every stuck
    /// task.
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Serialize)]
pub struct RecoverResponse {
    /// Tasks moved from `IN_PROGRESS`/`EVALUATING`/`RESOLVING_CONFLICTS`
    /// back to `TODO` (workspace kept).
    pub released: Vec<String>,
}

// ---------------------------------------------------------------------------
// Read endpoints
// ---------------------------------------------------------------------------

/// `GET /health` – liveness probe.
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

/// `GET /api/tasks` – every task, the dependency edges, and per-status counts.
pub async fn list_tasks(State(state): State<AppState>) -> ApiResult<Json<TasksResponse>> {
    let response = tokio::task::spawn_blocking(move || {
        let db = Database::open(&state.paths.state_db())?;
        Ok::<_, Error>(TasksResponse {
            tasks: db.list_tasks()?,
            dependencies: dependency_edges(&db)?,
            counts: zero_filled_counts(&db)?,
        })
    })
    .await??;
    Ok(Json(response))
}

/// `GET /api/tasks/:id` – one task, including its captured logs and diff.
pub async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> ApiResult<Json<Task>> {
    let db = state.open_db()?;
    let task = db.require_task(&task_id)?;
    Ok(Json(task))
}

/// `GET /api/status` – per-status counts and the concurrency cap.
pub async fn status(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let db = state.open_db()?;
    let counts = zero_filled_counts(&db)?;
    Ok(Json(json!({
        "counts": counts,
        "max_concurrency": state.pool.max_concurrency(),
        "initialized": true,
    })))
}

// ---------------------------------------------------------------------------
// Mutation endpoints
// ---------------------------------------------------------------------------

/// `POST /api/plan` – decompose a feature prompt into a task DAG.
pub async fn plan_feature(
    State(state): State<AppState>,
    Json(body): Json<PlanBody>,
) -> ApiResult<Json<PlanResponse>> {
    let paths = state.paths.clone();
    let task_ids = tokio::task::spawn_blocking(move || {
        let db = Database::open(&paths.state_db())?;
        plan(&db, &body.prompt)
    })
    .await??;

    state.emit_snapshot();
    Ok(Json(PlanResponse { task_ids }))
}

/// `POST /api/run` – start the agent on every ready (`TODO`) task.
///
/// Returns immediately with how many tasks were claimed; their output streams
/// over the WebSocket rather than blocking this response.
pub async fn run_ready(State(state): State<AppState>) -> ApiResult<Json<RunResponse>> {
    let started = state.pool.run_ready().await?.len();
    state.emit_snapshot();
    Ok(Json(RunResponse { started }))
}

/// `POST /api/tasks/:id/run` – start the agent on a single ready task.
pub async fn run_one(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> ApiResult<Json<RunResponse>> {
    state.pool.spawn_task(task_id).await?;
    state.emit_snapshot();
    Ok(Json(RunResponse { started: 1 }))
}

/// `POST /api/tasks/:id/approve` – accept reviewed work: merge, finish, unlock.
///
/// Only a `REVIEW` task can be approved (anything else yields 409), which is
/// the "human in the loop" gate. Approval first lands the task's diff in the
/// working tree ([`approve_task`]). Two outcomes:
///
/// * **Clean merge** → `REVIEW → DONE`, the DAG engine runs so anything
///   waiting on this task flips to `TODO`, and the workspace is reaped.
/// * **Conflict (Phase 9)** → nothing is written to the tree; instead the
///   workspace is rebased onto the repository's current content with conflict
///   markers, the task moves to `RESOLVING_CONFLICTS`, and this handler
///   immediately spawns the conflict-resolution agent (streaming its logs over
///   the WS). The result re-enters validation and returns to `REVIEW`, so the
///   human still approves the final diff — with only the pathological case
///   (the rebase itself failing, e.g. git unavailable) reported as `409`
///   `merge conflict …`, leaving the task in `REVIEW` for manual repair.
pub async fn approve(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> ApiResult<Json<ApproveResponse>> {
    let paths = state.paths.clone();
    let id = task_id.clone();
    let outcome = tokio::task::spawn_blocking(move || approve_task(&paths, &id)).await??;

    // Phase 9: hand a conflicted task straight to the resolution agent.
    if matches!(
        outcome.integration,
        MergeOutcome::ConflictResolutionQueued { .. }
    ) {
        state.pool.spawn_resolution(task_id).await?;
    }

    state.emit_snapshot();
    Ok(Json(ApproveResponse {
        task: outcome.task,
        promoted: outcome.promoted,
        integration: outcome.integration,
    }))
}

/// `POST /api/tasks/:id/reject` – send work back for another attempt.
///
/// Accepts tasks in `REVIEW` (the reviewer wants changes) or in
/// `FAILED_NEEDS_HUMAN` (validation exhausted its retries; the human frees it
/// back into the queue). Appends the reviewer's feedback to the task
/// description (so the next agent attempt sees it), resets the retry counter
/// for a fresh cycle, and moves the task to `TODO`. The workspace is left
/// intact so the retry can build on the previous diff rather than starting
/// cold.
pub async fn reject(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(body): Json<RejectBody>,
) -> ApiResult<Json<Task>> {
    let paths = state.paths.clone();
    let feedback = body.feedback;
    let task = tokio::task::spawn_blocking(move || {
        let db = Database::open(&paths.state_db())?;
        let task = db.require_task(&task_id)?;
        if !matches!(
            task.status,
            TaskStatus::Review | TaskStatus::FailedNeedsHuman
        ) {
            return Err(Error::InvalidTransition {
                task_id: task_id.clone(),
                from: task.status,
                to: TaskStatus::Todo,
            });
        }

        let description = if feedback.trim().is_empty() {
            None
        } else {
            let mut updated = task.description.clone();
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("\n## Reviewer feedback\n");
            updated.push_str(feedback.trim());
            Some(updated)
        };
        db.update_task(&task_id, None, description.as_deref())?;
        // Fresh human-approved cycle: the 3-failure budget starts over.
        db.set_retry_count(&task_id, 0)?;
        db.set_status(&task_id, TaskStatus::Todo)?;
        db.require_task(&task_id)
    })
    .await??;

    state.emit_snapshot();
    Ok(Json(task))
}

/// `POST /api/recover` – release stuck `IN_PROGRESS`/`EVALUATING`/
/// `RESOLVING_CONFLICTS` tasks back to `TODO`.
///
/// Crash recovery: a killed daemon (or an interrupted `zou run`) leaves its
/// tasks claimed forever — they can never be re-run, reviewed, or rejected,
/// and dependents stay blocked. Send `{"task_id": "..."}` to release one
/// task, or an empty body to release every stuck task.
///
/// For claims left by a *dead daemon* nothing is needed: `zou start` releases
/// and re-runs them automatically at boot (Phase 11). Only call this for
/// claims that no live worker owns: a CLI run and the daemon can legitimately
/// coexist, and nothing in the state can tell a live agent from a dead one.
/// The workspace is kept, so the retry builds on whatever the crashed agent
/// already produced.
pub async fn recover(
    State(state): State<AppState>,
    // An absent or malformed body means "recover everything stuck".
    body: Result<Json<RecoverBody>, JsonRejection>,
) -> ApiResult<Json<RecoverResponse>> {
    let task_id = body.ok().and_then(|Json(b)| b.task_id);
    let paths = state.paths.clone();
    let released = tokio::task::spawn_blocking(move || {
        let db = Database::open(&paths.state_db())?;
        recover_stuck(&db, task_id.as_deref())
    })
    .await??;

    state.emit_snapshot();
    Ok(Json(RecoverResponse { released }))
}

/// `ANY /api/*` – JSON 404 for unknown API paths.
///
/// Without this catch-all, an unmatched `/api/...` request falls through to
/// the dashboard fallback and receives `200` + HTML — a JSON client would
/// misread that as success. API paths must never serve the HTML fallback.
pub async fn api_not_found(Path(path): Path<String>) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        format!("unknown API endpoint: /api/{path}"),
    )
}

// ---------------------------------------------------------------------------
// Static dashboard fallback
// ---------------------------------------------------------------------------

/// Placeholder served at `/` when no built dashboard is available.
///
/// Reached when `ZOU_DASHBOARD_DIR` points nowhere and the auto-discovery
/// locations don't contain a built `web/out` (e.g. the dashboard hasn't been
/// built yet). The API itself is unaffected.
pub async fn placeholder() -> impl IntoResponse {
    const BODY: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>zou – orchestrator</title>
<style>
  :root { color-scheme: dark; }
  body { margin:0; min-height:100vh; display:grid; place-items:center;
         background:#0b0d10; color:#e6e8eb;
         font:16px/1.6 ui-sans-serif,system-ui,-apple-system,sans-serif; }
  main { max-width:44rem; padding:2rem; }
  h1 { font-size:1.6rem; margin:0 0 .5rem; letter-spacing:-.01em; }
  code { background:#171a1f; padding:.15em .4em; border-radius:5px;
         font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.9em; }
  ul { padding-left:1.2rem; } li { margin:.3rem 0; }
  a { color:#6cb6ff; }
</style>
</head>
<body>
<main>
  <h1>zou is running</h1>
  <p>The local API and WebSocket bridge are live, but no built dashboard was
     found. Build it with <code>npm install &amp;&amp; npm run build</code> in
     <code>web/</code>, or point <code>ZOU_DASHBOARD_DIR</code> at a built
     copy — meanwhile use the CLI or the endpoints below.</p>
  <ul>
    <li><code>GET  /health</code></li>
    <li><code>GET  /api/status</code></li>
    <li><code>GET  /api/tasks</code></li>
    <li><code>POST /api/plan</code> &nbsp;<code>{"prompt":"..."}</code></li>
    <li><code>POST /api/run</code></li>
    <li><code>POST /api/tasks/:id/run</code></li>
    <li><code>POST /api/tasks/:id/approve</code></li>
    <li><code>POST /api/tasks/:id/reject</code> &nbsp;<code>{"feedback":"..."}</code></li>
    <li><code>POST /api/recover</code> &nbsp;<code>{"task_id":"..."}</code> (crash recovery)</li>
    <li><code>WS   /ws</code></li>
  </ul>
</main>
</body>
</html>"#;
    (
        StatusCode::OK,
        [("content-type", "text/html; charset=utf-8")],
        BODY,
    )
}
