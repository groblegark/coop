// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Background health checker for all registered sessions.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::state::MuxState;
use crate::upstream::client::UpstreamClient;

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

            for entry in &entries {
                // Skip sessions deregistered since the snapshot was taken.
                if entry.cancel.is_cancelled() {
                    continue;
                }

                let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
                match client.health().await {
                    Ok(_) => {
                        entry.health_failures.store(0, Ordering::Relaxed);
                    }
                    Err(e) => {
                        // Re-check: session may have been deregistered during the request.
                        if entry.cancel.is_cancelled() {
                            continue;
                        }

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
                            if let Some(ref broker) = state.credential_broker {
                                if let Some(account) = entry.assigned_account.read().await.as_ref()
                                {
                                    broker.session_unassigned(account).await;
                                }
                            }
                            state.remove_session(&entry.id).await;
                        }
                    }
                }
            }
        }
    });
}
