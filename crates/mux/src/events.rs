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

// -- Subscription filtering ---------------------------------------------------

/// Parsed subscription preferences for a mux WebSocket connection.
///
/// Shared by both the standalone `coop-mux` and the embedded broker multiplexer.
/// Callers construct a `MuxFilter` from their query parameters, then use
/// [`wants_session`] and [`wants_event`] to decide what to forward.
pub struct MuxFilter {
    all_sessions: bool,
    session_filter: Vec<String>,
    pub state: bool,
    pub screen: bool,
    pub credentials: bool,
}

impl MuxFilter {
    /// Build a filter from raw query-style parameters.
    ///
    /// `sessions_csv` — comma-separated session/pod identifiers, or `"all"`.
    /// `subscribe_csv` — comma-separated event types: `"state,screen,credentials"`.
    pub fn new(sessions_csv: &str, subscribe_csv: &str) -> Self {
        let all_sessions = sessions_csv == "all";
        let session_filter: Vec<String> = if all_sessions {
            vec![]
        } else {
            sessions_csv.split(',').map(|s| s.trim().to_owned()).collect()
        };
        let mut state = false;
        let mut screen = false;
        let mut credentials = false;
        for token in subscribe_csv.split(',') {
            match token.trim() {
                "state" => state = true,
                "screen" => screen = true,
                "credentials" => credentials = true,
                _ => {}
            }
        }
        Self { all_sessions, session_filter, state, screen, credentials }
    }

    /// Whether the filter accepts events for the given session identifier.
    pub fn wants_session(&self, session: &str) -> bool {
        self.all_sessions || self.session_filter.iter().any(|s| s == session)
    }

    /// Whether the filter accepts this specific event.
    pub fn wants_event(&self, event: &MuxEvent) -> bool {
        let session = event.session();
        match event {
            MuxEvent::State { .. } => self.state && self.wants_session(session),
            MuxEvent::Screen { .. } => self.screen && self.wants_session(session),
            MuxEvent::Credential { .. } => self.credentials && self.wants_session(session),
            MuxEvent::SessionOnline { .. } | MuxEvent::SessionOffline { .. } => {
                self.wants_session(session)
            }
        }
    }
}

/// Build a vec of cached `MuxEvent`s to send as initial backfill on WS connect.
///
/// Returns events matching the filter from the provided cache snapshot.
pub fn backfill_events(
    cache: &HashMap<String, SessionCache>,
    filter: &MuxFilter,
) -> Vec<MuxEvent> {
    let mut events = Vec::new();
    for (session_id, entry) in cache {
        if !filter.wants_session(session_id) {
            continue;
        }
        if filter.state {
            if let Some(ref agent_state) = entry.agent_state {
                events.push(MuxEvent::State {
                    session: session_id.clone(),
                    prev: String::new(),
                    next: agent_state.clone(),
                    seq: 0,
                });
            }
        }
        if filter.screen {
            if let Some(ref lines) = entry.screen_lines {
                events.push(MuxEvent::Screen {
                    session: session_id.clone(),
                    lines: lines.clone(),
                    cols: entry.screen_cols,
                    rows: entry.screen_rows,
                });
            }
        }
    }
    events
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
