// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential distributor: pushes refreshed credentials to sessions as profiles.
//!
//! Smart distribution features:
//! - **Per-pod filtering**: sessions can declare `profiles_needed` in metadata
//!   to receive only matching credentials (wildcard `*` or omission = all).
//! - **Idle-check**: queries upstream status before triggering a profile switch;
//!   busy sessions receive credentials but defer the switch.
//! - **Concurrency control**: limits concurrent per-session pushes via semaphore.
//! - **Retries**: per-session distribution retries with exponential backoff.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Semaphore};

use crate::credential::CredentialEvent;
use crate::state::MuxState;
use crate::upstream::client::UpstreamClient;

/// Maximum concurrent per-session credential pushes.
const MAX_CONCURRENT_PUSHES: usize = 8;

/// Maximum retries per session for a single distribution round.
const MAX_RETRIES: u32 = 3;

/// Initial backoff duration for retries.
const INITIAL_BACKOFF: Duration = Duration::from_millis(500);

/// Maximum backoff duration for retries.
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Agent states considered "busy" (switch is deferred, credentials still pushed).
const BUSY_STATES: &[&str] = &["running", "streaming", "tool_use"];

/// Spawn a distributor task that listens for credential refresh events
/// and pushes fresh credentials to registered sessions.
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
                    distribute_to_sessions(&state, &account, &credentials, true).await;
                }
                CredentialEvent::RefreshFailed { .. } | CredentialEvent::ReauthRequired { .. } => {
                    // No action needed for distribution.
                }
            }
        }
    });
}

/// Push credentials to matching registered sessions as a profile.
///
/// When `switch` is true, also triggers a profile switch on each session —
/// unless the session is currently busy, in which case credentials are
/// registered but the switch is deferred.
pub async fn distribute_to_sessions(
    state: &MuxState,
    account: &str,
    credentials: &std::collections::HashMap<String, String>,
    switch: bool,
) {
    let sessions = state.sessions.read().await;
    let count = sessions.len();
    if count == 0 {
        tracing::info!(account, "distributor: no sessions registered, nothing to distribute");
        return;
    }

    // Collect eligible sessions (filtering by profiles_needed metadata).
    let eligible: Vec<_> = sessions
        .values()
        .filter(|entry| session_needs_account(entry, account))
        .cloned()
        .collect();
    drop(sessions);

    let eligible_count = eligible.len();
    let skipped = count - eligible_count;
    if eligible_count == 0 {
        tracing::info!(account, count, skipped, "distributor: no eligible sessions for this account");
        return;
    }
    tracing::info!(
        account,
        eligible = eligible_count,
        skipped,
        "distributor: pushing credentials to eligible sessions"
    );

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_PUSHES));
    let ok = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let failed = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let deferred = Arc::new(std::sync::atomic::AtomicU32::new(0));

    let mut handles = Vec::with_capacity(eligible_count);

    for entry in eligible {
        let sem = Arc::clone(&semaphore);
        let ok = Arc::clone(&ok);
        let failed = Arc::clone(&failed);
        let deferred = Arc::clone(&deferred);
        let account = account.to_owned();
        let credentials = credentials.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;

            match push_to_session(&entry, &account, &credentials, switch).await {
                PushResult::Ok => {
                    ok.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                PushResult::Deferred => {
                    deferred.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                PushResult::Failed => {
                    failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }));
    }

    // Await all concurrent pushes.
    for h in handles {
        let _ = h.await;
    }

    let ok = ok.load(std::sync::atomic::Ordering::Relaxed);
    let failed = failed.load(std::sync::atomic::Ordering::Relaxed);
    let deferred = deferred.load(std::sync::atomic::Ordering::Relaxed);
    tracing::info!(account, ok, failed, deferred, "distributor: distribution complete");
}

/// Result of pushing credentials to a single session.
enum PushResult {
    /// Credentials pushed and switch triggered.
    Ok,
    /// Credentials pushed but switch deferred (session busy).
    Deferred,
    /// Failed after retries.
    Failed,
}

/// Push credentials to a single session with retries and idle checking.
async fn push_to_session(
    entry: &crate::state::SessionEntry,
    account: &str,
    credentials: &std::collections::HashMap<String, String>,
    switch: bool,
) -> PushResult {
    let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
    let profile_body = serde_json::json!({
        "profiles": [{
            "name": account,
            "credentials": credentials,
        }]
    });

    // Retry loop for profile registration.
    let mut backoff = INITIAL_BACKOFF;
    let mut registered = false;
    for attempt in 0..=MAX_RETRIES {
        match client.post_json("/api/v1/session/profiles", &profile_body).await {
            Ok(_) => {
                registered = true;
                break;
            }
            Err(e) => {
                if attempt == MAX_RETRIES {
                    tracing::warn!(
                        session = %entry.id, account, attempt, err = %e,
                        "distributor: failed to push profile after retries"
                    );
                    return PushResult::Failed;
                }
                tracing::debug!(
                    session = %entry.id, account, attempt, err = %e,
                    "distributor: profile push failed, retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    if !registered {
        return PushResult::Failed;
    }

    if !switch {
        tracing::info!(session = %entry.id, account, "distributor: credentials pushed (no switch requested)");
        return PushResult::Ok;
    }

    // Idle check: query upstream status to decide whether to switch now.
    let is_busy = match client.get_status().await {
        Ok(status) => {
            status.get("state")
                .and_then(|s| s.as_str())
                .is_some_and(|s| BUSY_STATES.contains(&s))
        }
        Err(e) => {
            tracing::debug!(
                session = %entry.id, account, err = %e,
                "distributor: could not check session status, assuming idle"
            );
            false
        }
    };

    if is_busy {
        tracing::info!(
            session = %entry.id, account,
            "distributor: credentials pushed, switch deferred (session busy)"
        );
        return PushResult::Deferred;
    }

    // Trigger profile switch with retry.
    let switch_body = serde_json::json!({
        "profile": account,
        "force": false,
    });

    backoff = INITIAL_BACKOFF;
    for attempt in 0..=MAX_RETRIES {
        match client.post_json("/api/v1/session/switch", &switch_body).await {
            Ok(_) => {
                tracing::info!(
                    session = %entry.id, account,
                    "distributor: credentials pushed and switch triggered"
                );
                return PushResult::Ok;
            }
            Err(e) => {
                if attempt == MAX_RETRIES {
                    tracing::warn!(
                        session = %entry.id, account, attempt, err = %e,
                        "distributor: switch failed after retries (credentials still registered)"
                    );
                    // Credentials were registered, switch just failed.
                    return PushResult::Deferred;
                }
                tracing::debug!(
                    session = %entry.id, account, attempt, err = %e,
                    "distributor: switch failed, retrying"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }

    PushResult::Deferred
}

/// Check whether a session needs credentials for the given account.
///
/// Sessions can declare which accounts they need via the `profiles_needed`
/// metadata key:
/// - Absent / null → receives all accounts (backwards-compatible default).
/// - `["*"]` → receives all accounts.
/// - `["claude-max", "openai-api"]` → receives only those named accounts.
///
/// The account name is matched case-insensitively.
fn session_needs_account(entry: &crate::state::SessionEntry, account: &str) -> bool {
    session_needs_account_metadata(&entry.metadata, account)
}

/// Check whether metadata indicates a session needs a given account.
///
/// Public variant used by session registration to filter initial profile pushes.
pub fn session_needs_account_metadata(metadata: &serde_json::Value, account: &str) -> bool {
    let Some(needed) = metadata.get("profiles_needed") else {
        return true; // No filter → receives everything.
    };
    let Some(arr) = needed.as_array() else {
        return true; // Malformed → treat as no filter.
    };
    if arr.is_empty() {
        return true; // Empty array → receives everything.
    }
    let account_lower = account.to_lowercase();
    arr.iter().any(|v| {
        v.as_str().is_some_and(|s| s == "*" || s.to_lowercase() == account_lower)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::SessionEntry;
    use std::sync::atomic::AtomicU32;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

    fn make_entry(metadata: serde_json::Value) -> SessionEntry {
        SessionEntry {
            id: "test".into(),
            url: "http://localhost:8080".into(),
            auth_token: None,
            metadata,
            registered_at: std::time::Instant::now(),
            cached_screen: RwLock::new(None),
            cached_status: RwLock::new(None),
            health_failures: AtomicU32::new(0),
            cancel: CancellationToken::new(),
            ws_bridge: RwLock::new(None),
        }
    }

    #[test]
    fn test_no_metadata_receives_all() {
        let entry = make_entry(serde_json::Value::Null);
        assert!(session_needs_account(&entry, "claude-max"));
        assert!(session_needs_account(&entry, "openai-api"));
    }

    #[test]
    fn test_empty_profiles_needed_receives_all() {
        let entry = make_entry(serde_json::json!({ "profiles_needed": [] }));
        assert!(session_needs_account(&entry, "claude-max"));
    }

    #[test]
    fn test_wildcard_receives_all() {
        let entry = make_entry(serde_json::json!({ "profiles_needed": ["*"] }));
        assert!(session_needs_account(&entry, "claude-max"));
        assert!(session_needs_account(&entry, "openai-api"));
    }

    #[test]
    fn test_specific_filter() {
        let entry = make_entry(serde_json::json!({ "profiles_needed": ["claude-max"] }));
        assert!(session_needs_account(&entry, "claude-max"));
        assert!(!session_needs_account(&entry, "openai-api"));
    }

    #[test]
    fn test_case_insensitive() {
        let entry = make_entry(serde_json::json!({ "profiles_needed": ["Claude-Max"] }));
        assert!(session_needs_account(&entry, "claude-max"));
        assert!(session_needs_account(&entry, "CLAUDE-MAX"));
    }

    #[test]
    fn test_multiple_profiles() {
        let entry = make_entry(serde_json::json!({ "profiles_needed": ["claude-max", "gemini"] }));
        assert!(session_needs_account(&entry, "claude-max"));
        assert!(session_needs_account(&entry, "gemini"));
        assert!(!session_needs_account(&entry, "openai-api"));
    }

    #[test]
    fn test_malformed_profiles_needed() {
        // Not an array → treat as no filter
        let entry = make_entry(serde_json::json!({ "profiles_needed": "claude-max" }));
        assert!(session_needs_account(&entry, "anything"));
    }
}
