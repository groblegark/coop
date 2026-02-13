// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! `/ws/mux` multiplexed WebSocket handler.
//!
//! Tracks which sessions each client subscribes to, manages feed + poller
//! lifecycle, sends cached screen snapshots on connect, pushes state
//! transitions immediately + screen thumbnails periodically.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::state::{MuxEvent, MuxState, WatcherState};
use crate::transport::auth;
use crate::upstream::feed::spawn_event_feed;
use crate::upstream::poller::spawn_screen_poller;

/// Query parameters for `/ws/mux`.
#[derive(Debug, Deserialize)]
pub struct MuxWsQuery {
    /// Auth token (query-param auth for WebSocket).
    pub token: Option<String>,
}

/// Client → server messages on `/ws/mux`.
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum MuxClientMessage {
    /// Subscribe to events from specific sessions.
    Subscribe { sessions: Vec<String> },
    /// Unsubscribe from sessions.
    Unsubscribe { sessions: Vec<String> },
    /// Forward keyboard input to a session.
    #[serde(rename = "input:send")]
    InputSend {
        session: String,
        text: String,
        #[serde(default)]
        enter: bool,
    },
    /// Resize a session's terminal.
    Resize { session: String, cols: u16, rows: u16 },
}

/// Server → client messages on `/ws/mux`.
///
/// Uses manual serialization because `MuxEvent` uses its own `#[serde(tag)]`
/// and cannot be nested as `#[serde(untagged)]` inside a tagged enum.
#[derive(Debug)]
enum MuxServerMessage {
    /// Session list on connect.
    Sessions { sessions: Vec<SessionSnapshot> },
    /// An event from a watched session (serialized directly as its own tagged JSON).
    Event(MuxEvent),
    /// Periodic screen thumbnail batch.
    ScreenBatch { screens: Vec<ScreenThumbnail> },
    /// Error.
    Error { message: String },
}

impl serde::Serialize for MuxServerMessage {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Sessions { sessions } => {
                #[derive(Serialize)]
                struct Msg<'a> {
                    event: &'static str,
                    sessions: &'a [SessionSnapshot],
                }
                Msg { event: "sessions", sessions }.serialize(serializer)
            }
            Self::Event(mux_event) => mux_event.serialize(serializer),
            Self::ScreenBatch { screens } => {
                #[derive(Serialize)]
                struct Msg<'a> {
                    event: &'static str,
                    screens: &'a [ScreenThumbnail],
                }
                Msg { event: "screen_batch", screens }.serialize(serializer)
            }
            Self::Error { message } => {
                #[derive(Serialize)]
                struct Msg<'a> {
                    event: &'static str,
                    message: &'a str,
                }
                Msg { event: "error", message }.serialize(serializer)
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionSnapshot {
    id: String,
    url: String,
    state: Option<String>,
    metadata: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct ScreenThumbnail {
    session: String,
    lines: Vec<String>,
    ansi: Vec<String>,
    cols: u16,
    rows: u16,
    seq: u64,
}

/// WebSocket upgrade handler for `/ws/mux`.
pub async fn ws_mux_handler(
    State(state): State<Arc<MuxState>>,
    Query(query): Query<MuxWsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate auth.
    if state.config.auth_token.is_some() {
        if let Some(ref token) = query.token {
            if auth::validate_ws_query(token, state.config.auth_token.as_deref()).is_err() {
                return axum::http::Response::builder()
                    .status(401)
                    .body(axum::body::Body::from("unauthorized"))
                    .unwrap_or_default()
                    .into_response();
            }
        } else {
            return axum::http::Response::builder()
                .status(401)
                .body(axum::body::Body::from("token required"))
                .unwrap_or_default()
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_mux_ws(state, socket)).into_response()
}

/// Per-connection handler for `/ws/mux`.
async fn handle_mux_ws(state: Arc<MuxState>, socket: WebSocket) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut event_rx = state.feed.event_tx.subscribe();

    // Track which sessions this client is watching.
    let mut watched: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Send initial session list.
    {
        let sessions = state.sessions.read().await;
        let mut snapshots = Vec::with_capacity(sessions.len());
        for entry in sessions.values() {
            let cached_state = entry.cached_status.read().await.as_ref().map(|s| s.state.clone());
            snapshots.push(SessionSnapshot {
                id: entry.id.clone(),
                url: entry.url.clone(),
                state: cached_state,
                metadata: entry.metadata.clone(),
            });
        }
        let msg = MuxServerMessage::Sessions { sessions: snapshots };
        if send_json(&mut ws_tx, &msg).await.is_err() {
            return;
        }
    }

    // Screen thumbnail push interval (1 Hz).
    let mut screen_interval = tokio::time::interval(std::time::Duration::from_secs(1));
    screen_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Mux events (state transitions, online/offline).
            event = event_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                };
                // Forward session lifecycle and credential events to all clients;
                // state transitions only for watched sessions.
                let should_forward = match &event {
                    MuxEvent::CredentialRefreshed { .. }
                    | MuxEvent::CredentialRefreshFailed { .. }
                    | MuxEvent::CredentialReauthRequired { .. }
                    | MuxEvent::SessionOnline { .. }
                    | MuxEvent::SessionOffline { .. } => true,
                    MuxEvent::State { session, .. } => watched.contains(session),
                };
                if should_forward {
                    let msg = MuxServerMessage::Event(event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }

            // Screen thumbnail batch.
            _ = screen_interval.tick() => {
                if watched.is_empty() {
                    continue;
                }
                let sessions = state.sessions.read().await;
                let mut screens = Vec::new();
                for session_id in &watched {
                    if let Some(entry) = sessions.get(session_id) {
                        if let Some(screen) = entry.cached_screen.read().await.as_ref() {
                            screens.push(ScreenThumbnail {
                                session: session_id.clone(),
                                lines: screen.lines.clone(),
                                ansi: screen.ansi.clone(),
                                cols: screen.cols,
                                rows: screen.rows,
                                seq: screen.seq,
                            });
                        }
                    }
                }
                if !screens.is_empty() {
                    let msg = MuxServerMessage::ScreenBatch { screens };
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }

            // Client messages.
            msg = ws_rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break,
                };
                match msg {
                    Message::Text(text) => {
                        if let Ok(client_msg) = serde_json::from_str::<MuxClientMessage>(&text) {
                            match client_msg {
                                MuxClientMessage::Subscribe { sessions } => {
                                    let mut new_sids = Vec::new();
                                    for sid in sessions {
                                        if watched.insert(sid.clone()) {
                                            start_watching(&state, &sid).await;
                                            new_sids.push(sid);
                                        }
                                    }
                                    // Send immediate screen_batch for newly subscribed sessions
                                    // so clients don't wait for the next periodic tick.
                                    if !new_sids.is_empty() {
                                        let sessions_lock = state.sessions.read().await;
                                        let mut screens = Vec::new();
                                        for sid in &new_sids {
                                            if let Some(entry) = sessions_lock.get(sid) {
                                                if let Some(screen) = entry.cached_screen.read().await.as_ref() {
                                                    screens.push(ScreenThumbnail {
                                                        session: sid.clone(),
                                                        lines: screen.lines.clone(),
                                                        ansi: screen.ansi.clone(),
                                                        cols: screen.cols,
                                                        rows: screen.rows,
                                                        seq: screen.seq,
                                                    });
                                                }
                                            }
                                        }
                                        drop(sessions_lock);
                                        if !screens.is_empty() {
                                            let msg = MuxServerMessage::ScreenBatch { screens };
                                            if send_json(&mut ws_tx, &msg).await.is_err() {
                                                break;
                                            }
                                        }
                                    }
                                }
                                MuxClientMessage::Unsubscribe { sessions } => {
                                    for sid in sessions {
                                        if watched.remove(&sid) {
                                            stop_watching(&state, &sid).await;
                                        }
                                    }
                                }
                                MuxClientMessage::InputSend { session, text, enter } => {
                                    proxy_input(&state, &session, &text, enter).await;
                                }
                                MuxClientMessage::Resize { session, cols, rows } => {
                                    proxy_resize(&state, &session, cols, rows).await;
                                }
                            }
                        } else {
                            let err = MuxServerMessage::Error { message: "invalid message".to_owned() };
                            if send_json(&mut ws_tx, &err).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    // Cleanup: stop watching all sessions.
    for sid in &watched {
        stop_watching(&state, sid).await;
    }
}

/// Increment watcher count for a session, starting the event feed if needed.
async fn start_watching(state: &MuxState, session_id: &str) {
    let mut watchers = state.feed.watchers.write().await;
    if let Some(ws) = watchers.get_mut(session_id) {
        ws.count += 1;
        return;
    }

    // Start a new event feed and pollers for this session.
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(session_id) {
        Some(e) => Arc::clone(e),
        None => return,
    };
    drop(sessions);

    let feed_cancel = CancellationToken::new();
    spawn_event_feed(state.feed.event_tx.clone(), Arc::clone(&entry), feed_cancel.clone());

    let poller_cancel = CancellationToken::new();
    spawn_screen_poller(entry, &state.config, poller_cancel.clone());

    watchers.insert(session_id.to_owned(), WatcherState { count: 1, feed_cancel, poller_cancel });
}

/// Decrement watcher count for a session, stopping the event feed when 0.
async fn stop_watching(state: &MuxState, session_id: &str) {
    let mut watchers = state.feed.watchers.write().await;
    if let Some(ws) = watchers.get_mut(session_id) {
        ws.count = ws.count.saturating_sub(1);
        if ws.count == 0 {
            ws.feed_cancel.cancel();
            ws.poller_cancel.cancel();
            watchers.remove(session_id);
        }
    }
}

/// Proxy keyboard input to an upstream session.
async fn proxy_input(state: &MuxState, session_id: &str, text: &str, enter: bool) {
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(session_id) {
        Some(e) => Arc::clone(e),
        None => return,
    };
    drop(sessions);

    let client =
        crate::upstream::client::UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
    let body = serde_json::json!({ "text": text, "enter": enter });
    let _ = client.post_json("/api/v1/input", &body).await;
}

/// Proxy terminal resize to an upstream session.
async fn proxy_resize(state: &MuxState, session_id: &str, cols: u16, rows: u16) {
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(session_id) {
        Some(e) => Arc::clone(e),
        None => return,
    };
    drop(sessions);

    let client =
        crate::upstream::client::UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
    let body = serde_json::json!({ "cols": cols, "rows": rows });
    let _ = client.post_json("/api/v1/resize", &body).await;
}

/// Send a JSON message over the WebSocket.
async fn send_json<S>(tx: &mut S, msg: &MuxServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let text = match serde_json::to_string(msg) {
        Ok(t) => t,
        Err(_) => return Err(()),
    };
    tx.send(Message::Text(text.into())).await.map_err(|_| ())
}
