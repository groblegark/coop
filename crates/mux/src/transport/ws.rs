// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Downstream WebSocket handler for mux clients.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::state::{MuxState, SessionEntry};
use crate::transport::auth;
use crate::upstream::client::UpstreamClient;
use crate::upstream::bridge::WsBridge;

/// Query parameters for downstream WS upgrade.
#[derive(Debug, Clone, Deserialize)]
pub struct MuxWsQuery {
    pub token: Option<String>,
    /// Comma-separated upstream subscription flags (e.g. `pty,screen,state`).
    #[serde(default = "default_subscribe")]
    pub subscribe: String,
}

fn default_subscribe() -> String {
    "pty,screen,state".to_owned()
}

/// `GET /ws/{session_id}` — WebSocket upgrade for a mux session.
pub async fn ws_handler(
    State(state): State<Arc<MuxState>>,
    Path(session_id): Path<String>,
    Query(query): Query<MuxWsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate auth via query param.
    let query_str = query.token.as_ref().map(|t| format!("token={t}")).unwrap_or_default();
    if let Err(_code) = auth::validate_ws_query(&query_str, state.config.auth_token.as_deref()) {
        return axum::http::Response::builder()
            .status(401)
            .body(axum::body::Body::from("unauthorized"))
            .unwrap_or_default()
            .into_response();
    }

    // Look up session.
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(&session_id) {
        Some(e) => Arc::clone(e),
        None => {
            return axum::http::Response::builder()
                .status(404)
                .body(axum::body::Body::from("session not found"))
                .unwrap_or_default()
                .into_response();
        }
    };
    drop(sessions);

    let subscribe = query.subscribe.clone();

    ws.on_upgrade(move |socket| handle_ws(socket, entry, subscribe)).into_response()
}

/// Per-connection WebSocket handler.
async fn handle_ws(socket: WebSocket, entry: Arc<SessionEntry>, subscribe: String) {
    // Get or create the WS bridge for this session.
    let bridge = get_or_create_bridge(&entry, &subscribe).await;

    let mut rx = bridge.tx.subscribe();
    let (mut ws_tx, mut ws_rx) = socket.split();

    loop {
        tokio::select! {
            _ = entry.cancel.cancelled() => break,

            // Forward upstream messages to downstream client.
            msg = rx.recv() => {
                match msg {
                    Ok(text) => {
                        if ws_tx.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(lagged = n, "downstream WS client lagged, skipping");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }

            // Handle messages from downstream client.
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Try to parse as input command and proxy via HTTP.
                        handle_client_input(&entry, &text).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Get the existing WS bridge or create a new one.
async fn get_or_create_bridge(entry: &Arc<SessionEntry>, subscribe: &str) -> Arc<WsBridge> {
    {
        let guard = entry.ws_bridge.read().await;
        if let Some(ref bridge) = *guard {
            return Arc::clone(bridge);
        }
    }

    let mut guard = entry.ws_bridge.write().await;
    // Double-check after acquiring write lock.
    if let Some(ref bridge) = *guard {
        return Arc::clone(bridge);
    }

    let bridge = WsBridge::connect(entry, subscribe);
    *guard = Some(Arc::clone(&bridge));
    bridge
}

/// Handle a text message from a downstream WS client.
///
/// Input messages are proxied to upstream via HTTP POST (not through the shared
/// upstream WS). This avoids input multiplexing complexity.
async fn handle_client_input(entry: &SessionEntry, text: &str) {
    // Parse the message to determine what kind of input it is.
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    let event = msg.get("event").and_then(|v| v.as_str()).unwrap_or_default();
    let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());

    let result = match event {
        "input:send" => client.post_json("/api/v1/input", &msg).await,
        "input:send:raw" => client.post_json("/api/v1/input/raw", &msg).await,
        "keys:send" => client.post_json("/api/v1/input/keys", &msg).await,
        "nudge" => client.post_json("/api/v1/agent/nudge", &msg).await,
        "respond" => client.post_json("/api/v1/agent/respond", &msg).await,
        "signal:send" => client.post_json("/api/v1/signal", &msg).await,
        "resize" => client.post_json("/api/v1/resize", &msg).await,
        _ => return, // Unknown event type — ignore.
    };

    if let Err(e) = result {
        tracing::debug!(session_id = %entry.id, event, err = %e, "WS input proxy failed");
    }
}
