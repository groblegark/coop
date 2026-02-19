// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker: static API key store with pool load balancing.
//!
//! Accounts are loaded from `--credential-config` at startup. Each account
//! holds a long-lived API key (no refresh loops, no OAuth flows). Keys can
//! be set at runtime via `set_token()` or `add_account()`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use crate::credential::{
    provider_default_env_key, AccountConfig, AccountStatus, CredentialConfig, CredentialEvent,
};

/// Runtime state for a single account.
struct AccountState {
    config: AccountConfig,
    status: AccountStatus,
    /// The API key or token for this account.
    api_key: Option<String>,
}

/// The credential broker manages static API keys for configured accounts.
///
/// No refresh loops, no OAuth flows. Keys are loaded at startup or set
/// dynamically via `set_token()` / `add_account()`.
pub struct CredentialBroker {
    accounts: RwLock<HashMap<String, AccountState>>,
    event_tx: broadcast::Sender<CredentialEvent>,
    /// Per-account session counts for pool load balancing.
    session_counts: RwLock<HashMap<String, AtomicU32>>,
}

impl CredentialBroker {
    /// Create a new broker from config.
    pub fn new(
        config: CredentialConfig,
        event_tx: broadcast::Sender<CredentialEvent>,
    ) -> Arc<Self> {
        let mut accounts = HashMap::new();
        for acct in &config.accounts {
            accounts.insert(
                acct.name.clone(),
                AccountState {
                    config: acct.clone(),
                    status: AccountStatus::Missing,
                    api_key: None,
                },
            );
        }
        let session_counts: HashMap<String, AtomicU32> =
            accounts.keys().map(|name| (name.clone(), AtomicU32::new(0))).collect();

        Arc::new(Self {
            accounts: RwLock::new(accounts),
            event_tx,
            session_counts: RwLock::new(session_counts),
        })
    }

    /// Set the API key for an account. Emits a `Refreshed` event so the
    /// distributor pushes the credential to sessions.
    ///
    /// `refresh_token` and `expires_in` are accepted for API compatibility
    /// but ignored (static keys don't expire).
    pub async fn set_token(
        &self,
        account: &str,
        access_token: String,
        _refresh_token: Option<String>,
        _expires_in: Option<u64>,
    ) -> anyhow::Result<()> {
        let mut accounts = self.accounts.write().await;
        let state = accounts
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account}"))?;

        state.api_key = Some(access_token.clone());
        state.status = AccountStatus::Healthy;

        let env_key = state
            .config
            .env_key
            .as_deref()
            .unwrap_or_else(|| provider_default_env_key(&state.config.provider));
        let credentials = HashMap::from([(env_key.to_owned(), access_token)]);
        let _ = self
            .event_tx
            .send(CredentialEvent::Refreshed { account: account.to_owned(), credentials });

        Ok(())
    }

    /// Dynamically add a new account at runtime.
    ///
    /// Optionally seeds an API key and emits a `Refreshed` event (which
    /// triggers distribution to sessions).
    pub async fn add_account(
        self: &Arc<Self>,
        config: AccountConfig,
        access_token: Option<String>,
        _refresh_token: Option<String>,
        _expires_in: Option<u64>,
    ) -> anyhow::Result<()> {
        let name = config.name.clone();

        {
            let mut accounts = self.accounts.write().await;
            if accounts.contains_key(&name) {
                anyhow::bail!("account already exists: {name}");
            }

            let has_token = access_token.is_some();
            let status = if has_token { AccountStatus::Healthy } else { AccountStatus::Missing };

            let state = AccountState {
                config: config.clone(),
                status,
                api_key: access_token.clone(),
            };
            accounts.insert(name.clone(), state);

            // Emit Refreshed event if we have a token (triggers distribution).
            if let Some(ref token) = access_token {
                let env_key = config
                    .env_key
                    .as_deref()
                    .unwrap_or_else(|| provider_default_env_key(&config.provider));
                let credentials = HashMap::from([(env_key.to_owned(), token.clone())]);
                let _ = self
                    .event_tx
                    .send(CredentialEvent::Refreshed { account: name.clone(), credentials });
            }
        }

        // Initialize pool session count for the new account.
        self.session_counts
            .write()
            .await
            .entry(name.clone())
            .or_insert_with(|| AtomicU32::new(0));

        tracing::info!(account = %name, "account added");
        Ok(())
    }

    /// Get the first account name.
    pub async fn first_account_name(&self) -> Option<String> {
        self.accounts.read().await.keys().next().cloned()
    }

    /// Get the current credentials map for an account (env_key -> token).
    pub async fn get_credentials(&self, account: &str) -> Option<HashMap<String, String>> {
        let accounts = self.accounts.read().await;
        let state = accounts.get(account)?;
        let token = state.api_key.as_ref()?;
        let env_key = state
            .config
            .env_key
            .as_deref()
            .unwrap_or_else(|| provider_default_env_key(&state.config.provider));
        Some(HashMap::from([(env_key.to_owned(), token.clone())]))
    }

    /// Get status info for all accounts.
    pub async fn status_list(&self) -> Vec<AccountStatusInfo> {
        let accounts = self.accounts.read().await;
        accounts
            .iter()
            .map(|(name, state)| AccountStatusInfo {
                name: name.clone(),
                provider: state.config.provider.clone(),
                status: state.status,
                has_api_key: state.api_key.is_some(),
            })
            .collect()
    }

    // ── Pool load balancing ─────────────────────────────────────────────

    /// Pick the least-loaded healthy account for a new session.
    pub async fn assign_account(&self, preferred: Option<&str>) -> Option<String> {
        let accounts = self.accounts.read().await;
        let counts = self.session_counts.read().await;

        // Check preferred account first.
        if let Some(pref) = preferred {
            if let Some(state) = accounts.get(pref) {
                if state.status == AccountStatus::Healthy {
                    return Some(pref.to_owned());
                }
            }
        }

        // Find healthy account with lowest session count.
        let mut best: Option<(String, u32)> = None;
        for (name, state) in accounts.iter() {
            if state.status != AccountStatus::Healthy {
                continue;
            }
            let count = counts
                .get(name)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0);
            match &best {
                None => best = Some((name.clone(), count)),
                Some((_, best_count)) if count < *best_count => {
                    best = Some((name.clone(), count));
                }
                _ => {}
            }
        }
        best.map(|(name, _)| name)
    }

    /// Record that a session has been assigned to an account.
    pub async fn session_assigned(&self, account: &str) {
        let counts = self.session_counts.read().await;
        if let Some(counter) = counts.get(account) {
            counter.fetch_add(1, Ordering::Relaxed);
        } else {
            drop(counts);
            let mut counts = self.session_counts.write().await;
            counts
                .entry(account.to_owned())
                .or_insert_with(|| AtomicU32::new(0))
                .fetch_add(1, Ordering::Relaxed);
        }
        tracing::debug!(account, "pool: session assigned");
    }

    /// Record that a session has been unassigned from an account.
    pub async fn session_unassigned(&self, account: &str) {
        let counts = self.session_counts.read().await;
        if let Some(counter) = counts.get(account) {
            let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                if v > 0 { Some(v - 1) } else { None }
            });
        }
        tracing::debug!(account, "pool: session unassigned");
    }

    /// Get the pool status: per-account utilization info.
    pub async fn pool_status(&self) -> Vec<PoolAccountInfo> {
        let accounts = self.accounts.read().await;
        let counts = self.session_counts.read().await;

        accounts
            .iter()
            .map(|(name, state)| {
                let session_count = counts
                    .get(name)
                    .map(|c| c.load(Ordering::Relaxed))
                    .unwrap_or(0);
                PoolAccountInfo {
                    name: name.clone(),
                    provider: state.config.provider.clone(),
                    status: state.status,
                    session_count,
                    has_api_key: state.api_key.is_some(),
                }
            })
            .collect()
    }

    /// List accounts that have no API key configured.
    pub async fn unhealthy_accounts(&self) -> Vec<String> {
        let accounts = self.accounts.read().await;
        accounts
            .iter()
            .filter(|(_, state)| state.status != AccountStatus::Healthy)
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Get all healthy account names.
    pub async fn healthy_accounts(&self) -> Vec<String> {
        let accounts = self.accounts.read().await;
        accounts
            .iter()
            .filter(|(_, state)| state.status == AccountStatus::Healthy)
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Status info for an account (returned by the API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountStatusInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    pub has_api_key: bool,
}

/// Pool utilization info for an account (returned by the pool API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolAccountInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    pub session_count: u32,
    pub has_api_key: bool,
}
