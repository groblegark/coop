// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::upstream::ws_bridge::WsBridge;

/// Shared mux state.
pub struct MuxState {
    pub sessions: RwLock<HashMap<String, Arc<SessionEntry>>>,
    pub config: MuxConfig,
    pub shutdown: CancellationToken,
    /// Aggregated event channel for `/ws/mux` clients.
    pub aggregator: Aggregator,
}

impl MuxState {
    pub fn new(config: MuxConfig, shutdown: CancellationToken) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
            shutdown,
            aggregator: Aggregator::new(),
        }
    }
}

// -- Aggregated mux event types -----------------------------------------------

/// Events emitted by the aggregator, tagged with the source session ID.
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

/// Cached state for a single session.
#[derive(Debug, Clone, Default)]
pub struct SessionCache {
    pub agent_state: Option<String>,
    pub screen_lines: Option<Vec<String>>,
    pub screen_cols: u16,
    pub screen_rows: u16,
    pub credential_status: Option<String>,
}

/// Aggregator hub — fans out session events to `/ws/mux` clients.
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

/// A registered upstream coop session.
pub struct SessionEntry {
    pub id: String,
    pub url: String,
    pub auth_token: Option<String>,
    pub metadata: serde_json::Value,
    pub registered_at: Instant,
    pub cached_screen: RwLock<Option<CachedScreen>>,
    pub cached_status: RwLock<Option<CachedStatus>>,
    pub health_failures: AtomicU32,
    pub cancel: CancellationToken,
    pub ws_bridge: RwLock<Option<Arc<WsBridge>>>,
}

/// Cached screen snapshot from upstream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedScreen {
    pub lines: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub alt_screen: bool,
    pub seq: u64,
    pub fetched_at: u64,
}

/// Cached status from upstream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedStatus {
    pub session_id: String,
    pub state: String,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub exit_code: Option<i32>,
    pub screen_seq: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub ws_clients: i32,
    pub fetched_at: u64,
}

/// Return current epoch millis.
pub fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
