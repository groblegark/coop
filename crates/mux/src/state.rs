// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::credential::broker::CredentialBroker;
use crate::credential::CredentialEvent;
use crate::upstream::bridge::WsBridge;

/// Events emitted by the mux for aggregation consumers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum MuxEvent {
    /// An agent state transition from an upstream session.
    ///
    /// Fields mirror the upstream single-session `Transition` WebSocket message,
    /// plus `session` to identify which upstream session it came from.
    Transition {
        session: String,
        prev: String,
        next: String,
        seq: u64,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        cause: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_category: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parked_reason: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resume_at_epoch_ms: Option<u64>,
    },
    /// An upstream session came online (feed connected).
    #[serde(rename = "session:online")]
    SessionOnline { session: String, url: String, metadata: serde_json::Value },
    /// An upstream session went offline (deregistered or feed disconnected).
    #[serde(rename = "session:offline")]
    SessionOffline { session: String },
    /// Credentials refreshed successfully for an account.
    #[serde(rename = "credential:refreshed")]
    CredentialRefreshed { account: String },
    /// A credential refresh attempt failed.
    #[serde(rename = "credential:refresh:failed")]
    CredentialRefreshFailed { account: String, error: String },
    /// User interaction required for credential reauthorization.
    #[serde(rename = "credential:reauth:required")]
    CredentialReauthRequired { account: String, auth_url: String },
}

impl MuxEvent {
    /// Convert a [`CredentialEvent`] into a `MuxEvent`, stripping secrets
    /// (access tokens from `Refreshed` are not included).
    pub fn from_credential(event: &CredentialEvent) -> Self {
        match event {
            CredentialEvent::Refreshed { account, .. } => {
                Self::CredentialRefreshed { account: account.clone() }
            }
            CredentialEvent::RefreshFailed { account, error } => {
                Self::CredentialRefreshFailed { account: account.clone(), error: error.clone() }
            }
            CredentialEvent::ReauthRequired { account, auth_url } => {
                Self::CredentialReauthRequired {
                    account: account.clone(),
                    auth_url: auth_url.clone(),
                }
            }
        }
    }
}

/// Per-session event feed and watcher tracking.
pub struct SessionFeed {
    /// Broadcast channel for mux events (state transitions, online/offline).
    pub event_tx: broadcast::Sender<MuxEvent>,
    /// Per-session watcher count. Feed + poller start when >0, stop when 0.
    pub watchers: RwLock<HashMap<String, WatcherState>>,
}

/// Tracks per-session watcher count and feed/poller cancellation.
pub struct WatcherState {
    pub count: usize,
    /// Cancel token for the event feed task.
    pub feed_cancel: CancellationToken,
    /// Cancel token for screen + status pollers.
    pub poller_cancel: CancellationToken,
}

impl Default for SessionFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionFeed {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(512);
        Self { event_tx, watchers: RwLock::new(HashMap::new()) }
    }
}

/// Shared mux state.
pub struct MuxState {
    pub sessions: RwLock<HashMap<String, Arc<SessionEntry>>>,
    pub config: MuxConfig,
    pub shutdown: CancellationToken,
    pub feed: SessionFeed,
    pub credential_broker: Option<Arc<CredentialBroker>>,
}

impl MuxState {
    pub fn new(config: MuxConfig, shutdown: CancellationToken) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
            shutdown,
            feed: SessionFeed::new(),
            credential_broker: None,
        }
    }

    /// Remove a session and clean up all associated resources.
    ///
    /// Cancels the session entry, emits `SessionOffline`, and tears down
    /// any active feed/poller watcher tasks.
    pub async fn remove_session(&self, id: &str) -> Option<Arc<SessionEntry>> {
        let entry = self.sessions.write().await.remove(id)?;
        entry.cancel.cancel();
        let _ = self.feed.event_tx.send(MuxEvent::SessionOffline { session: id.to_owned() });
        let mut watchers = self.feed.watchers.write().await;
        if let Some(ws) = watchers.remove(id) {
            ws.feed_cancel.cancel();
            ws.poller_cancel.cancel();
        }
        Some(entry)
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
    pub ansi: Vec<String>,
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
