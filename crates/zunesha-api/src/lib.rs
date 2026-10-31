//! The local HTTP + WebSocket API that bridges the orchestrator core to the
//! dashboard.
//!
//! This crate exposes a single entrypoint, [`serve`], which binds an Axum
//! server on `localhost` and wires three things together:
//!
//! 1. **The task store** – every handler opens its own short-lived
//!    [`zunesha_core::Database`] handle (SQLite is fast for this, and
//!    WAL + `busy_timeout` make concurrent opens safe).
//! 2. **The worker pool** – [`zunesha_core::WorkerPool`] runs agents in
//!    isolated workspaces and emits [`zunesha_core::WorkerEvent`]s.
//! 3. **The event bus** – worker events are rebroadcast as [`events::ApiEvent`]s
//!    on a `tokio::sync::broadcast` channel, which the WebSocket handler
//!    streams to every connected client.
//!
//! ## Endpoints
//!
//! | Method | Path                      | Purpose                                  |
//! |--------|---------------------------|------------------------------------------|
//! | GET    | `/health`                 | Liveness probe.                          |
//! | GET    | `/api/status`             | Project identity, per-status counts + concurrency cap. |
//! | GET    | `/api/tasks`              | Project identity, every task, the dependency DAG, and per-status counts. |
//! | GET    | `/api/tasks/:id`          | One task (logs + diff included).         |
//! | POST   | `/api/plan`               | Decompose a prompt into a task DAG (built-in or `ZOU_PLANNER_CMD`). |
//! | POST   | `/api/run`                | Start agents on all ready (`TODO`) tasks.|
//! | POST   | `/api/tasks/:id/run`      | Start the agent on one ready task.       |
//! | POST   | `/api/tasks/:id/cancel`   | Stop a running agent and release its claim. |
//! | GET    | `/api/tasks/:id/events`   | Structured run/audit timeline for a task. |
//! | POST   | `/api/tasks/:id/approve`  | Accept reviewed work; merge or auto-resolve a conflict. |
//! | POST   | `/api/tasks/:id/reject`   | Send reviewed work back with feedback.   |
//! | POST   | `/api/recover`            | Release stuck tasks (worker crashed; on `zou start` this happens automatically). |
//! | GET    | `/api/playground`         | Compact task projection for the pixel office. |
//! | WS     | `/api/playground/ws`      | Compact live office events (no logs/diffs). |
//! | WS     | `/ws`                     | Live snapshots + streaming agent logs.   |
//!
//! Unmatched GET paths fall back to a static dashboard (served from `web/out`
//! when available, otherwise a helpful placeholder page).
//!
//! ## Crash resumption
//!
//! A daemon killed mid-run leaves its claims behind: tasks stuck in
//! `IN_PROGRESS`, `EVALUATING`, or `RESOLVING_CONFLICTS` that nothing will
//! ever finish. [`serve`] recovers them at boot — releasing each back to
//! `TODO` and re-running it in its kept workspace — so restarting
//! `zou start` resumes the run instead of stranding it. The endpoint stays
//! for claims a *live* CLI run may own, which only a human can judge (see
//! [`zunesha_core::dag::recover_stuck`]).

pub mod events;
pub mod handlers;
pub mod playground;
pub mod ws;

use std::sync::Arc;

use axum::routing::{any, get, post};
use axum::Router;
use tokio::sync::broadcast;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

use zunesha_core::{AgentCommand, WorkerPool, ZuneshaPaths};

use events::{spawn_event_bridge, ApiEvent};

/// Default port the local API binds to.
pub const DEFAULT_PORT: u16 = 57804;

/// Capacity of the event broadcast channel.
///
/// Deliberately small: most events are `TaskUpdated`/`Snapshot` payloads that
/// embed a task's full logs + diff, so a chatty agent plus a stalled client
/// would otherwise pin a lot of memory in the ring. A lagged client is not a
/// correctness problem — the WebSocket layer resyncs it with a fresh
/// [`crate::events::ApiEvent::Snapshot`], which the reducer merges
/// idempotently.
const EVENT_BUFFER: usize = 256;

/// Shared state handed to every Axum handler.
///
/// `Clone` is cheap: the heavy pieces live behind `Arc`, and the broadcast
/// sender is designed to be cloned.
#[derive(Clone)]
pub struct AppState {
    /// The installation this server is bound to.
    pub paths: ZuneshaPaths,
    /// The bounded pool that runs agents in isolated workspaces.
    pub pool: Arc<WorkerPool>,
    /// Fan-out channel every connected WebSocket client reads from.
    pub events: broadcast::Sender<ApiEvent>,
}

/// Configuration for [`serve`].
pub struct ServeConfig {
    /// Installation to serve.
    pub paths: ZuneshaPaths,
    /// Interface to bind (e.g. `127.0.0.1`).
    pub host: String,
    /// Port to bind; `0` picks an ephemeral port.
    pub port: u16,
    /// Cap on simultaneously running agents.
    pub max_concurrency: usize,
    /// The agent CLI to run for each task.
    pub agent: AgentCommand,
    /// Dashboard build directory (`web/out`). `None` runs
    /// [`find_dashboard_dir`]'s search; `Some` must contain `index.html` to
    /// be served (otherwise the placeholder page shows).
    pub dashboard_dir: Option<std::path::PathBuf>,
    /// Re-run automatically whatever tasks a previous daemon left claimed
    /// (Phase 11 crash resumption). At boot the previous process is gone by
    /// construction, which is what makes releasing its `IN_PROGRESS`,
    /// `EVALUATING`, and `RESOLVING_CONFLICTS` tasks safe here. Opt out when
    /// a concurrent CLI `zou run` may own those claims — see
    /// [`zunesha_core::dag::recover_stuck`].
    pub resume: bool,
}

impl ServeConfig {
    /// Bind to `127.0.0.1` with the default agent and concurrency cap.
    pub fn local(paths: ZuneshaPaths) -> Self {
        Self {
            paths,
            host: "127.0.0.1".to_string(),
            port: DEFAULT_PORT,
            max_concurrency: zunesha_core::DEFAULT_MAX_CONCURRENCY,
            agent: AgentCommand::default(),
            dashboard_dir: None,
            resume: true,
        }
    }
}

/// Build the router. Exposed so tests can exercise the full stack without
/// binding a real socket.
pub fn router(state: AppState, dashboard_dir: Option<std::path::PathBuf>) -> Router {
    let api = Router::new()
        .route("/health", get(handlers::health))
        .route("/api/status", get(handlers::status))
        .route("/api/tasks", get(handlers::list_tasks))
        .route("/api/tasks/{id}", get(handlers::get_task))
        // Attachments are base64 in the JSON body; the default 2 MiB limit
        // would reject a normal screenshot before the handler can validate
        // it. 24 MiB decoded is ~32 MiB encoded, so leave room for JSON and
        // the prompt rather than rejecting a valid maximum-size batch.
        .route(
            "/api/plan",
            post(handlers::plan_feature)
                .layer(axum::extract::DefaultBodyLimit::max(40 * 1024 * 1024)),
        )
        .route("/api/run", post(handlers::run_ready))
        .route("/api/tasks/{id}/run", post(handlers::run_one))
        .route("/api/tasks/{id}/cancel", post(handlers::cancel))
        .route("/api/tasks/{id}/events", get(handlers::events))
        .route("/api/tasks/{id}/approve", post(handlers::approve))
        .route(
            "/api/tasks/{id}/reject",
            post(handlers::reject).layer(axum::extract::DefaultBodyLimit::max(40 * 1024 * 1024)),
        )
        .route("/api/recover", post(handlers::recover))
        .route("/api/playground", get(playground::snapshot))
        .route("/api/playground/ws", get(playground::ws_handler))
        // Catch-all for unknown API paths: JSON 404 instead of the HTML
        // dashboard fallback (a JSON client would otherwise see `200` + HTML
        // for a typo'd endpoint). Specific routes above always win.
        .route("/api/{*rest}", any(handlers::api_not_found))
        .route("/ws", get(ws::ws_handler))
        // Permissive CORS: the dashboard may be served from a different
        // localhost port (e.g. the Next.js dev server) during Phase 6.
        .layer(CorsLayer::very_permissive())
        // Fill in the `AppState` the routes extract, yielding `Router<()>`.
        .with_state(state);

    // Static dashboard: serve `web/out` when the Phase 6 build exists, with
    // `index.html` as the SPA fallback so client-side routes resolve. Unknown
    // API paths never reach here (the catch-all above owns `/api/*`).
    let dashboard_dir = dashboard_dir.filter(|d| d.join("index.html").is_file());
    if let Some(dir) = dashboard_dir {
        let index = dir.join("index.html");
        let serve_dir = ServeDir::new(&dir).fallback(ServeFile::new(&index));
        api.fallback_service(serve_dir)
    } else {
        api.fallback(handlers::placeholder)
    }
}

/// Locate the built dashboard (`web/out/index.html`).
///
/// `zou start` is usually run from inside a *target* repository, so a bare
/// relative path would look in the wrong place. Candidates, in order:
///
/// 1. `ZOU_DASHBOARD_DIR` — explicit override.
/// 2. `--dashboard <dir>` — passed in as `explicit`.
/// 3. `web/out` relative to the current directory (running from the
///    orchestrator checkout).
/// 4. Walk up from the executable's directory until `web/out` is found (this
///    covers both the dev layout `<repo>/target/{debug,release}/zou` and
///    oddities like cargo's hardlinked `deps/` temp binary, where the
///    executable does not sit at the depth you'd guess).
/// 5. Walk up from the current directory (e.g. running from `web/` or a
///    nested folder of the orchestrator checkout).
pub fn find_dashboard_dir(explicit: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir);
    }
    if let Ok(dir) = std::env::var("ZOU_DASHBOARD_DIR") {
        let dir = std::path::PathBuf::from(dir);
        if dir.join("index.html").is_file() {
            return Some(dir);
        }
    }

    let has_dashboard = |d: &std::path::Path| d.join("index.html").is_file();
    let cwd = std::env::current_dir().ok()?;

    let cwd_rel = cwd.join("web").join("out");
    if has_dashboard(&cwd_rel) {
        return Some(cwd_rel);
    }

    // Walk up from the executable's directory. `current_exe()` can resolve to
    // an unexpected path (e.g. cargo's hardlinked `deps/.l2s.*` temp name), so
    // rather than guessing fixed depths like `../..`, search upward until we
    // find `web/out` or hit the filesystem root.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(found) = walk_up_for_dashboard(exe_dir) {
                return Some(found);
            }
        }
    }

    // Walk up from CWD (catches running from inside the checkout).
    walk_up_for_dashboard(&cwd)
}

/// Search `dir` and each of its ancestors for `web/out/index.html`.
///
/// Returns the first `<ancestor>/web/out` that contains `index.html`, or
/// `None` at the filesystem root. Used for both the executable-relative and
/// current-directory searches — `current_exe()` can resolve to an unexpected
/// path (e.g. cargo's hardlinked `deps/.l2s.*` temp name), so walking upward
/// is more robust than guessing a fixed depth like `../..`.
fn walk_up_for_dashboard(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut current = dir;
    loop {
        let candidate = current.join("web").join("out");
        if candidate.join("index.html").is_file() {
            return Some(candidate);
        }
        current = current.parent()?;
    }
}

/// Build the shared state, start the worker-event bridge, and return the
/// router — without binding a socket. Useful for tests.
pub fn build_app(config: ServeConfig) -> (Router, AppState) {
    let pool = Arc::new(WorkerPool::new(
        config.paths.clone(),
        config.max_concurrency,
        config.agent,
    ));
    let (events, _) = broadcast::channel::<ApiEvent>(EVENT_BUFFER);

    // Rebroadcast Phase 4's worker events as API events. This must happen
    // *before* any agent can start, or early log lines would be dropped.
    spawn_event_bridge(pool.subscribe(), config.paths.state_db(), events.clone());

    let state = AppState {
        paths: config.paths,
        pool,
        events,
    };
    let dashboard = find_dashboard_dir(config.dashboard_dir);
    (router(state.clone(), dashboard), state)
}

/// Bind and serve until the process is interrupted.
///
/// Returns a [`ServeHandle`] once the listener is bound and any interrupted
/// tasks have been resumed (see [`ServeConfig::resume`]). `ServeConfig::port`
/// of `0` picks an ephemeral port — read it back with
/// [`ServeHandle::address`].
pub async fn serve(config: ServeConfig) -> std::io::Result<ServeHandle> {
    let resume = config.resume;
    let bind_addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let bound = listener.local_addr()?;

    let (app, state) = build_app(config);

    // Phase 11: a restarted daemon picks its crashed run back up. The event
    // bridge above is already subscribed, so the released/re-claimed flips
    // stream to any dashboard that connects — and every connect replays a
    // snapshot anyway.
    let resumed = if resume {
        state.pool.resume_stuck().await.map_err(io_error)?
    } else {
        Vec::new()
    };

    let handle = tokio::spawn(async move { axum::serve(listener, app).await });

    Ok(ServeHandle {
        handle,
        address: bound,
        resumed,
    })
}

/// Map a core error into the I/O error [`serve`] speaks.
fn io_error(err: zunesha_core::Error) -> std::io::Error {
    std::io::Error::other(format!("crash resumption failed: {err}"))
}

/// Handle to a running server.
pub struct ServeHandle {
    handle: tokio::task::JoinHandle<std::io::Result<()>>,
    address: std::net::SocketAddr,
    resumed: Vec<String>,
}

impl ServeHandle {
    /// The address the server is actually listening on (useful when the
    /// configured port was `0`).
    pub fn address(&self) -> std::net::SocketAddr {
        self.address
    }

    /// Ids of the crashed tasks this boot released and re-ran (empty unless
    /// [`ServeConfig::resume`] was enabled and there was something to resume).
    pub fn resumed(&self) -> &[String] {
        &self.resumed
    }

    /// Wait for the server to shut down.
    pub async fn join(self) -> std::io::Result<()> {
        match self.handle.await {
            Ok(result) => result,
            Err(join_err) => Err(std::io::Error::other(format!(
                "server task panicked: {join_err}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out a fake checkout `<root>/web/out/index.html` and assert the
    /// upward search finds it from a deep starting point.
    ///
    /// This is a regression test for a real bug: the executable-relative
    /// search originally guessed a fixed `../..` depth, which missed when
    /// `current_exe()` resolved to cargo's hardlinked `deps/.l2s.*` temp name
    /// (one level deeper than `target/debug`). Walking upward from an
    /// arbitrarily deep path fixes it.
    #[test]
    fn walk_up_finds_dashboard_from_deep_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let out_dir = root.join("web").join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(out_dir.join("index.html"), "<html></html>").unwrap();

        // A path nested well below the root — deeper than the old `../..`.
        let deep = root.join("target").join("debug").join("deps");
        std::fs::create_dir_all(&deep).unwrap();

        let found = walk_up_for_dashboard(&deep).expect("should find web/out");
        assert_eq!(found, out_dir);
    }

    /// A directory with no `web/out` anywhere up the chain returns None
    /// rather than looping forever or panicking at the filesystem root.
    #[test]
    fn walk_up_returns_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        // tempdir lives under some root that has no web/out; walking up must
        // terminate at `/` and yield None.
        assert!(walk_up_for_dashboard(tmp.path()).is_none());
    }

    /// The explicit path always wins, verbatim, even if it doesn't exist —
    /// the caller decides whether to serve or fall back.
    #[test]
    fn explicit_path_is_returned_verbatim() {
        let explicit = std::path::PathBuf::from("/nonexistent/dashboard");
        assert_eq!(find_dashboard_dir(Some(explicit.clone())), Some(explicit));
    }
}
