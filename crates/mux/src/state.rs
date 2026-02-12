// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::upstream::ws_bridge::WsBridge;

// Re-export shared event types so existing `use crate::state::MuxEvent` still works.
pub use crate::events::{Aggregator, MuxEvent, SessionCache};

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
