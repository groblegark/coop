// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Background health checker for all registered sessions.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::state::MuxState;
use crate::upstream::client::UpstreamClient;

/// Timeout for individual upstream health check requests.
/// Kept short so dead pods don't block the checker and starve the mux's
/// own liveness probe.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// Spawn a single background task that periodically checks health of all sessions.
pub fn spawn_health_checker(state: Arc<MuxState>) {
    let interval = state.config.health_check_interval();
    let max_failures = state.config.max_health_failures;

    tokio::spawn(async move {
        let mut timer = tokio::time::interval(interval);
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = state.shutdown.cancelled() => break,
                _ = timer.tick() => {}
            }

            // Snapshot current sessions.
            let entries: Vec<_> = {
                let sessions = state.sessions.read().await;
                sessions.values().map(Arc::clone).collect()
            };

            // Run all health checks concurrently so a single dead pod
            // doesn't block the entire tick and starve the mux health endpoint.
            let state_ref = &state;
            let checks: Vec<_> = entries
                .iter()
                .filter(|entry| {
                    !entry.cancel.is_cancelled()
                        && !matches!(
                            entry.transport,
                            crate::state::SessionTransport::Nats { .. }
                        )
                })
                .map(|entry| {
                    let entry = Arc::clone(entry);
                    async move {
                        let client = UpstreamClient::with_timeout(
                            entry.url.clone(),
                            entry.auth_token.clone(),
                            HEALTH_CHECK_TIMEOUT,
                        );
                        (entry, client.health().await)
                    }
                })
                .collect();

            let results = futures_util::future::join_all(checks).await;

            for (entry, result) in results {
                if entry.cancel.is_cancelled() {
                    continue;
                }

                match result {
                    Ok(_) => {
                        entry.health_failures.store(0, Ordering::Relaxed);
                    }
                    Err(e) => {
                        let prev = entry.health_failures.fetch_add(1, Ordering::Relaxed);
                        let count = prev + 1;
                        tracing::warn!(
                            session_id = %entry.id,
                            failures = count,
                            err = %e,
                            "health check failed"
                        );

                        if count >= max_failures {
                            tracing::warn!(
                                session_id = %entry.id,
                                "evicting session after {count} consecutive health failures"
                            );
                            // Unassign from credential pool before removal.
                            if let Some(ref broker) = state_ref.credential_broker {
                                if let Some(account) =
                                    entry.assigned_account.read().await.as_ref()
                                {
                                    broker.session_unassigned(account).await;
                                }
                            }
                            state_ref.remove_session(&entry.id).await;
                        }
                    }
                }
            }
        }
    });
}
