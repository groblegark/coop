// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Aggregated WebSocket endpoint — fans out events from all sessions
//! to dashboard clients over a single `/ws/mux` connection.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::state::{MuxEvent, MuxState};
use crate::transport::auth;
use crate::upstream::client::UpstreamClient;

/// Query parameters for the aggregated mux WebSocket.
#[derive(Debug, Deserialize)]
pub struct MuxAggQuery {
    /// Auth token.
    pub token: Option<String>,
    /// Comma-separated session IDs, or "all" (default: "all").
    #[serde(default = "default_sessions")]
    pub sessions: String,
    /// Comma-separated event types: state,screen (default: all).
    #[serde(default = "default_subscribe")]
    pub subscribe: String,
}

fn default_sessions() -> String {
    "all".to_owned()
}
fn default_subscribe() -> String {
    "state,screen,credentials".to_owned()
}

/// Parsed subscription preferences.
struct MuxFlags {
    all_sessions: bool,
    session_filter: Vec<String>,
    state: bool,
    screen: bool,
    credentials: bool,
}

impl MuxFlags {
    fn parse(query: &MuxAggQuery) -> Self {
        let all_sessions = query.sessions == "all";
        let session_filter: Vec<String> = if all_sessions {
            vec![]
        } else {
            query.sessions.split(',').map(|s| s.trim().to_owned()).collect()
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
        Self { all_sessions, session_filter, state, screen, credentials }
    }

    fn wants_session(&self, session: &str) -> bool {
        self.all_sessions || self.session_filter.iter().any(|s| s == session)
    }

    fn wants_event(&self, event: &MuxEvent) -> bool {
        match event {
            MuxEvent::State { session, .. } => self.state && self.wants_session(session),
            MuxEvent::Screen { session, .. } => self.screen && self.wants_session(session),
            MuxEvent::Credential { session, .. } => {
                self.credentials && self.wants_session(session)
            }
            MuxEvent::SessionOnline { session, .. } | MuxEvent::SessionOffline { session, .. } => {
                self.wants_session(session)
            }
        }
    }
}

/// `GET /ws/mux` — WebSocket upgrade for aggregated session stream.
pub async fn ws_mux_handler(
    State(state): State<Arc<MuxState>>,
    Query(query): Query<MuxAggQuery>,
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

    let flags = MuxFlags::parse(&query);
    ws.on_upgrade(move |socket| handle_mux_connection(state, flags, socket))
        .into_response()
}

/// Per-connection event loop for aggregated mux clients.
async fn handle_mux_connection(state: Arc<MuxState>, flags: MuxFlags, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    let mut mux_rx = state.aggregator.subscribe();

    // Send initial cached state for all matching sessions.
    let cached = state.aggregator.cached_state().await;
    for (session_id, cache) in &cached {
        if !flags.wants_session(session_id) {
            continue;
        }
        // Send cached agent state.
        if flags.state {
            if let Some(ref agent_state) = cache.agent_state {
                let evt = MuxEvent::State {
                    session: session_id.clone(),
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
                    session: session_id.clone(),
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

    // Event loop: forward aggregated events + handle client input.
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
                        handle_client_input(&state, &text).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

/// Handle a text message from a downstream mux client.
/// Input messages target a specific session and are proxied via HTTP.
async fn handle_client_input(state: &MuxState, text: &str) {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    let event = msg.get("event").and_then(|v| v.as_str()).unwrap_or_default();
    let session_id = msg.get("session").and_then(|v| v.as_str()).unwrap_or_default();
    if session_id.is_empty() {
        return;
    }

    // Look up session.
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(session_id) {
        Some(e) => Arc::clone(e),
        None => return,
    };
    drop(sessions);

    let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());

    let result = match event {
        "input:send" => client.post_json("/api/v1/input", &msg).await,
        "input:send:raw" => client.post_json("/api/v1/input/raw", &msg).await,
        "keys:send" => client.post_json("/api/v1/input/keys", &msg).await,
        "nudge" => client.post_json("/api/v1/agent/nudge", &msg).await,
        "respond" => client.post_json("/api/v1/agent/respond", &msg).await,
        "signal:send" => client.post_json("/api/v1/signal", &msg).await,
        "resize" => client.post_json("/api/v1/resize", &msg).await,
        _ => return,
    };

    if let Err(e) = result {
        tracing::debug!(session_id, event, err = %e, "mux input proxy failed");
    }
}
