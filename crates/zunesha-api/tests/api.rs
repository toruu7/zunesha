//! End-to-end tests for the local API.
//!
//! These exercise the full stack — router, handlers, database, and the
//! WebSocket bridge — against a real SQLite database in a temp directory, so
//! they catch wiring bugs that unit tests on individual handlers would miss.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

use zunesha_api::{build_app, ServeConfig};
use zunesha_core::{Database, TaskStatus, WorkspaceManager, ZuneshaPaths};

/// A scratch, initialized installation rooted in a temp directory.
///
/// `Database::open` creates `state.db` on disk, which is all `build_app`
/// needs to treat the directory as initialized.
fn scratch() -> (tempfile::TempDir, ZuneshaPaths) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ZuneshaPaths::new(tmp.path());
    std::fs::create_dir_all(paths.workspaces_dir()).unwrap();
    Database::open(&paths.state_db()).unwrap();
    (tmp, paths)
}

/// Build the app for a scratch install with a small concurrency cap.
fn app_for(paths: &ZuneshaPaths) -> Router {
    let config = ServeConfig {
        paths: paths.clone(),
        host: "127.0.0.1".into(),
        port: 0,
        max_concurrency: 2,
        agent: zunesha_core::AgentCommand::shell("true"),
        // Pin an explicitly-broken dashboard path: `None` would trigger
        // discovery, which can find a real `web/out` in the checkout and make
        // placeholder expectations depend on whether the dashboard has been
        // built.
        dashboard_dir: Some(std::path::PathBuf::from("/nonexistent-zou-test-dashboard")),
    };
    build_app(config).0
}

/// Send a JSON request and decode the response as JSON.
async fn json_req(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body)
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Send a request and return the raw response body bytes.
async fn body_of(app: Router, method: Method, uri: &str) -> Vec<u8> {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    res.into_body().collect().await.unwrap().to_bytes().to_vec()
}

#[tokio::test]
async fn health_reports_ok() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let (status, body) = json_req(&app, Method::GET, "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
}

#[tokio::test]
async fn status_zero_fills_every_column() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let (status, body) = json_req(&app, Method::GET, "/api/status", None).await;
    assert_eq!(status, StatusCode::OK);
    let counts = body["counts"].as_array().unwrap();
    assert_eq!(counts.len(), 8, "all eight statuses present, incl. zeros");
    assert!(counts.iter().all(|c| c["count"] == json!(0)));
    assert_eq!(body["max_concurrency"], json!(2));
}

#[tokio::test]
async fn plan_creates_tasks_and_lists_them() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);

    let (status, body) = json_req(
        &app,
        Method::POST,
        "/api/plan",
        Some(json!({ "prompt": "add dark mode" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let ids = body["task_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 3, "mock planner emits a 3-task chain");

    let (status, body) = json_req(&app, Method::GET, "/api/tasks", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tasks"].as_array().unwrap().len(), 3);
    // Head is TODO, the other two are BACKLOG.
    let todo = body["counts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["status"] == json!("TODO"))
        .unwrap();
    assert_eq!(todo["count"], json!(1));
}

#[tokio::test]
async fn get_missing_task_is_404() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let (status, _) = json_req(&app, Method::GET, "/api/tasks/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Plan a feature and return the head task id (the one promoted to TODO).
async fn plan_head(app: &Router) -> String {
    let (_, body) = json_req(
        app,
        Method::POST,
        "/api/plan",
        Some(json!({ "prompt": "ship it" })),
    )
    .await;
    body["task_ids"][0].as_str().unwrap().to_string()
}

/// Push a task to REVIEW using only legal transitions (mirrors what a worker
/// does after a successful agent run + validation).
fn force_to_review(paths: &ZuneshaPaths, id: &str) {
    let db = Database::open(&paths.state_db()).unwrap();
    db.set_status(id, TaskStatus::InProgress).unwrap();
    db.set_status(id, TaskStatus::Evaluating).unwrap();
    db.set_status(id, TaskStatus::Review).unwrap();
}

/// Push a task to `FAILED_NEEDS_HUMAN` the way the worker's retry loop does.
fn force_to_failed(paths: &ZuneshaPaths, id: &str) {
    let db = Database::open(&paths.state_db()).unwrap();
    db.set_status(id, TaskStatus::InProgress).unwrap();
    db.set_status(id, TaskStatus::Evaluating).unwrap();
    db.set_status(id, TaskStatus::FailedNeedsHuman).unwrap();
    db.set_retry_count(id, 3).unwrap();
}

#[tokio::test]
async fn approve_moves_review_to_done_and_unlocks_dependents() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await;
    force_to_review(&paths, &head);

    let (status, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/approve"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["task"]["status"], json!("DONE"));
    // The middle task of the chain should now be TODO.
    let promoted = body["promoted"].as_array().unwrap();
    assert_eq!(promoted.len(), 1, "one dependent unlocked");
    // No workspace existed, so there was never anything to land.
    assert_eq!(
        body["integration"]["status"],
        json!("no_changes"),
        "body: {body}"
    );
}

#[tokio::test]
async fn approve_with_conflicting_diff_queues_auto_resolution_and_runs_it() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);

    // A real file for the workspace to snapshot…
    std::fs::write(paths.root().join("app.rs"), "fn main() {}\n").unwrap();
    let head = plan_head(&app).await;

    // …agent work inside the isolated workspace…
    let db = Database::open(&paths.state_db()).unwrap();
    let record = WorkspaceManager::new(paths.clone())
        .create(&db, &head)
        .unwrap();
    std::fs::write(
        std::path::Path::new(&record.path).join("app.rs"),
        "fn main() { println!(\"agent\"); }\n",
    )
    .unwrap();
    force_to_review(&paths, &head);

    // …and a conflicting user edit to the same lines in the real tree.
    std::fs::write(paths.root().join("app.rs"), "fn main() { /* user */ }\n").unwrap();

    // Phase 9: the conflict no longer 409s — it queues auto-resolution.
    let (status, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/approve"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        body["integration"]["status"],
        json!("conflict_resolution_queued"),
        "body: {body}"
    );
    assert_eq!(
        body["task"]["status"],
        json!("RESOLVING_CONFLICTS"),
        "body: {body}"
    );
    let files = body["integration"]["files"].as_array().unwrap();
    assert!(
        files.iter().any(|f| f == "app.rs"),
        "names the clashing file: {files:?}"
    );

    // Nothing was written to the user's tree; the workspace now carries
    // conflict markers for the resolution agent…
    assert_eq!(
        std::fs::read_to_string(paths.root().join("app.rs")).unwrap(),
        "fn main() { /* user */ }\n",
        "conflicting approval must not touch the repository"
    );
    let ws = std::path::Path::new(&record.path);
    assert!(ws.is_dir(), "workspace kept for the resolution agent");
    assert!(
        std::fs::read_to_string(ws.join("app.rs"))
            .unwrap()
            .contains("<<<<<<<"),
        "markers staged for the agent"
    );

    // …which this very request spawned: the handler must drive the task
    // through RESOLVING_CONFLICTS -> EVALUATING and back to the human gate.
    // (The test agent is `true`, so it succeeds without editing anything.)
    let mut seen = db.require_task(&head).unwrap().status;
    for _ in 0..100 {
        if seen == TaskStatus::Review {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        seen = Database::open(&paths.state_db())
            .unwrap()
            .require_task(&head)
            .unwrap()
            .status;
    }
    assert_eq!(
        seen,
        TaskStatus::Review,
        "auto-resolution ran and returned the task to REVIEW"
    );
}

#[tokio::test]
async fn approving_a_non_review_task_is_409() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await; // still TODO

    let (status, _) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/approve"),
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "human-in-the-loop gate rejects approving a TODO task"
    );
}

#[tokio::test]
async fn reject_appends_feedback_and_sends_task_back() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await;
    force_to_review(&paths, &head);

    let (status, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/reject"),
        Some(json!({ "feedback": "handle the edge case" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], json!("TODO"), "work goes back for retry");
    assert!(
        body["description"]
            .as_str()
            .unwrap()
            .contains("handle the edge case"),
        "feedback recorded on the task"
    );
}

#[tokio::test]
async fn reject_of_non_review_task_is_409() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await; // TODO

    let (status, _) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/reject"),
        Some(json!({ "feedback": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn reject_frees_a_parked_task_and_resets_its_retry_budget() {
    // Phase 8: FAILED_NEEDS_HUMAN is the human's parking spot; rejecting it
    // sends it back to the queue with a fresh 3-failure budget.
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await;
    force_to_failed(&paths, &head);

    let (status, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/reject"),
        Some(json!({ "feedback": "install the missing dependency" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], json!("TODO"));
    assert_eq!(
        body["retry_count"],
        json!(0),
        "budget reset for the human cycle"
    );
    assert!(body["description"]
        .as_str()
        .unwrap()
        .contains("install the missing dependency"));

    // A parked task with EMPTY feedback is still freed.
    force_to_failed(&paths, &head);
    let (status, body) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/reject"),
        Some(json!({ "feedback": "" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["status"], json!("TODO"));
    assert_eq!(body["retry_count"], json!(0));
}

#[tokio::test]
async fn rejecting_a_mid_flight_task_is_still_409() {
    // EVALUATING/IN_PROGRESS are owned by a live worker; only REVIEW and
    // FAILED_NEEDS_HUMAN are human-rejectable.
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await;
    let db = Database::open(&paths.state_db()).unwrap();
    db.set_status(&head, TaskStatus::InProgress).unwrap();
    db.set_status(&head, TaskStatus::Evaluating).unwrap();

    let (status, _) = json_req(
        &app,
        Method::POST,
        &format!("/api/tasks/{head}/reject"),
        Some(json!({ "feedback": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn run_ready_starts_the_head_task() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let head = plan_head(&app).await;

    let (status, body) = json_req(&app, Method::POST, "/api/run", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["started"], json!(1), "only the TODO head runs");

    // Give the worker a moment to claim and finish (agent is `true`).
    let db = Database::open(&paths.state_db()).unwrap();
    let mut seen = db.require_task(&head).unwrap().status;
    for _ in 0..50 {
        if seen == TaskStatus::Review {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        seen = Database::open(&paths.state_db())
            .unwrap()
            .require_task(&head)
            .unwrap()
            .status;
    }
    assert_eq!(seen, TaskStatus::Review, "agent success lands in REVIEW");
}

#[tokio::test]
async fn unknown_api_route_is_json_404_not_html() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    // A typo'd endpoint must NOT fall through to the HTML dashboard fallback
    // (which returns 200) — JSON clients would misread that as success.
    let (status, body) = json_req(&app, Method::GET, "/api/nope", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("unknown API endpoint"),
        "body: {body}"
    );

    // POST to an unknown API path is a 404 too.
    let (status, _) = json_req(&app, Method::POST, "/api/nope/here", Some(json!({}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_route_falls_back_to_placeholder_page() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let req = Request::builder()
        .uri("/definitely/not/a/route")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "placeholder served at /");
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8_lossy(&bytes);
    assert!(html.contains("zou is running"), "placeholder page");
}

#[tokio::test]
async fn websocket_delivers_snapshot_then_live_updates() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_tungstenite::tungstenite::Message;

    let (_tmp, paths) = scratch();
    // Bind a real server on an ephemeral port.
    let config = ServeConfig {
        paths: paths.clone(),
        host: "127.0.0.1".into(),
        port: 0,
        max_concurrency: 1,
        agent: zunesha_core::AgentCommand::shell("true"),
        dashboard_dir: Some(std::path::PathBuf::from("/nonexistent-zou-test-dashboard")),
    };
    let handle = zunesha_api::serve(config).await.unwrap();
    let addr = handle.address();

    let url = format!("ws://{addr}/ws");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let (mut write, mut read) = ws.split();

    // First message: a full snapshot (empty board).
    let first = read.next().await.unwrap().unwrap();
    let first = first.to_text().unwrap().to_string();
    let first: Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["type"], json!("snapshot"));
    assert!(first["tasks"].as_array().unwrap().is_empty());

    // Trigger a mutation over real HTTP against the *live* server, so the
    // event lands on the same broadcast channel the WebSocket is subscribed
    // to. (A separate `app_for` instance would have its own channel.)
    let body = r#"{"prompt":"live update"}"#;
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "POST /api/plan HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "plan via live server: {resp}"
    );

    // Read until we see a snapshot containing the new task.
    let mut saw_tasks = false;
    for _ in 0..20 {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), read.next())
            .await
            .expect("timed out waiting for ws message")
            .unwrap()
            .unwrap();
        let text = match msg {
            Message::Text(t) => t.to_string(),
            _ => continue,
        };
        let v: Value = serde_json::from_str(&text).unwrap();
        if v["type"] == json!("snapshot") && !v["tasks"].as_array().unwrap().is_empty() {
            saw_tasks = true;
            break;
        }
        if v["type"] == json!("task_updated") {
            saw_tasks = true;
            break;
        }
    }
    assert!(
        saw_tasks,
        "client received a live update after the mutation"
    );

    // Close the socket. `axum::serve` keeps listening regardless (it serves
    // many clients), so we deliberately *don't* await `handle.join()` — that
    // would block forever. Dropping the handle detaches the task and the test
    // runtime tears it down on exit.
    write.close().await.ok();
    drop(handle);
}

/// An explicitly configured dashboard directory wins, and when it contains
/// `index.html` the app serves it as the SPA fallback for `/`.
#[tokio::test]
async fn dashboard_dir_is_served_and_wins_over_discovery() {
    let (_tmp, paths) = scratch();
    let dash = tempfile::tempdir().unwrap();
    std::fs::write(dash.path().join("index.html"), "<h1>zou dashboard</h1>").unwrap();
    std::fs::write(dash.path().join("app.js"), "console.log('hi')").unwrap();

    let config = ServeConfig {
        paths: paths.clone(),
        host: "127.0.0.1".into(),
        port: 0,
        max_concurrency: 1,
        agent: zunesha_core::AgentCommand::shell("true"),
        dashboard_dir: Some(dash.path().to_path_buf()),
    };
    let (app, _state) = build_app(config);

    // `/` serves the exported index.html…
    let body = body_of(app.clone(), Method::GET, "/").await;
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("zou dashboard"), "got: {html}");

    // …assets resolve too…
    let body = body_of(app.clone(), Method::GET, "/app.js").await;
    assert_eq!(body, b"console.log('hi')".to_vec());

    // …unknown API paths still get JSON, not the SPA shell…
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/nope")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // …and a non-API unknown path falls back to index.html (SPA routes).
    let body = body_of(app, Method::GET, "/task/some-id").await;
    assert!(
        String::from_utf8_lossy(&body).contains("zou dashboard"),
        "SPA fallback serves index.html for client routes"
    );
}

/// A configured directory *without* `index.html` must not break the server:
/// the placeholder page shows instead.
#[tokio::test]
async fn missing_dashboard_falls_back_to_placeholder() {
    let (_tmp, paths) = scratch();
    let empty = tempfile::tempdir().unwrap();

    let config = ServeConfig {
        paths,
        host: "127.0.0.1".into(),
        port: 0,
        max_concurrency: 1,
        agent: zunesha_core::AgentCommand::shell("true"),
        dashboard_dir: Some(empty.path().to_path_buf()),
    };
    let (app, _state) = build_app(config);
    let body = body_of(app, Method::GET, "/").await;
    let html = String::from_utf8_lossy(&body);
    assert!(!html.contains("zou dashboard"));
    assert!(
        html.contains("dashboard"),
        "placeholder mentions the dashboard"
    );
}

/// Discovery honors an explicit path even when it has no index.html — the
/// caller (not discovery) decides whether to serve or show the placeholder.
#[test]
fn explicit_dashboard_dir_is_returned_verbatim() {
    let explicit = std::path::PathBuf::from("/definitely/not/here");
    assert_eq!(
        zunesha_api::find_dashboard_dir(Some(explicit.clone())),
        Some(explicit)
    );
}

// ---------------------------------------------------------------------------
// Crash recovery (`POST /api/recover`)
// ---------------------------------------------------------------------------

/// Insert a task directly and claim it for a worker that then "crashes".
fn stuck_task(paths: &ZuneshaPaths, title: &str) -> zunesha_core::Task {
    let db = Database::open(&paths.state_db()).unwrap();
    let task = zunesha_core::Task::new(format!("id-{title}"), title, "do the thing");
    db.insert_task(&task).unwrap();
    // Claim it the way the pool would: ready first, then running.
    db.set_status(&task.id, TaskStatus::Todo).unwrap();
    db.set_status(&task.id, TaskStatus::InProgress).unwrap();
    task
}

#[tokio::test]
async fn recover_releases_a_stuck_task() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let task = stuck_task(&paths, "crashed-one");

    let (status, body) = json_req(
        &app,
        Method::POST,
        "/api/recover",
        Some(json!({ "task_id": task.id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["released"], json!([task.id]));

    // A fresh read agrees the task is runnable again.
    let (status, task_json) =
        json_req(&app, Method::GET, &format!("/api/tasks/{}", task.id), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task_json["status"], json!("TODO"));
}

/// Phase 9: a daemon killed mid-auto-merge leaves a task in
/// `RESOLVING_CONFLICTS` — equally worker-owned, equally releasable.
#[tokio::test]
async fn recover_releases_an_interrupted_conflict_resolution() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let task = stuck_task(&paths, "crashed-resolver");

    // Walk it to RESOLVING_CONFLICTS the way the conflict path does.
    let db = Database::open(&paths.state_db()).unwrap();
    db.set_status(&task.id, TaskStatus::Evaluating).unwrap();
    db.set_status(&task.id, TaskStatus::Review).unwrap();
    db.set_status(&task.id, TaskStatus::ResolvingConflicts)
        .unwrap();

    let (status, body) = json_req(
        &app,
        Method::POST,
        "/api/recover",
        Some(json!({ "task_id": task.id })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["released"], json!([task.id]));

    let (_, task_json) =
        json_req(&app, Method::GET, &format!("/api/tasks/{}", task.id), None).await;
    assert_eq!(task_json["status"], json!("TODO"));
}

#[tokio::test]
async fn recover_without_a_body_releases_every_stuck_task() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);
    let a = stuck_task(&paths, "stuck a");
    let b = stuck_task(&paths, "stuck b");
    // An unrelated task must not be touched.
    let db = Database::open(&paths.state_db()).unwrap();
    let calm = zunesha_core::Task::new("id-calm", "calm", "");
    db.insert_task(&calm).unwrap();

    // `{}` is as valid as sending no body at all: release everything stuck.
    let (status, body) = json_req(&app, Method::POST, "/api/recover", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let mut released: Vec<String> = body["released"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    released.sort();
    assert_eq!(released, vec![a.id.clone(), b.id.clone()]);

    let (_, calm_json) = json_req(&app, Method::GET, "/api/tasks/id-calm", None).await;
    assert_eq!(calm_json["status"], json!("BACKLOG"));
}

#[tokio::test]
async fn recover_rejects_unknown_and_unstuck_tasks() {
    let (_tmp, paths) = scratch();
    let app = app_for(&paths);

    // Unknown id -> 404.
    let (status, body) = json_req(
        &app,
        Method::POST,
        "/api/recover",
        Some(json!({ "task_id": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body: {body}");

    // Existing but not stuck -> 409, and nothing changes.
    let db = Database::open(&paths.state_db()).unwrap();
    let done = zunesha_core::Task::new("id-done", "already done", "");
    db.insert_task(&done).unwrap();
    db.force_set_status(&done.id, TaskStatus::Done).unwrap();

    let (status, body) = json_req(
        &app,
        Method::POST,
        "/api/recover",
        Some(json!({ "task_id": done.id })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body: {body}");

    let (_, task_json) = json_req(&app, Method::GET, "/api/tasks/id-done", None).await;
    assert_eq!(task_json["status"], json!("DONE"), "untouched by the 409");
}

/// A WebSocket client that falls behind the broadcast channel must be
/// resynced with a fresh snapshot instead of being left with a silent gap.
#[tokio::test]
async fn websocket_resyncs_a_lagged_client_with_a_snapshot() {
    use futures_util::StreamExt;

    let (_tmp, paths) = scratch();
    let config = ServeConfig {
        paths: paths.clone(),
        host: "127.0.0.1".into(),
        port: 0,
        max_concurrency: 1,
        agent: zunesha_core::AgentCommand::shell("true"),
        dashboard_dir: Some(std::path::PathBuf::from("/nonexistent-zou-test-dashboard")),
    };
    let (app, state) = build_app(config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let (write, mut read) = ws.split();

    // First message: the initial snapshot.
    let first = read.next().await.unwrap().unwrap();
    let first: Value = serde_json::from_str(first.to_text().unwrap()).unwrap();
    assert_eq!(first["type"], json!("snapshot"));

    // Flood the broadcast channel well past its capacity (256) with large
    // log events while the client is deliberately not reading. The oversized
    // messages also fill the TCP buffers so the server stops draining the
    // channel — guaranteeing the receiver lags.
    let big = "x".repeat(128 * 1024);
    for i in 0..300 {
        let event = zunesha_api::events::ApiEvent::Log {
            task_id: format!("t{i}"),
            stream: zunesha_core::StreamKind::Stdout,
            line: big.clone(),
            seq: 0,
        };
        state.events.send(event).unwrap();
    }

    // The client must eventually see a snapshot again (after whatever log
    // lines were forwarded before the flood overran the ring).
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let msg = tokio::time::timeout_at(deadline, read.next())
            .await
            .expect("timed out waiting for the resync snapshot")
            .expect("stream ended early")
            .expect("websocket error");
        let ev: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        if ev["type"] == json!("snapshot") {
            assert!(ev["tasks"].as_array().unwrap().is_empty());
            break;
        }
        assert_eq!(ev["type"], json!("log"), "unexpected event before resync");
    }

    drop(write);
}
