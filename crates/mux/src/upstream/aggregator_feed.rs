// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Per-session aggregator feed — connects to upstream WS and emits
//! parsed `MuxEvent`s into the shared aggregator broadcast channel.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message;

use crate::state::{MuxEvent, SessionCache, SessionEntry};

/// Spawn a background task that connects to the session's upstream WS
/// and feeds parsed events into the aggregator.
pub fn spawn_aggregator_feed(
    entry: &Arc<SessionEntry>,
    event_tx: broadcast::Sender<MuxEvent>,
    cache: Arc<RwLock<HashMap<String, SessionCache>>>,
) {
    let entry = Arc::clone(entry);
    let cancel = entry.cancel.clone();

    tokio::spawn(async move {
        let session_id = entry.id.clone();
        let mut backoff_ms = 100u64;
        let max_backoff_ms = 5000u64;

        // Emit online event.
        let _ = event_tx.send(MuxEvent::SessionOnline {
            session: session_id.clone(),
            url: entry.url.clone(),
        });

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let url = build_ws_url(&entry.url, entry.auth_token.as_deref());

            match tokio_tungstenite::connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    backoff_ms = 100; // reset on successful connect
                    tracing::debug!(session_id = %session_id, "aggregator feed connected");

                    let (_write, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text.to_string()) {
                                            handle_message(&session_id, &json, &event_tx, &cache).await;
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        tracing::debug!(session_id = %session_id, "aggregator feed WS closed");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        tracing::debug!(session_id = %session_id, err = %e, "aggregator feed WS error");
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        session_id = %session_id,
                        err = %e,
                        backoff_ms,
                        "aggregator feed connect failed, retrying"
                    );
                }
            }

            // Emit offline on disconnect.
            let _ = event_tx.send(MuxEvent::SessionOffline { session: session_id.clone() });

            // Exponential backoff before reconnect.
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
            }
            backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
        }

        // Final offline on clean shutdown.
        let _ = event_tx.send(MuxEvent::SessionOffline { session: session_id });
    });
}

/// Parse an upstream WS message and emit the corresponding MuxEvent.
async fn handle_message(
    session_id: &str,
    msg: &serde_json::Value,
    event_tx: &broadcast::Sender<MuxEvent>,
    cache: &Arc<RwLock<HashMap<String, SessionCache>>>,
) {
    let event_type = msg.get("event").and_then(|e| e.as_str()).unwrap_or("");

    match event_type {
        "transition" => {
            let prev = msg.get("prev").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let next = msg.get("next").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let seq = msg.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);

            cache.write().await.entry(session_id.to_owned()).or_default().agent_state =
                Some(next.clone());

            let _ = event_tx.send(MuxEvent::State {
                session: session_id.to_owned(),
                prev,
                next,
                seq,
            });
        }
        "screen" => {
            let lines: Vec<String> = msg
                .get("lines")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let cols = msg.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
            let rows = msg.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;

            {
                let mut c = cache.write().await;
                let entry = c.entry(session_id.to_owned()).or_default();
                entry.screen_lines = Some(lines.clone());
                entry.screen_cols = cols;
                entry.screen_rows = rows;
            }

            let _ = event_tx.send(MuxEvent::Screen {
                session: session_id.to_owned(),
                lines,
                cols,
                rows,
            });
        }
        _ => {
            // Other event types are forwarded as-is for extensibility.
        }
    }
}

/// Build the upstream WebSocket URL for the aggregator feed.
fn build_ws_url(base_url: &str, auth_token: Option<&str>) -> String {
    let ws_base = if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    };

    let mut url = format!("{ws_base}/ws?subscribe=screen,state");
    if let Some(token) = auth_token {
        url.push_str(&format!("&token={token}"));
    }
    url
}
