// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential distributor: pushes refreshed credentials to sessions as profiles.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::credential::CredentialEvent;
use crate::state::MuxState;
use crate::upstream::client::UpstreamClient;

/// Spawn a distributor task that listens for credential refresh events
/// and pushes fresh credentials to all registered sessions.
pub fn spawn_distributor(state: Arc<MuxState>, mut event_rx: broadcast::Receiver<CredentialEvent>) {
    tokio::spawn(async move {
        loop {
            let event = match event_rx.recv().await {
                Ok(e) => e,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "distributor lagged");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            match event {
                CredentialEvent::Refreshed { account, credentials } => {
                    distribute_to_sessions(&state, &account, &credentials).await;
                }
                CredentialEvent::RefreshFailed { .. } | CredentialEvent::ReauthRequired { .. } => {
                    // No action needed for distribution.
                }
            }
        }
    });
}

/// Push fresh credentials to all sessions that need this account's profile.
async fn distribute_to_sessions(
    state: &MuxState,
    account: &str,
    credentials: &std::collections::HashMap<String, String>,
) {
    let sessions = state.sessions.read().await;
    for entry in sessions.values() {
        // Check if this session wants this account's profile.
        if !entry.profiles_needed.iter().any(|p| p == account) {
            continue;
        }

        let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());

        // Register as a profile.
        let profile_body = serde_json::json!({
            "profiles": [{
                "name": account,
                "credentials": credentials,
            }]
        });
        if let Err(e) = client.post_json("/api/v1/session/profiles", &profile_body).await {
            tracing::debug!(session = %entry.id, account, err = %e, "failed to push profile");
            continue;
        }

        // Trigger switch to the fresh profile.
        let switch_body = serde_json::json!({
            "profile": account,
            "force": false,
        });
        if let Err(e) = client.post_json("/api/v1/session/switch", &switch_body).await {
            tracing::debug!(session = %entry.id, account, err = %e, "failed to trigger switch");
        }
    }
}
