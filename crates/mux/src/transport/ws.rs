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
use crate::upstream::bridge::{SubscriptionFlags, WsBridge};

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
    let bridge = get_or_create_bridge(&entry).await;
    let flags = SubscriptionFlags::parse(&subscribe);
    let (client_id, mut client_rx) = bridge.add_client(flags).await;
    let (mut ws_tx, mut ws_rx) = socket.split();

    loop {
        tokio::select! {
            _ = entry.cancel.cancelled() => break,

            // Bridge -> downstream client
            msg = client_rx.recv() => {
                match msg {
                    Some(text) => {
                        if ws_tx.send(Message::Text(text.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }

            // Downstream client -> bridge (upstream)
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        bridge.send_upstream(client_id, text.to_string());
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }

    bridge.remove_client(client_id).await;
}

/// Get the existing WS bridge or create a new one.
async fn get_or_create_bridge(entry: &Arc<SessionEntry>) -> Arc<WsBridge> {
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

    let bridge = WsBridge::connect(entry);
    *guard = Some(Arc::clone(&bridge));
    bridge
}
