// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker: manages account states, runs refresh loops, emits events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, RwLock};

use crate::credential::device_code;
use crate::credential::oauth::DeviceCodeResponse;
use crate::credential::persist::{PersistedAccount, PersistedCredentials};
use crate::credential::refresh::refresh_with_retries;
use crate::credential::{
    provider_default_env_key, AccountConfig, AccountStatus, CredentialConfig, CredentialEvent,
};

/// Runtime state for a single account.
struct AccountState {
    config: AccountConfig,
    status: AccountStatus,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: u64, // epoch seconds
}

/// The credential broker manages token freshness for all configured accounts.
pub struct CredentialBroker {
    accounts: RwLock<HashMap<String, AccountState>>,
    config: CredentialConfig,
    event_tx: broadcast::Sender<CredentialEvent>,
    http: reqwest::Client,
}

impl CredentialBroker {
    /// Create a new broker from config.
    pub fn new(
        config: CredentialConfig,
        event_tx: broadcast::Sender<CredentialEvent>,
    ) -> Arc<Self> {
        let mut accounts = HashMap::new();
        for acct in &config.accounts {
            let status = if acct.r#static { AccountStatus::Static } else { AccountStatus::Expired };
            accounts.insert(
                acct.name.clone(),
                AccountState {
                    config: acct.clone(),
                    status,
                    access_token: None,
                    refresh_token: None,
                    expires_at: 0,
                },
            );
        }
        Arc::new(Self {
            accounts: RwLock::new(accounts),
            config,
            event_tx,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        })
    }

    /// Get a reference to the credential config.
    pub fn config(&self) -> &CredentialConfig {
        &self.config
    }

    /// Load persisted credentials and seed account states.
    pub async fn load_persisted(&self, creds: &PersistedCredentials) {
        let mut accounts = self.accounts.write().await;
        for (name, persisted) in &creds.accounts {
            if let Some(state) = accounts.get_mut(name) {
                state.access_token = Some(persisted.access_token.clone());
                state.refresh_token = persisted.refresh_token.clone();
                state.expires_at = persisted.expires_at;
                if state.config.r#static {
                    state.status = AccountStatus::Static;
                } else if persisted.expires_at > epoch_secs() {
                    state.status = AccountStatus::Healthy;
                } else {
                    state.status = AccountStatus::Expired;
                }
            }
        }
    }

    /// Seed initial tokens for an account (from API call).
    pub async fn seed(
        &self,
        account: &str,
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    ) -> anyhow::Result<()> {
        let mut accounts = self.accounts.write().await;
        let state = accounts
            .get_mut(account)
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account}"))?;

        state.access_token = Some(access_token.clone());
        state.refresh_token = refresh_token;
        state.expires_at = expires_in.map(|s| epoch_secs() + s).unwrap_or(0);
        state.status = AccountStatus::Healthy;

        // Build credentials map and emit event.
        let env_key = state
            .config
            .env_key
            .as_deref()
            .unwrap_or_else(|| provider_default_env_key(&state.config.provider));
        let credentials = HashMap::from([(env_key.to_owned(), access_token)]);
        let _ = self
            .event_tx
            .send(CredentialEvent::Refreshed { account: account.to_owned(), credentials });

        // Persist if configured.
        self.persist(&accounts).await;
        Ok(())
    }

    /// Get status info for all accounts.
    pub async fn status_list(&self) -> Vec<AccountStatusInfo> {
        let accounts = self.accounts.read().await;
        let now = epoch_secs();
        accounts
            .iter()
            .map(|(name, state)| {
                let expires_in =
                    if state.expires_at > now { Some(state.expires_at - now) } else { None };
                AccountStatusInfo {
                    name: name.clone(),
                    provider: state.config.provider.clone(),
                    status: state.status,
                    expires_in_secs: expires_in,
                    has_refresh_token: state.refresh_token.is_some(),
                }
            })
            .collect()
    }

    /// Initiate the device code re-authentication flow for an account.
    ///
    /// Returns the device code response with user code and verification URL.
    /// Spawns a background task that polls for authorization completion and
    /// seeds credentials on success.
    pub async fn initiate_reauth(
        self: &Arc<Self>,
        account_name: &str,
    ) -> anyhow::Result<DeviceCodeResponse> {
        // Look up the account config.
        let acct_config = self
            .config
            .accounts
            .iter()
            .find(|a| a.name == account_name)
            .ok_or_else(|| anyhow::anyhow!("unknown account: {account_name}"))?;

        let device_auth_url = acct_config
            .device_auth_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no device_auth_url configured for {account_name}"))?;
        let client_id = acct_config
            .client_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no client_id configured for {account_name}"))?;
        let token_url = acct_config
            .token_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no token_url configured for {account_name}"))?;

        let resp = device_code::initiate_reauth(&self.http, device_auth_url, client_id).await?;

        // Emit ReauthRequired event.
        let _ = self.event_tx.send(CredentialEvent::ReauthRequired {
            account: account_name.to_owned(),
            auth_url: resp.verification_uri.clone(),
            user_code: resp.user_code.clone(),
        });

        // Spawn background poll task.
        let broker = Arc::clone(self);
        let name = account_name.to_owned();
        let device_code = resp.device_code.clone();
        let interval = resp.interval;
        let expires_in = resp.expires_in;
        let poll_token_url = token_url.to_owned();
        let poll_client_id = client_id.to_owned();

        tokio::spawn(async move {
            match device_code::poll_device_code(
                &broker.http,
                &poll_token_url,
                &poll_client_id,
                &device_code,
                interval,
                expires_in,
            )
            .await
            {
                Ok(token) => {
                    if let Err(e) = broker
                        .seed(&name, token.access_token, token.refresh_token, Some(token.expires_in))
                        .await
                    {
                        tracing::warn!(account = %name, err = %e, "failed to seed after reauth");
                    } else {
                        tracing::info!(account = %name, "reauth completed, credentials seeded");
                    }
                }
                Err(e) => {
                    let _ = broker.event_tx.send(CredentialEvent::RefreshFailed {
                        account: name.clone(),
                        error: e.to_string(),
                    });
                    tracing::warn!(account = %name, err = %e, "device code polling failed");
                }
            }
        });

        Ok(resp)
    }

    /// Spawn refresh loops for all non-static accounts.
    pub fn spawn_refresh_loops(self: &Arc<Self>) {
        let accounts_snapshot: Vec<String> = {
            // We can't hold the lock across await, so collect names first.
            // We'll read config from the hashmap in the loop.
            self.config.accounts.iter().filter(|a| !a.r#static).map(|a| a.name.clone()).collect()
        };

        for name in accounts_snapshot {
            let broker = Arc::clone(self);
            tokio::spawn(async move {
                broker.refresh_loop(&name).await;
            });
        }
    }

    /// Refresh loop for a single account.
    async fn refresh_loop(&self, account_name: &str) {
        loop {
            let (token_url, client_id, refresh_token, margin, expires_at) = {
                let accounts = self.accounts.read().await;
                let Some(state) = accounts.get(account_name) else {
                    return;
                };
                if state.config.r#static {
                    return;
                }
                let token_url = match state.config.token_url.as_deref() {
                    Some(u) => u.to_owned(),
                    None => {
                        // No token URL configured — wait and retry.
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };
                let client_id = state.config.client_id.clone().unwrap_or_default();
                let refresh_token = match &state.refresh_token {
                    Some(rt) => rt.clone(),
                    None => {
                        // No refresh token — mark expired and wait.
                        drop(accounts);
                        let mut accounts = self.accounts.write().await;
                        if let Some(s) = accounts.get_mut(account_name) {
                            s.status = AccountStatus::Expired;
                        }
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };
                (
                    token_url,
                    client_id,
                    refresh_token,
                    state.config.refresh_margin_secs,
                    state.expires_at,
                )
            };

            // Calculate sleep until margin before expiry.
            let now = epoch_secs();
            let refresh_at = expires_at.saturating_sub(margin);
            if refresh_at > now {
                tokio::time::sleep(Duration::from_secs(refresh_at - now)).await;
            }

            // Mark refreshing.
            {
                let mut accounts = self.accounts.write().await;
                if let Some(s) = accounts.get_mut(account_name) {
                    s.status = AccountStatus::Refreshing;
                }
            }

            // Attempt refresh.
            match refresh_with_retries(&self.http, &token_url, &client_id, &refresh_token, 5).await
            {
                Ok(token) => {
                    let mut accounts = self.accounts.write().await;
                    if let Some(state) = accounts.get_mut(account_name) {
                        state.access_token = Some(token.access_token.clone());
                        if let Some(rt) = token.refresh_token {
                            state.refresh_token = Some(rt);
                        }
                        state.expires_at = epoch_secs() + token.expires_in;
                        state.status = AccountStatus::Healthy;

                        let env_key =
                            state.config.env_key.as_deref().unwrap_or_else(|| {
                                provider_default_env_key(&state.config.provider)
                            });
                        let credentials =
                            HashMap::from([(env_key.to_owned(), token.access_token.clone())]);
                        let _ = self.event_tx.send(CredentialEvent::Refreshed {
                            account: account_name.to_owned(),
                            credentials,
                        });
                    }
                    self.persist(&accounts).await;
                    tracing::info!(account = %account_name, "credentials refreshed");
                }
                Err(e) => {
                    let mut accounts = self.accounts.write().await;
                    if let Some(s) = accounts.get_mut(account_name) {
                        s.status = AccountStatus::Expired;
                    }
                    let _ = self.event_tx.send(CredentialEvent::RefreshFailed {
                        account: account_name.to_owned(),
                        error: e.to_string(),
                    });
                    tracing::warn!(account = %account_name, err = %e, "credential refresh failed");
                    // Retry in 60 seconds.
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            }
        }
    }

    /// Persist current credentials to disk if configured.
    async fn persist(&self, accounts: &HashMap<String, AccountState>) {
        let Some(ref path) = self.config.persist_path else {
            return;
        };
        let mut persisted = PersistedCredentials::default();
        for (name, state) in accounts {
            if let Some(ref token) = state.access_token {
                persisted.accounts.insert(
                    name.clone(),
                    PersistedAccount {
                        access_token: token.clone(),
                        refresh_token: state.refresh_token.clone(),
                        expires_at: state.expires_at,
                    },
                );
            }
        }
        if let Err(e) = crate::credential::persist::save(path, &persisted) {
            tracing::warn!(err = %e, "failed to persist credentials");
        }
    }
}

/// Status info for an account (returned by the API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountStatusInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    pub has_refresh_token: bool,
}

fn epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
