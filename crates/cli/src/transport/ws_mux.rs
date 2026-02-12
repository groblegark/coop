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

use crate::broker::mux::MuxEvent;
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

/// Parsed subscription preferences.
struct MuxFlags {
    all_pods: bool,
    pod_filter: Vec<String>,
    state: bool,
    screen: bool,
    credentials: bool,
}

impl MuxFlags {
    fn parse(query: &MuxQuery) -> Self {
        let all_pods = query.pods == "all";
        let pod_filter: Vec<String> = if all_pods {
            vec![]
        } else {
            query.pods.split(',').map(|s| s.trim().to_owned()).collect()
        };
        let mut state = false;
        let mut screen = false;
        let mut credentials = false;
        for token in query.subscribe.split(',') {
            match token.trim() {
                "state" => state = true,
                "screen" => screen = true,
                "credentials" => credentials = true,
                _ => {}
            }
        }
        Self { all_pods, pod_filter, state, screen, credentials }
    }

    fn wants_pod(&self, pod: &str) -> bool {
        self.all_pods || self.pod_filter.iter().any(|p| p == pod)
    }

    fn wants_event(&self, event: &MuxEvent) -> bool {
        match event {
            MuxEvent::State { pod, .. } => self.state && self.wants_pod(pod),
            MuxEvent::Screen { pod, .. } => self.screen && self.wants_pod(pod),
            MuxEvent::Credential { pod, .. } => self.credentials && self.wants_pod(pod),
            MuxEvent::PodOnline { pod, .. } => self.wants_pod(pod),
            MuxEvent::PodOffline { pod, .. } => self.wants_pod(pod),
        }
    }
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
    let flags = MuxFlags::parse(&query);
    ws.on_upgrade(move |socket| handle_mux_connection(state, flags, socket)).into_response()
}

/// Per-connection event loop for multiplexed clients.
async fn handle_mux_connection(state: Arc<Store>, flags: MuxFlags, socket: WebSocket) {
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
        for (pod_name, cache) in &cached {
            if !flags.wants_pod(pod_name) {
                continue;
            }
            // Send cached agent state.
            if flags.state {
                if let Some(ref agent_state) = cache.agent_state {
                    let evt = MuxEvent::State {
                        pod: pod_name.clone(),
                        prev: String::new(),
                        next: agent_state.clone(),
                        seq: 0,
                    };
                    if let Ok(json) = serde_json::to_string(&evt) {
                        if ws_tx.send(Message::Text(json.into())).await.is_err() {
                            return;
                        }
                    }
                }
            }
            // Send cached screen.
            if flags.screen {
                if let Some(ref lines) = cache.screen_lines {
                    let evt = MuxEvent::Screen {
                        pod: pod_name.clone(),
                        lines: lines.clone(),
                        cols: cache.screen_cols,
                        rows: cache.screen_rows,
                    };
                    if let Ok(json) = serde_json::to_string(&evt) {
                        if ws_tx.send(Message::Text(json.into())).await.is_err() {
                            return;
                        }
                    }
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
                if flags.wants_event(&event) {
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
                        // Handle input routing: { "event": "input:send", "pod": "X", "text": "..." }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if json.get("event").and_then(|e| e.as_str()) == Some("input:send") {
                                if let (Some(pod), Some(input_text)) = (
                                    json.get("pod").and_then(|p| p.as_str()),
                                    json.get("text").and_then(|t| t.as_str()),
                                ) {
                                    // Proxy input to the pod's coop HTTP API.
                                    proxy_input_to_pod(&state, pod, input_text).await;
                                }
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
