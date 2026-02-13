// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Per-session event feed: connects to upstream `/ws?subscribe=state`,
//! parses transitions → `MuxEvent::Transition`. Emits SessionOnline/SessionOffline.
//! Reconnects with exponential backoff. Started/stopped on demand.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::state::{MuxEvent, SessionEntry};

/// Spawn a per-session event feed that subscribes to upstream state transitions.
///
/// Emits `SessionOnline` when first connected, `SessionOffline` on cancel.
/// Reconnects with exponential backoff on disconnection.
pub fn spawn_event_feed(
    event_tx: broadcast::Sender<MuxEvent>,
    entry: Arc<SessionEntry>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let session_id = entry.id.clone();
        let mut backoff = Duration::from_millis(100);
        let max_backoff = Duration::from_secs(5);

        loop {
            if cancel.is_cancelled() {
                break;
            }

            let ws_url = build_ws_url(&entry.url, "state", entry.auth_token.as_deref());

            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    backoff = Duration::from_millis(100); // Reset on success.

                    // Emit online.
                    let _ = event_tx.send(MuxEvent::SessionOnline {
                        session: session_id.clone(),
                        url: entry.url.clone(),
                        metadata: entry.metadata.clone(),
                    });

                    let (_, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => break,
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                                        if let Some(event) = parse_state_transition(&session_id, &text) {
                                            let _ = event_tx.send(event);
                                        }
                                    }
                                    Some(Ok(_)) => {} // Ignore binary, ping, pong.
                                    Some(Err(e)) => {
                                        tracing::debug!(session = %session_id, err = %e, "feed ws error");
                                        break;
                                    }
                                    None => break, // Stream ended.
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(session = %session_id, err = %e, "feed ws connect failed");
                }
            }

            if cancel.is_cancelled() {
                break;
            }

            // Exponential backoff before reconnect.
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(max_backoff);
        }

        // Emit offline.
        let _ = event_tx.send(MuxEvent::SessionOffline { session: session_id });
    });
}

/// Upstream transition JSON shape (subset we care about).
#[derive(serde::Deserialize)]
struct UpstreamTransition {
    event: String,
    prev: String,
    next: String,
    seq: u64,
    #[serde(default)]
    cause: String,
    #[serde(default)]
    last_message: Option<String>,
    #[serde(default)]
    prompt: Option<serde_json::Value>,
    #[serde(default)]
    error_detail: Option<String>,
    #[serde(default)]
    error_category: Option<String>,
    #[serde(default)]
    parked_reason: Option<String>,
    #[serde(default)]
    resume_at_epoch_ms: Option<u64>,
}

/// Parse an upstream state transition message into a `MuxEvent::Transition`.
fn parse_state_transition(session_id: &str, text: &str) -> Option<MuxEvent> {
    let t: UpstreamTransition = serde_json::from_str(text).ok()?;
    if t.event != "transition" {
        return None;
    }

    Some(MuxEvent::Transition {
        session: session_id.to_owned(),
        prev: t.prev,
        next: t.next,
        seq: t.seq,
        cause: t.cause,
        last_message: t.last_message,
        prompt: t.prompt,
        error_detail: t.error_detail,
        error_category: t.error_category,
        parked_reason: t.parked_reason,
        resume_at_epoch_ms: t.resume_at_epoch_ms,
    })
}

/// Build a WebSocket URL from an HTTP base URL.
fn build_ws_url(base_url: &str, subscribe: &str, auth_token: Option<&str>) -> String {
    let ws_base = if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    };
    let mut url = format!("{ws_base}/ws?subscribe={subscribe}");
    if let Some(token) = auth_token {
        url.push_str(&format!("&token={token}"));
    }
    url
}
