// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket bridge: single upstream WS connection fanned out to multiple downstream clients.

use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::state::SessionEntry;

/// Upstream WS bridge for a single session.
///
/// Maintains one WS connection to the upstream coop and broadcasts all received
/// messages to downstream subscribers via a `broadcast::Sender`.
pub struct WsBridge {
    pub tx: broadcast::Sender<String>,
    cancel: CancellationToken,
}

impl WsBridge {
    /// Create and start a new WS bridge for the given session entry.
    pub fn connect(entry: &Arc<SessionEntry>, subscribe: &str) -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        let cancel = entry.cancel.child_token();

        let bridge = Arc::new(Self { tx: tx.clone(), cancel: cancel.clone() });

        let url = build_ws_url(&entry.url, entry.auth_token.as_deref(), subscribe);
        let entry_id = entry.id.clone();

        tokio::spawn(async move {
            let mut backoff_ms = 100u64;
            let max_backoff_ms = 5000u64;

            loop {
                if cancel.is_cancelled() {
                    break;
                }

                match tokio_tungstenite::connect_async(&url).await {
                    Ok((ws_stream, _)) => {
                        backoff_ms = 100; // reset on successful connect
                        tracing::debug!(session_id = %entry_id, "upstream WS connected");

                        let (_write, mut read) = ws_stream.split();

                        loop {
                            tokio::select! {
                                _ = cancel.cancelled() => break,
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(Message::Text(text))) => {
                                            // Broadcast to all downstream subscribers.
                                            // Ignore send errors (no subscribers).
                                            let _ = tx.send(text.to_string());
                                        }
                                        Some(Ok(Message::Close(_))) | None => {
                                            tracing::debug!(session_id = %entry_id, "upstream WS closed");
                                            break;
                                        }
                                        Some(Err(e)) => {
                                            tracing::debug!(session_id = %entry_id, err = %e, "upstream WS error");
                                            break;
                                        }
                                        _ => {} // ping/pong/binary ignored
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::debug!(
                            session_id = %entry_id,
                            err = %e,
                            backoff_ms,
                            "upstream WS connect failed, retrying"
                        );
                    }
                }

                // Exponential backoff before reconnect.
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
                }
                backoff_ms = (backoff_ms * 2).min(max_backoff_ms);
            }
        });

        bridge
    }
}

impl Drop for WsBridge {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Build the upstream WebSocket URL from an HTTP base URL.
fn build_ws_url(base_url: &str, auth_token: Option<&str>, subscribe: &str) -> String {
    // Convert http(s):// to ws(s)://
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
