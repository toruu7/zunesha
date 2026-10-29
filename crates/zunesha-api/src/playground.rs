//! Compact, read-only projection of the task board for lightweight office
//! clients.
//!
//! The interactive `/playground/` page uses the full `/api/tasks` + `/ws`
//! protocol so it can inspect logs, diffs, and final output. This endpoint is
//! kept for small monitors that only need a stable id, a label, a lifecycle
//! status, and a visual state; it avoids shipping source diffs and
//! transcripts to a page whose only job is to show who is working, waiting,
//! or asleep.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;

use zunesha_core::{Database, Task, TaskStatus, ZuneshaPaths};

use crate::events::{project_info, zero_filled_counts, ProjectInfo, StatusCount};
use crate::AppState;

/// One agent-sized view of a task.
#[derive(Clone, Debug, Serialize)]
pub struct PlaygroundAgent {
    pub id: String,
    /// Display name used by the office labels.
    pub name: String,
    /// Full task title, shown in the compact detail panel.
    pub title: String,
    /// Human-readable lifecycle status (`IN_PROGRESS`, `REVIEW`, …).
    pub status: String,
    /// Renderer state: `working`, `waiting`, `idle`, `sleeping`, `done`, or
    /// `error`.
    pub state: String,
    pub updated_at: String,
    pub retry_count: u32,
}

/// Full playground state. `GET /api/playground` and the first WebSocket frame
/// both use this shape.
#[derive(Clone, Debug, Serialize)]
pub struct PlaygroundSnapshot {
    pub project: ProjectInfo,
    pub agents: Vec<PlaygroundAgent>,
    pub counts: Vec<StatusCount>,
    pub running: usize,
}

/// Live playground events. Logs and full task payloads are deliberately absent.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlaygroundEvent {
    Snapshot {
        project: ProjectInfo,
        agents: Vec<PlaygroundAgent>,
        counts: Vec<StatusCount>,
        running: usize,
    },
    AgentUpdated {
        agent: PlaygroundAgent,
    },
}

/// `GET /api/playground` – compact board projection.
pub async fn snapshot(
    State(state): State<AppState>,
) -> Result<Json<PlaygroundSnapshot>, (StatusCode, String)> {
    let paths = state.paths.clone();
    tokio::task::spawn_blocking(move || snapshot_from_paths(&paths))
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

/// `GET /api/playground/ws` – live compact events.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();

    // Subscribe before the initial snapshot so a task change cannot fall into
    // the gap between the two. Duplicate updates are harmless to the client.
    if let Ok(snapshot) = snapshot_from_paths(&state.paths) {
        if let Ok(text) = serde_json::to_string(&snapshot_event(snapshot)) {
            if sender.send(Message::Text(text.into())).await.is_err() {
                return;
            }
        }
    }

    loop {
        tokio::select! {
            event = events.recv() => {
                let event = match event {
                    Ok(event) => match event_from_api(event) {
                        Some(event) => event,
                        None => continue,
                    },
                    Err(RecvError::Lagged(_)) => {
                        match snapshot_from_paths(&state.paths) {
                            Ok(snapshot) => snapshot_event(snapshot),
                            Err(_) => continue,
                        }
                    }
                    Err(RecvError::Closed) => break,
                };
                match serde_json::to_string(&event) {
                    Ok(text) => {
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => continue,
                }
            }
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                }
            }
        }
    }
}

fn snapshot_event(snapshot: PlaygroundSnapshot) -> PlaygroundEvent {
    PlaygroundEvent::Snapshot {
        project: snapshot.project,
        agents: snapshot.agents,
        counts: snapshot.counts,
        running: snapshot.running,
    }
}

fn event_from_api(event: crate::events::ApiEvent) -> Option<PlaygroundEvent> {
    match event {
        crate::events::ApiEvent::Snapshot {
            project,
            tasks,
            counts,
            ..
        } => {
            let agents: Vec<PlaygroundAgent> = tasks.iter().map(map_task).collect();
            let running = agents
                .iter()
                .filter(|agent| agent.state == "working")
                .count();
            Some(PlaygroundEvent::Snapshot {
                project,
                agents,
                counts,
                running,
            })
        }
        crate::events::ApiEvent::TaskUpdated { task } => Some(PlaygroundEvent::AgentUpdated {
            agent: map_task(&task),
        }),
        // The playground intentionally does not stream transcripts.
        crate::events::ApiEvent::Log { .. } => None,
    }
}

fn snapshot_from_paths(paths: &ZuneshaPaths) -> zunesha_core::Result<PlaygroundSnapshot> {
    let db = Database::open(&paths.state_db())?;
    let tasks = db.list_tasks()?;
    let agents: Vec<PlaygroundAgent> = tasks.iter().map(map_task).collect();
    let running = agents
        .iter()
        .filter(|agent| agent.state == "working")
        .count();
    Ok(PlaygroundSnapshot {
        project: project_info(paths),
        agents,
        counts: zero_filled_counts(&db)?,
        running,
    })
}

/// Project one task into the renderer's vocabulary.
pub fn map_task(task: &Task) -> PlaygroundAgent {
    PlaygroundAgent {
        id: task.id.clone(),
        name: task.title.clone(),
        title: task.title.clone(),
        status: status_label(task.status).to_string(),
        state: visual_state(task.status).to_string(),
        updated_at: task.updated_at.clone(),
        retry_count: task.retry_count,
    }
}

fn visual_state(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "sleeping",
        TaskStatus::Todo => "idle",
        TaskStatus::InProgress | TaskStatus::Evaluating | TaskStatus::ResolvingConflicts => {
            "working"
        }
        TaskStatus::Review => "waiting",
        TaskStatus::Done => "done",
        TaskStatus::FailedNeedsHuman => "error",
    }
}

fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Backlog => "BACKLOG",
        TaskStatus::Todo => "TODO",
        TaskStatus::InProgress => "IN_PROGRESS",
        TaskStatus::Evaluating => "EVALUATING",
        TaskStatus::Review => "REVIEW",
        TaskStatus::ResolvingConflicts => "RESOLVING_CONFLICTS",
        TaskStatus::Done => "DONE",
        TaskStatus::FailedNeedsHuman => "FAILED_NEEDS_HUMAN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: TaskStatus) -> Task {
        Task {
            id: "task-1".into(),
            title: "Ship the playground".into(),
            description: String::new(),
            status,
            retry_count: 0,
            worker_logs: "secret transcript".into(),
            agent_output: "secret output".into(),
            diff: "secret diff".into(),
            created_at: "2026-10-28 00:00:00".into(),
            updated_at: "2026-10-28 00:00:01".into(),
        }
    }

    #[test]
    fn maps_lifecycle_states_to_office_states() {
        assert_eq!(map_task(&task(TaskStatus::Backlog)).state, "sleeping");
        assert_eq!(map_task(&task(TaskStatus::Todo)).state, "idle");
        assert_eq!(map_task(&task(TaskStatus::InProgress)).state, "working");
        assert_eq!(map_task(&task(TaskStatus::Evaluating)).state, "working");
        assert_eq!(
            map_task(&task(TaskStatus::ResolvingConflicts)).state,
            "working"
        );
        assert_eq!(map_task(&task(TaskStatus::Review)).state, "waiting");
        assert_eq!(map_task(&task(TaskStatus::Done)).state, "done");
        assert_eq!(map_task(&task(TaskStatus::FailedNeedsHuman)).state, "error");
    }

    #[test]
    fn projection_does_not_include_logs_or_diffs() {
        let json = serde_json::to_string(&map_task(&task(TaskStatus::InProgress))).unwrap();
        assert!(!json.contains("secret transcript"));
        assert!(!json.contains("secret output"));
        assert!(!json.contains("secret diff"));
    }
}
