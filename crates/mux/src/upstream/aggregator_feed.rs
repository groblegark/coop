// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Per-session aggregator feed — connects to upstream WS and emits
//! parsed `MuxEvent`s into the shared aggregator broadcast channel.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{broadcast, RwLock};
use tokio_tungstenite::tungstenite::Message;

use crate::events::{build_upstream_ws_url, parse_upstream_message, MuxEvent, SessionCache};
use crate::state::SessionEntry;

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

            let url = build_upstream_ws_url(&entry.url, entry.auth_token.as_deref());

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
                                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(text.as_ref()) {
                                            parse_upstream_message(&session_id, &json, &event_tx, &cache).await;
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
