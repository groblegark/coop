// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS subscriber for auto-discovering coop sessions via relay.
//!
//! Subscribes to `{prefix}.session.>` and processes:
//! - `announce` → register/remove `SessionEntry` in `MuxState`
//! - `status` → write to `entry.cached_status`
//! - `state` → emit `MuxEvent::Transition` via `state.feed.event_tx`
//!
//! Sessions discovered via NATS have `SessionTransport::Nats` and their
//! liveness is tracked by announce heartbeats (90s timeout) instead of HTTP health.

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::state::{
    CachedStatus, MuxEvent, MuxState, SessionEntry, SessionTransport,
};

/// Configuration for the NATS relay subscriber.
pub struct NatsRelayConfig {
    pub url: String,
    pub token: Option<String>,
    pub prefix: String,
}

/// Spawn the NATS relay subscriber as a background task.
pub fn spawn_nats_subscriber(state: Arc<MuxState>, config: NatsRelayConfig) {
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        if let Err(e) = run_subscriber(state, config, shutdown).await {
            tracing::error!(err = %e, "nats-relay subscriber failed");
        }
    });
}

async fn run_subscriber(
    state: Arc<MuxState>,
    config: NatsRelayConfig,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let opts = if let Some(ref token) = config.token {
        async_nats::ConnectOptions::with_token(token.clone())
    } else {
        async_nats::ConnectOptions::new()
    };

    let client = opts.connect(&config.url).await?;
    tracing::info!(url = %config.url, prefix = %config.prefix, "nats-relay subscriber connected");

    // Store the client on MuxState so proxy handlers can publish input commands.
    *state.nats_client.write().await = Some(client.clone());

    // Subscribe to all session-scoped subjects.
    let subject = format!("{}.session.>", config.prefix);
    let mut sub = client.subscribe(subject).await?;

    // Track last announce time per session for heartbeat timeout.
    let mut last_announce: HashMap<String, Instant> = HashMap::new();
    let mut eviction_timer = tokio::time::interval(std::time::Duration::from_secs(15));
    eviction_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            msg = sub.next() => {
                let Some(msg) = msg else { break };
                let subject_str = msg.subject.as_str();
                // Parse subject: {prefix}.session.{session_id}.{event_type}
                let suffix = match subject_str.strip_prefix(&config.prefix) {
                    Some(s) => s.strip_prefix('.').unwrap_or(s),
                    None => continue,
                };
                let parts: Vec<&str> = suffix.splitn(3, '.').collect();
                if parts.len() < 3 || parts[0] != "session" {
                    continue;
                }
                let session_id = parts[1];
                let event_type = parts[2];

                match event_type {
                    "announce" => {
                        handle_announce(
                            &state,
                            &config.prefix,
                            session_id,
                            &msg.payload,
                            &mut last_announce,
                        ).await;
                    }
                    "status" => {
                        handle_status(&state, session_id, &msg.payload).await;
                    }
                    "state" => {
                        handle_state(&state, session_id, &msg.payload).await;
                    }
                    _ => {
                        tracing::trace!(event_type, session_id, "nats-relay: unknown event type");
                    }
                }
            }
            _ = eviction_timer.tick() => {
                // Evict sessions that haven't announced in 90s.
                let threshold = std::time::Duration::from_secs(90);
                let now = Instant::now();
                let stale: Vec<String> = last_announce
                    .iter()
                    .filter(|(_, ts)| now.duration_since(**ts) > threshold)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in stale {
                    last_announce.remove(&id);
                    // Only evict if it's a NATS-transport session.
                    let is_nats = {
                        let sessions = state.sessions.read().await;
                        sessions.get(&id).is_some_and(|e| matches!(e.transport, SessionTransport::Nats { .. }))
                    };
                    if is_nats {
                        tracing::info!(session_id = %id, "nats-relay: evicting session (announce timeout)");
                        state.remove_session(&id).await;
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle an announce event (online, heartbeat, offline).
async fn handle_announce(
    state: &MuxState,
    prefix: &str,
    session_id: &str,
    payload: &[u8],
    last_announce: &mut HashMap<String, Instant>,
) {
    #[derive(serde::Deserialize)]
    struct AnnounceMsg {
        event: String,
        #[serde(default)]
        url: Option<String>,
        #[serde(flatten)]
        extra: serde_json::Value,
    }

    let msg: AnnounceMsg = match serde_json::from_slice(payload) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("nats-relay: invalid announce message: {e}");
            return;
        }
    };

    match msg.event.as_str() {
        "online" | "heartbeat" => {
            last_announce.insert(session_id.to_owned(), Instant::now());

            // Check if session already exists — heartbeat re-registration.
            let exists = state.sessions.read().await.contains_key(session_id);
            if exists {
                return;
            }

            // Register new session.
            let url = msg.url.unwrap_or_default();
            let metadata = msg.extra;
            let cancel = CancellationToken::new();

            let entry = Arc::new(SessionEntry {
                id: session_id.to_owned(),
                url: url.clone(),
                auth_token: None,
                metadata: metadata.clone(),
                registered_at: Instant::now(),
                cached_screen: tokio::sync::RwLock::new(None),
                cached_status: tokio::sync::RwLock::new(None),
                health_failures: AtomicU32::new(0),
                cancel,
                ws_bridge: tokio::sync::RwLock::new(None),
                assigned_account: tokio::sync::RwLock::new(None),
                transport: SessionTransport::Nats { prefix: prefix.to_owned() },
            });

            state.sessions.write().await.insert(session_id.to_owned(), Arc::clone(&entry));
            let _ = state.feed.event_tx.send(MuxEvent::SessionOnline {
                session: session_id.to_owned(),
                url,
                metadata,
            });
            tracing::info!(session_id, "nats-relay: session registered");
        }
        "offline" => {
            last_announce.remove(session_id);
            // Only remove if it's a NATS-transport session (don't evict HTTP sessions).
            let is_nats = {
                let sessions = state.sessions.read().await;
                sessions.get(session_id).is_some_and(|e| matches!(e.transport, SessionTransport::Nats { .. }))
            };
            if is_nats {
                state.remove_session(session_id).await;
                tracing::info!(session_id, "nats-relay: session deregistered (offline)");
            }
        }
        other => {
            tracing::debug!(event = other, "nats-relay: unknown announce event");
        }
    }
}

/// Handle a status update from a NATS-relayed session.
async fn handle_status(state: &MuxState, session_id: &str, payload: &[u8]) {
    let status: CachedStatus = match serde_json::from_slice(payload) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("nats-relay: invalid status message: {e}");
            return;
        }
    };

    let sessions = state.sessions.read().await;
    if let Some(entry) = sessions.get(session_id) {
        *entry.cached_status.write().await = Some(status);
    }
}

/// Handle a state transition from a NATS-relayed session.
async fn handle_state(state: &MuxState, session_id: &str, payload: &[u8]) {
    #[derive(serde::Deserialize)]
    struct StateMsg {
        #[serde(default)]
        prev: String,
        #[serde(default)]
        next: String,
        #[serde(default)]
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

    let msg: StateMsg = match serde_json::from_slice(payload) {
        Ok(m) => m,
        Err(e) => {
            tracing::debug!("nats-relay: invalid state message: {e}");
            return;
        }
    };

    let _ = state.feed.event_tx.send(MuxEvent::Transition {
        session: session_id.to_owned(),
        prev: msg.prev,
        next: msg.next,
        seq: msg.seq,
        cause: msg.cause,
        last_message: msg.last_message,
        prompt: msg.prompt,
        error_detail: msg.error_detail,
        error_category: msg.error_category,
        parked_reason: msg.parked_reason,
        resume_at_epoch_ms: msg.resume_at_epoch_ms,
    });
}

#[cfg(test)]
#[path = "nats_sub_tests.rs"]
mod tests;
