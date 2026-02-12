// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared mux event types, cache, aggregator, and upstream WS message parsing.
//!
//! This module is the canonical source for mux event types, used by both the
//! standalone `coop-mux` binary and the embedded broker multiplexer in the
//! `coop` CLI.  All wire-format events use `session` as the identity key;
//! callers map their own identifiers (pod name, bead ID, etc.) into this field.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

// -- Wire-format event types -------------------------------------------------

/// Events emitted by the mux aggregator, tagged with a source session ID.
///
/// The `session` field is a generic identifier — it may represent a registered
/// session ID (standalone mux) or a pod name (broker mode).  Both sides agree
/// on the wire format so dashboard clients work unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MuxEvent {
    /// A session's agent state changed.
    State { session: String, prev: String, next: String, seq: u64 },
    /// A session's screen was updated.
    Screen { session: String, lines: Vec<String>, cols: u16, rows: u16 },
    /// A session's credential status changed.
    Credential {
        session: String,
        account: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// A session came online.
    SessionOnline { session: String, url: String },
    /// A session went offline.
    SessionOffline { session: String },
}

impl MuxEvent {
    /// Return the session identifier for this event.
    pub fn session(&self) -> &str {
        match self {
            Self::State { session, .. }
            | Self::Screen { session, .. }
            | Self::Credential { session, .. }
            | Self::SessionOnline { session, .. }
            | Self::SessionOffline { session } => session,
        }
    }
}

// -- Cached state ------------------------------------------------------------

/// Cached state for a single session/pod.
#[derive(Debug, Clone, Default)]
pub struct SessionCache {
    pub agent_state: Option<String>,
    pub screen_lines: Option<Vec<String>>,
    pub screen_cols: u16,
    pub screen_rows: u16,
    pub credential_status: Option<String>,
}

// -- Aggregator hub ----------------------------------------------------------

/// Aggregator hub — fans out mux events to downstream clients via broadcast.
pub struct Aggregator {
    pub event_tx: broadcast::Sender<MuxEvent>,
    pub cache: Arc<RwLock<HashMap<String, SessionCache>>>,
}

impl Aggregator {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self { event_tx, cache: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Subscribe to aggregated events.
    pub fn subscribe(&self) -> broadcast::Receiver<MuxEvent> {
        self.event_tx.subscribe()
    }

    /// Return cached state for all sessions.
    pub async fn cached_state(&self) -> HashMap<String, SessionCache> {
        self.cache.read().await.clone()
    }
}

impl Default for Aggregator {
    fn default() -> Self {
        Self::new()
    }
}

// -- Upstream WS message parsing ---------------------------------------------

/// Parse an upstream coop WebSocket JSON message and emit the corresponding
/// `MuxEvent`.  Updates the session cache as a side effect.
///
/// `source_id` is the identifier to use in the emitted event's `session` field
/// (e.g. a registered session ID or a pod name).
pub async fn parse_upstream_message(
    source_id: &str,
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

            cache.write().await.entry(source_id.to_owned()).or_default().agent_state =
                Some(next.clone());

            let _ = event_tx.send(MuxEvent::State {
                session: source_id.to_owned(),
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
                let entry = c.entry(source_id.to_owned()).or_default();
                entry.screen_lines = Some(lines.clone());
                entry.screen_cols = cols;
                entry.screen_rows = rows;
            }

            let _ = event_tx.send(MuxEvent::Screen {
                session: source_id.to_owned(),
                lines,
                cols,
                rows,
            });
        }
        "credential:status" => {
            let account = msg.get("account").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let status = msg.get("status").and_then(|v| v.as_str()).unwrap_or("").to_owned();
            let error = msg.get("error").and_then(|v| v.as_str()).map(String::from);

            cache.write().await.entry(source_id.to_owned()).or_default().credential_status =
                Some(status.clone());

            let _ = event_tx.send(MuxEvent::Credential {
                session: source_id.to_owned(),
                account,
                status,
                error,
            });
        }
        _ => {
            // Other event types ignored for now.
        }
    }
}

// -- Utility -----------------------------------------------------------------

/// Build an upstream WebSocket URL from a coop HTTP base URL.
///
/// Subscribes to screen, state, and credential events.  Appends an auth token
/// query parameter if provided.
pub fn build_upstream_ws_url(base_url: &str, auth_token: Option<&str>) -> String {
    let ws_base = if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    };

    let mut url = format!("{ws_base}/ws?subscribe=screen,state,credentials");
    if let Some(token) = auth_token {
        url.push_str(&format!("&token={token}"));
    }
    url
}
