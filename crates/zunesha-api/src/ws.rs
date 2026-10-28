//! The WebSocket bridge the dashboard uses for live updates.
//!
//! A client that connects to `/ws` immediately receives a full
//! [`crate::events::ApiEvent::Snapshot`] so it can render the board without a
//! separate REST round-trip. From then on it receives every subsequent
//! [`crate::events::ApiEvent`] — task updates and streaming agent logs —
//! pushed from the shared broadcast channel.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast::error::RecvError;

use zunesha_core::Database;

use crate::events::snapshot_event;
use crate::AppState;

/// Upgrade an HTTP request into the live event stream.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Serve one connected client until it disconnects or the stream closes.
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe *before* building the snapshot so no event is missed between
    // the two steps; any event already sent will simply be re-sent as a fresh
    // TaskUpdated right after the client renders the snapshot.
    let mut events = state.events.subscribe();

    // Send the initial full state so the client never renders an empty board.
    {
        let snapshot = match Database::open(&state.paths.state_db()) {
            Ok(db) => snapshot_event(&db, &state.paths).ok(),
            Err(_) => None,
        };
        if let Some(event) = snapshot {
            if let Ok(text) = serde_json::to_string(&event) {
                if sender.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        tokio::select! {
            // Push server -> client events.
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    // Lagged: the client fell behind the channel capacity
                    // (e.g. an agent flooding log lines). Resync it with a
                    // fresh full-state snapshot instead of leaving a silent
                    // gap — the reducer merges snapshots idempotently, and
                    // persisted `worker_logs` (keyed by `seq`) fill in any
                    // log lines that were skipped in transit.
                    Err(RecvError::Lagged(_)) => {
                        match Database::open(&state.paths.state_db())
                            .ok()
                            .and_then(|db| snapshot_event(&db, &state.paths).ok())
                        {
                            Some(snapshot) => snapshot,
                            None => continue,
                        }
                    }
                    // The sender was dropped: the server is shutting down.
                    Err(RecvError::Closed) => break,
                };
                match serde_json::to_string(&event) {
                    Ok(text) => {
                        if sender.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    // A serialization failure for one event shouldn't kill the
                    // connection; drop that event and keep streaming.
                    Err(_) => continue,
                }
            }

            // Drain client -> server messages so we notice disconnects and
            // pings. The dashboard doesn't send meaningful payloads today, but
            // the socket must still be read to detect closure.
            incoming = receiver.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                }
            }
        }
    }
}
