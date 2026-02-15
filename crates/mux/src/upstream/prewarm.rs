// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Slow-poll pre-warmer for recently registered sessions.
//!
//! Maintains an LRU of up to `capacity` session IDs. Every `interval`,
//! iterates the LRU and polls screen+status for sessions that don't
//! have active watchers (i.e., no dashboard subscriber running a fast poller).

use std::sync::Arc;

use indexmap::IndexMap;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::state::{epoch_ms, CachedScreen, CachedStatus, MuxState};
use crate::upstream::client::UpstreamClient;

/// Bounded LRU of session IDs for pre-warming.
///
/// Uses `IndexMap` for O(1) insert/remove with insertion-order iteration.
/// Most-recently-touched entries are at the back; eviction pops from the front.
pub struct PrewarmCache {
    map: IndexMap<String, ()>,
    capacity: usize,
}

impl PrewarmCache {
    pub fn new(capacity: usize) -> Self {
        Self { map: IndexMap::with_capacity(capacity), capacity }
    }

    /// Bump a session to the back (most-recently-used).
    pub fn touch(&mut self, session_id: &str) {
        // Remove + reinsert to move to back.
        self.map.shift_remove(session_id);
        self.map.insert(session_id.to_owned(), ());
        // Evict oldest if over capacity.
        while self.map.len() > self.capacity {
            self.map.shift_remove_index(0);
        }
    }

    /// Remove a session from the cache.
    pub fn remove(&mut self, session_id: &str) {
        self.map.shift_remove(session_id);
    }

    /// Return all session IDs in LRU order (oldest first).
    pub fn session_ids(&self) -> Vec<String> {
        self.map.keys().cloned().collect()
    }
}

/// Spawn the pre-warm polling task.
///
/// Every `interval`, iterates the LRU and polls screen+status for sessions
/// that don't have an active watcher (fast poller).
pub fn spawn_prewarm_task(
    state: Arc<MuxState>,
    prewarm: Arc<Mutex<PrewarmCache>>,
    interval: std::time::Duration,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tick.tick() => {}
            }

            let ids = prewarm.lock().await.session_ids();
            if ids.is_empty() {
                continue;
            }

            // Snapshot which sessions have active watchers so we can skip them.
            let watchers = state.feed.watchers.read().await;
            let active: std::collections::HashSet<&str> =
                watchers.keys().map(|s| s.as_str()).collect();

            // Snapshot session entries we need to poll.
            let sessions = state.sessions.read().await;
            let to_poll: Vec<Arc<crate::state::SessionEntry>> = ids
                .iter()
                .filter(|id| !active.contains(id.as_str()))
                .filter_map(|id| sessions.get(id).cloned())
                .collect();
            drop(sessions);
            drop(watchers);

            for entry in to_poll {
                let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());

                // Poll screen.
                if let Ok(value) = client.get_screen().await {
                    let lines: Vec<String> = value
                        .get("lines")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let ansi: Vec<String> = value
                        .get("ansi")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let screen = CachedScreen {
                        lines,
                        ansi,
                        cols: value.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16,
                        rows: value.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16,
                        alt_screen: value
                            .get("alt_screen")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        seq: value.get("seq").and_then(|v| v.as_u64()).unwrap_or(0),
                        fetched_at: epoch_ms(),
                    };
                    *entry.cached_screen.write().await = Some(screen);
                }

                // Poll status.
                if let Ok(value) = client.get_status().await {
                    let status = CachedStatus {
                        session_id: value
                            .get("session_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        state: value
                            .get("state")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_owned(),
                        pid: value.get("pid").and_then(|v| v.as_i64()).map(|v| v as i32),
                        uptime_secs: value
                            .get("uptime_secs")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0),
                        exit_code: value
                            .get("exit_code")
                            .and_then(|v| v.as_i64())
                            .map(|v| v as i32),
                        screen_seq: value
                            .get("screen_seq")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        bytes_read: value
                            .get("bytes_read")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        bytes_written: value
                            .get("bytes_written")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        ws_clients: value
                            .get("ws_clients")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0) as i32,
                        fetched_at: epoch_ms(),
                    };
                    *entry.cached_status.write().await = Some(status);
                }
            }
        }
    });
}
