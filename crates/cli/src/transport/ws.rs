// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket message types and handler for the coop real-time protocol.
//!
//! Messages use internally-tagged JSON enums (`{"event": "input", ...}`) as
//! specified in DESIGN.md. Two top-level enums cover server-to-client and
//! client-to-server directions.

#[path = "ws_msg.rs"]
mod msg;
pub use msg::*;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};

use crate::error::ErrorCode;
use crate::event::OutputEvent;
use crate::start::StartConfig;
use crate::stop::StopConfig;
use crate::transport::auth;
use crate::transport::handler::{
    compute_health, compute_status, error_message, handle_input, handle_input_raw, handle_keys,
    handle_nudge, handle_resize, handle_respond, handle_signal,
};
use crate::transport::read_ring_combined;
use crate::transport::state::Store;

/// Short-circuit: return an auth error if the client has not authenticated.
macro_rules! require_auth {
    ($authed:expr) => {
        if !*$authed {
            return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
        }
    };
}

/// WebSocket upgrade handler. Validates auth from query params if configured.
pub async fn ws_handler(
    State(state): State<Arc<Store>>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate auth token from query param if one is required.
    if state.config.auth_token.is_some() {
        if let Some(ref token) = query.token {
            if let Err(_code) = auth::validate_ws_auth(token, state.config.auth_token.as_deref()) {
                return axum::http::Response::builder()
                    .status(401)
                    .body(axum::body::Body::from("unauthorized"))
                    .unwrap_or_default()
                    .into_response();
            }
        }
        // If no token provided in query, the client can still auth via Auth message.
        // We'll track auth state per-connection.
    }

    let flags = query.flags();
    let needs_auth = state.config.auth_token.is_some() && query.token.is_none();

    ws.on_upgrade(move |socket| {
        let client_id = format!("ws-{}", next_client_id());
        handle_connection(state, flags, socket, client_id, needs_auth)
    })
    .into_response()
}

/// Per-connection event loop.
async fn handle_connection(
    state: Arc<Store>,
    flags: SubscriptionFlags,
    socket: WebSocket,
    client_id: String,
    needs_auth: bool,
) {
    state.lifecycle.ws_client_count.fetch_add(1, Ordering::Relaxed);

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut output_rx = state.channels.output_tx.subscribe();
    let mut state_rx = state.channels.state_tx.subscribe();
    let mut prompt_rx = state.channels.prompt_tx.subscribe();
    let mut stop_rx = state.stop.stop_tx.subscribe();
    let mut start_rx = state.start.start_tx.subscribe();
    let mut hook_rx = state.channels.hook_tx.subscribe();
    let mut message_rx = state.channels.message_tx.subscribe();
    let mut authed = !needs_auth;

    loop {
        tokio::select! {
            event = prompt_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.state {
                    let msg = ServerMessage::PromptOutcome {
                        source: event.source,
                        r#type: event.r#type,
                        subtype: event.subtype,
                        option: event.option,
                    };
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = stop_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.state {
                    let msg = stop_event_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = start_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.state {
                    let msg = start_event_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = output_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match &event {
                    OutputEvent::Raw(data) if flags.output => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                        let ring = state.terminal.ring.read().await;
                        let offset = ring.total_written().saturating_sub(data.len() as u64);
                        let msg = ServerMessage::Output { data: encoded, offset };
                        if send_json(&mut ws_tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    OutputEvent::ScreenUpdate { seq } if flags.screen => {
                        let snap = state.terminal.screen.read().await.snapshot();
                        if send_json(&mut ws_tx, &snapshot_to_msg(snap, *seq)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            event = state_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.state {
                    let msg = transition_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = hook_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.hooks {
                    let msg = ServerMessage::HookRaw { data: event.json };
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = message_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if flags.messages {
                    let msg = ServerMessage::MessageRaw { data: event.json, source: event.source };
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            msg = ws_rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break,
                };

                match msg {
                    Message::Text(text) => {
                        let client_msg: ClientMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(_) => {
                                let err = ServerMessage::Error {
                                    code: ErrorCode::BadRequest.as_str().to_owned(),
                                    message: "invalid message".to_owned(),
                                };
                                if send_json(&mut ws_tx, &err).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };

                        if let Some(reply) = handle_client_message(&state, client_msg, &client_id, &mut authed).await {
                            if send_json(&mut ws_tx, &reply).await.is_err() {
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

    // Cleanup
    state.lifecycle.ws_client_count.fetch_sub(1, Ordering::Relaxed);
}

/// Handle a single client message and optionally return a reply.
async fn handle_client_message(
    state: &Store,
    msg: ClientMessage,
    _client_id: &str,
    authed: &mut bool,
) -> Option<ServerMessage> {
    match msg {
        // Terminal
        ClientMessage::GetHealth {} => {
            let h = compute_health(state).await;
            Some(ServerMessage::Health {
                status: h.status,
                pid: h.pid,
                uptime_secs: h.uptime_secs,
                agent: h.agent,
                terminal_cols: h.terminal_cols,
                terminal_rows: h.terminal_rows,
                ws_clients: h.ws_clients,
                ready: h.ready,
            })
        }

        ClientMessage::GetReady {} => {
            let ready = state.ready.load(Ordering::Acquire);
            Some(ServerMessage::Ready { ready })
        }

        ClientMessage::GetScreen { cursor } => {
            require_auth!(authed);
            let snap = state.terminal.screen.read().await.snapshot();
            let seq = snap.sequence;
            Some(ServerMessage::Screen {
                lines: snap.lines,
                cols: snap.cols,
                rows: snap.rows,
                alt_screen: snap.alt_screen,
                cursor: if cursor { Some(snap.cursor) } else { None },
                seq,
            })
        }

        ClientMessage::GetStatus {} => {
            require_auth!(authed);
            Some(compute_status(state).await.into())
        }

        ClientMessage::GetReplay { offset, limit } => {
            require_auth!(authed);
            let ring = state.terminal.ring.read().await;
            let total_written = ring.total_written();
            let mut combined = read_ring_combined(&ring, offset);
            if let Some(limit) = limit {
                combined.truncate(limit);
            }
            let read_len = combined.len() as u64;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);
            Some(ServerMessage::Replay {
                data: encoded,
                offset,
                next_offset: offset + read_len,
                total_written,
            })
        }

        ClientMessage::SendInput { text, enter } => {
            require_auth!(authed);
            let bytes_written = handle_input(state, text, enter).await;
            Some(ServerMessage::InputSent { bytes_written })
        }

        ClientMessage::SendInputRaw { data } => {
            require_auth!(authed);
            let decoded = match base64::engine::general_purpose::STANDARD.decode(&data) {
                Ok(d) => d,
                Err(_) => return Some(ws_error(ErrorCode::BadRequest, "invalid base64 data")),
            };
            let bytes_written = handle_input_raw(state, decoded).await;
            Some(ServerMessage::InputSent { bytes_written })
        }

        ClientMessage::SendKeys { keys } => {
            require_auth!(authed);
            match handle_keys(state, &keys).await {
                Ok(bytes_written) => Some(ServerMessage::InputSent { bytes_written }),
                Err(bad_key) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("unknown key: {bad_key}")))
                }
            }
        }

        ClientMessage::SendSignal { signal } => {
            require_auth!(authed);
            match handle_signal(state, &signal).await {
                Ok(()) => Some(ServerMessage::SignalSent { delivered: true }),
                Err(bad_signal) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("unknown signal: {bad_signal}")))
                }
            }
        }

        ClientMessage::Resize { cols, rows } => {
            require_auth!(authed);
            match handle_resize(state, cols, rows).await {
                Ok(()) => Some(ServerMessage::Resized { cols, rows }),
                Err(_) => Some(ws_error(ErrorCode::BadRequest, "cols and rows must be positive")),
            }
        }

        // Agent
        ClientMessage::GetAgent {} => {
            require_auth!(authed);
            let agent = state.driver.agent_state.read().await;
            let screen = state.terminal.screen.read().await;
            let detection = state.driver.detection.read().await;
            let error_detail = state.driver.error.read().await.as_ref().map(|e| e.detail.clone());
            let error_category =
                state.driver.error.read().await.as_ref().map(|e| e.category.as_str().to_owned());
            let last_message = state.driver.last_message.read().await.clone();
            Some(ServerMessage::Agent {
                agent: state.config.agent.to_string(),
                state: agent.as_str().to_owned(),
                since_seq: state.driver.state_seq.load(std::sync::atomic::Ordering::Acquire),
                screen_seq: screen.seq(),
                detection_tier: detection.tier_str(),
                detection_cause: detection.cause.clone(),
                prompt: agent.prompt().cloned(),
                error_detail,
                error_category,
                last_message,
            })
        }

        ClientMessage::Nudge { message } => {
            require_auth!(authed);
            match handle_nudge(state, &message).await {
                Ok(outcome) => Some(outcome.into()),
                Err(code) => Some(ws_error(code, error_message(code))),
            }
        }

        ClientMessage::Respond { accept, text, answers, option } => {
            require_auth!(authed);
            match handle_respond(state, accept, option, text.as_deref(), &answers).await {
                Ok(outcome) => Some(outcome.into()),
                Err(code) => Some(ws_error(code, error_message(code))),
            }
        }

        // Stop hook
        ClientMessage::GetStopConfig {} => {
            require_auth!(authed);
            let config = state.stop.config.read().await;
            let json = serde_json::to_value(&*config).unwrap_or_default();
            Some(ServerMessage::StopConfig { config: json })
        }

        ClientMessage::PutStopConfig { config } => {
            require_auth!(authed);
            match serde_json::from_value::<StopConfig>(config) {
                Ok(new_config) => {
                    *state.stop.config.write().await = new_config;
                    Some(ServerMessage::StopConfigured { updated: true })
                }
                Err(e) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("invalid stop config: {e}")))
                }
            }
        }

        ClientMessage::ResolveStop { body } => {
            require_auth!(authed);
            let stop = &state.stop;
            *stop.signal_body.write().await = Some(body);
            stop.signaled.store(true, std::sync::atomic::Ordering::Release);
            Some(ServerMessage::StopResolved { accepted: true })
        }

        // Start hook
        ClientMessage::GetStartConfig {} => {
            require_auth!(authed);
            let config = state.start.config.read().await;
            let json = serde_json::to_value(&*config).unwrap_or_default();
            Some(ServerMessage::StartConfig { config: json })
        }

        ClientMessage::PutStartConfig { config } => {
            require_auth!(authed);
            match serde_json::from_value::<StartConfig>(config) {
                Ok(new_config) => {
                    *state.start.config.write().await = new_config;
                    Some(ServerMessage::StartConfigured { updated: true })
                }
                Err(e) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("invalid start config: {e}")))
                }
            }
        }

        // Lifecycle
        ClientMessage::Shutdown {} => {
            require_auth!(authed);
            state.lifecycle.shutdown.cancel();
            Some(ServerMessage::Shutdown { accepted: true })
        }

        // Connection
        ClientMessage::Ping {} => Some(ServerMessage::Pong {}),

        ClientMessage::Auth { token } => {
            match auth::validate_ws_auth(&token, state.config.auth_token.as_deref()) {
                Ok(()) => {
                    *authed = true;
                    None
                }
                Err(code) => Some(ServerMessage::Error {
                    code: code.as_str().to_owned(),
                    message: "authentication failed".to_owned(),
                }),
            }
        }
    }
}

/// Send a JSON-serialized message over the WebSocket.
async fn send_json<S>(tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let text = match serde_json::to_string(msg) {
        Ok(t) => t,
        Err(_) => return Err(()),
    };
    tx.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Generate a simple unique ID (not cryptographic, just for client tracking).
fn next_client_id() -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{ts:x}-{n}")
}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod tests;
