// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Multiplexed WebSocket endpoint — fans out MuxEvents from all pods
//! to dashboard clients over a single connection.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use coop_mux::events::{backfill_events, MuxFilter};
use crate::transport::state::Store;

/// Query parameters for the mux WebSocket.
#[derive(Debug, Deserialize)]
pub struct MuxQuery {
    /// Auth token.
    pub token: Option<String>,
    /// Comma-separated pod names, or "all" (default: "all").
    #[serde(default = "default_pods")]
    pub pods: String,
    /// Comma-separated event types: state,screen,credentials (default: all).
    #[serde(default = "default_subscribe")]
    pub subscribe: String,
}

fn default_pods() -> String {
    "all".to_owned()
}
fn default_subscribe() -> String {
    "state,screen,credentials".to_owned()
}

/// WebSocket upgrade handler for /ws/mux.
pub async fn ws_mux_handler(
    State(state): State<Arc<Store>>,
    Query(query): Query<MuxQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate token from query param (mirrors /ws handler pattern).
    if let Some(ref expected) = state.config.auth_token {
        match &query.token {
            Some(tok) if crate::transport::auth::constant_time_eq(tok, expected) => {}
            _ => {
                return (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(serde_json::json!({"error":"unauthorized"})),
                )
                    .into_response();
            }
        }
    }
    let filter = MuxFilter::new(&query.pods, &query.subscribe);
    ws.on_upgrade(move |socket| handle_mux_connection(state, filter, socket)).into_response()
}

/// Per-connection event loop for multiplexed clients.
async fn handle_mux_connection(state: Arc<Store>, filter: MuxFilter, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Get a MuxEvent receiver from the multiplexer (if available).
    let mux = state.multiplexer.as_ref().map(|m| m.subscribe());

    // If no multiplexer available, send error and close.
    let Some(mut mux_rx) = mux else {
        let err = serde_json::json!({
            "event": "error",
            "message": "multiplexer not available (broker mode not enabled)"
        });
        let _ = ws_tx.send(Message::Text(err.to_string().into())).await;
        return;
    };

    // Send initial cached state for all subscribed pods.
    if let Some(ref mux) = state.multiplexer {
        let cached = mux.cached_state().await;
        for evt in backfill_events(&cached, &filter) {
            if let Ok(json) = serde_json::to_string(&evt) {
                if ws_tx.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    loop {
        tokio::select! {
            event = mux_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if filter.wants_event(&event) {
                    if let Ok(json) = serde_json::to_string(&event) {
                        if ws_tx.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            match json.get("event").and_then(|e| e.as_str()) {
                                // Handle input routing: { "event": "input:send", "pod": "X", "text": "..." }
                                Some("input:send") => {
                                    if let (Some(pod), Some(input_text)) = (
                                        json.get("pod").and_then(|p| p.as_str()),
                                        json.get("text").and_then(|t| t.as_str()),
                                    ) {
                                        proxy_input_to_pod(&state, pod, input_text).await;
                                    }
                                }
                                // Handle resize: { "event": "input:resize", "pod": "X", "cols": N, "rows": N }
                                Some("input:resize") => {
                                    if let (Some(pod), Some(cols), Some(rows)) = (
                                        json.get("pod").and_then(|p| p.as_str()),
                                        json.get("cols").and_then(|c| c.as_u64()),
                                        json.get("rows").and_then(|r| r.as_u64()),
                                    ) {
                                        proxy_resize_to_pod(&state, pod, cols as u16, rows as u16).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Proxy input to a specific pod via its coop HTTP API.
async fn proxy_input_to_pod(state: &Store, pod_name: &str, text: &str) {
    let Some(ref registry) = state.broker_registry else { return };
    let pods = registry.list().await;
    let Some(pod) = pods.iter().find(|p| p.name == pod_name) else { return };

    let url = format!("{}/api/v1/input", pod.coop_url);
    let client = reqwest::Client::new();
    let _ =
        client.post(&url).json(&serde_json::json!({ "text": text, "enter": true })).send().await;
}

/// Proxy a terminal resize to a specific pod via its coop HTTP API.
async fn proxy_resize_to_pod(state: &Store, pod_name: &str, cols: u16, rows: u16) {
    let Some(ref registry) = state.broker_registry else { return };
    let pods = registry.list().await;
    let Some(pod) = pods.iter().find(|p| p.name == pod_name) else { return };

    let url = format!("{}/api/v1/resize", pod.coop_url);
    let client = reqwest::Client::new();
    let _ = client.post(&url).json(&serde_json::json!({ "cols": cols, "rows": rows })).send().await;
}
