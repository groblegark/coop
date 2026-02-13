// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker: manages account states, runs refresh loops, emits events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, RwLock};

use crate::credential::persist::{PersistedAccount, PersistedCredentials};
use crate::credential::refresh::refresh_with_retries;
use crate::credential::{
    provider_default_auth_url, provider_default_client_id, provider_default_env_key,
    provider_default_scopes, provider_default_token_url, AccountConfig, AccountStatus,
    CredentialConfig, CredentialEvent,
};

/// Set of account names that were defined in the original static config file
/// (used to distinguish dynamic accounts for persistence).
type StaticNames = std::collections::HashSet<String>;

/// Runtime state for a single account.
struct AccountState {
    config: AccountConfig,
    status: AccountStatus,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: u64, // epoch seconds
}

/// In-flight OAuth authorization code + PKCE flow.
struct PendingAuth {
    account: String,
    code_verifier: String,
    redirect_uri: String,
    token_url: String,
    client_id: String,
}

/// The credential broker manages token freshness for all configured accounts.
pub struct CredentialBroker {
    accounts: RwLock<HashMap<String, AccountState>>,
    /// Account names from the original static config (for persistence filtering).
    static_names: StaticNames,
    /// Pending OAuth authorization code flows, keyed by `state` parameter.
    pending_auths: RwLock<HashMap<String, PendingAuth>>,
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
        let mut static_names = StaticNames::new();
        for acct in &config.accounts {
            static_names.insert(acct.name.clone());
            accounts.insert(
                acct.name.clone(),
                AccountState {
                    config: acct.clone(),
                    status: AccountStatus::Expired,
                    access_token: None,
                    refresh_token: None,
                    expires_at: 0,
                },
            );
        }
        Arc::new(Self {
            accounts: RwLock::new(accounts),
            static_names,
            pending_auths: RwLock::new(HashMap::new()),
            event_tx,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        })
    }

    /// Load persisted credentials and seed account states.
    ///
    /// Also restores dynamic accounts that were added at runtime in previous sessions.
    pub async fn load_persisted(&self, creds: &PersistedCredentials) {
        let mut accounts = self.accounts.write().await;

        // Restore dynamic account configs first (so token seeding below finds them).
        for acct_config in &creds.dynamic_accounts {
            if !accounts.contains_key(&acct_config.name) {
                accounts.insert(
                    acct_config.name.clone(),
                    AccountState {
                        config: acct_config.clone(),
                        status: AccountStatus::Expired,
                        access_token: None,
                        refresh_token: None,
                        expires_at: 0,
                    },
                );
            }
        }

        // Seed persisted tokens into all known accounts.
        for (name, persisted) in &creds.accounts {
            if let Some(state) = accounts.get_mut(name) {
                state.access_token = Some(persisted.access_token.clone());
                state.refresh_token = persisted.refresh_token.clone();
                state.expires_at = persisted.expires_at;
                // expires_at == 0 means no expiry (long-lived token).
                if persisted.expires_at == 0 || persisted.expires_at > epoch_secs() {
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

    /// Dynamically add a new account at runtime.
    ///
    /// Optionally seeds tokens and emits a `Refreshed` event (which triggers
    /// distribution to sessions). Spawns a refresh loop for the new account.
    /// Returns an error if the account name is already taken.
    pub async fn add_account(
        self: &Arc<Self>,
        config: AccountConfig,
        access_token: Option<String>,
        refresh_token: Option<String>,
        expires_in: Option<u64>,
    ) -> anyhow::Result<()> {
        let name = config.name.clone();

        {
            let mut accounts = self.accounts.write().await;
            if accounts.contains_key(&name) {
                anyhow::bail!("account already exists: {name}");
            }

            let has_token = access_token.is_some();
            let expires_at = expires_in.map(|s| epoch_secs() + s).unwrap_or(0);
            let status = if has_token && (expires_in.is_none() || expires_at > epoch_secs()) {
                AccountStatus::Healthy
            } else {
                AccountStatus::Expired
            };

            let state = AccountState {
                config: config.clone(),
                status,
                access_token: access_token.clone(),
                refresh_token,
                expires_at,
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

            self.persist(&accounts).await;
        }

        // Spawn a refresh loop for the new account.
        let broker = Arc::clone(self);
        let loop_name = name.clone();
        tokio::spawn(async move {
            broker.refresh_loop(&loop_name).await;
        });

        tracing::info!(account = %name, "dynamic account added");
        Ok(())
    }

    /// Get the first account name (for default selection in reauth).
    pub async fn first_account_name(&self) -> Option<String> {
        self.accounts.read().await.keys().next().cloned()
    }

    /// Get the current credentials map for an account (env_key -> token).
    pub async fn get_credentials(&self, account: &str) -> Option<HashMap<String, String>> {
        let accounts = self.accounts.read().await;
        let state = accounts.get(account)?;
        let token = state.access_token.as_ref()?;
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

    /// Initiate OAuth reauth flow for an account.
    ///
    /// If the account has `device_auth_url` set, uses device code flow (RFC 8628).
    /// Otherwise uses authorization code + PKCE. The `redirect_uri` is optional —
    /// providers with a registered platform redirect (e.g. Claude) don't need one.
    pub async fn initiate_reauth(
        self: &Arc<Self>,
        account_name: &str,
    ) -> anyhow::Result<ReauthResponse> {
        let has_device_auth = {
            let accounts = self.accounts.read().await;
            let acct_state = accounts
                .get(account_name)
                .ok_or_else(|| anyhow::anyhow!("unknown account: {account_name}"))?;
            acct_state.config.device_auth_url.is_some()
        };

        if has_device_auth {
            self.initiate_device_code_reauth(account_name).await
        } else {
            self.initiate_pkce_reauth(account_name).await
        }
    }

    /// Initiate device code flow (RFC 8628).
    ///
    /// Emits `ReauthRequired` with `user_code`, then spawns a background task
    /// that polls the token endpoint and auto-seeds on success.
    async fn initiate_device_code_reauth(
        self: &Arc<Self>,
        account_name: &str,
    ) -> anyhow::Result<ReauthResponse> {
        let (device_auth_url, client_id, token_url, scope) = {
            let accounts = self.accounts.read().await;
            let acct_state = accounts
                .get(account_name)
                .ok_or_else(|| anyhow::anyhow!("unknown account: {account_name}"))?;
            let cfg = &acct_state.config;

            let device_auth_url = cfg
                .device_auth_url
                .clone()
                .ok_or_else(|| anyhow::anyhow!("no device_auth_url for {account_name}"))?;
            let client_id = cfg
                .client_id
                .clone()
                .or_else(|| provider_default_client_id(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no client_id configured for {account_name}"))?;
            let token_url = cfg
                .token_url
                .clone()
                .or_else(|| provider_default_token_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no token_url configured for {account_name}"))?;
            let scope = provider_default_scopes(&cfg.provider).to_owned();
            (device_auth_url, client_id, token_url, scope)
        };

        let device = crate::credential::device_code::initiate_device_auth(
            &self.http,
            &device_auth_url,
            &client_id,
            &scope,
        )
        .await?;

        let verification_uri = device.verification_uri.clone();
        let user_code = device.user_code.clone();

        // Emit ReauthRequired with user_code for display.
        let _ = self.event_tx.send(CredentialEvent::ReauthRequired {
            account: account_name.to_owned(),
            auth_url: verification_uri.clone(),
            user_code: Some(user_code.clone()),
        });

        // Spawn background poll task.
        let broker = Arc::clone(self);
        let poll_account = account_name.to_owned();
        let poll_client_id = client_id;
        let poll_token_url = token_url;
        tokio::spawn(async move {
            match crate::credential::device_code::poll_device_code(
                &broker.http,
                &poll_token_url,
                &poll_client_id,
                &device.device_code,
                device.interval,
                device.expires_in,
            )
            .await
            {
                Ok(token) => {
                    if let Err(e) = broker
                        .seed(
                            &poll_account,
                            token.access_token,
                            token.refresh_token,
                            Some(token.expires_in),
                        )
                        .await
                    {
                        tracing::warn!(account = %poll_account, err = %e, "failed to seed after device code auth");
                    } else {
                        tracing::info!(account = %poll_account, "device code auth completed, credentials seeded");
                    }
                }
                Err(e) => {
                    tracing::warn!(account = %poll_account, err = %e, "device code polling failed");
                    let _ = broker.event_tx.send(CredentialEvent::RefreshFailed {
                        account: poll_account,
                        error: e.to_string(),
                    });
                }
            }
        });

        Ok(ReauthResponse {
            account: account_name.to_owned(),
            auth_url: verification_uri,
            user_code: Some(user_code),
            state: None,
        })
    }

    /// Initiate authorization code + PKCE flow.
    async fn initiate_pkce_reauth(&self, account_name: &str) -> anyhow::Result<ReauthResponse> {
        use crate::credential::pkce;
        use crate::credential::provider_default_redirect_uri;

        let (auth_url, client_id, token_url, scope, redirect_uri) = {
            let accounts = self.accounts.read().await;
            let acct_state = accounts
                .get(account_name)
                .ok_or_else(|| anyhow::anyhow!("unknown account: {account_name}"))?;
            let cfg = &acct_state.config;

            let auth_url = cfg
                .auth_url
                .clone()
                .or_else(|| provider_default_auth_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no auth_url configured for {account_name}"))?;
            let client_id = cfg
                .client_id
                .clone()
                .or_else(|| provider_default_client_id(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no client_id configured for {account_name}"))?;
            let token_url = cfg
                .token_url
                .clone()
                .or_else(|| provider_default_token_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no token_url configured for {account_name}"))?;
            let scope = provider_default_scopes(&cfg.provider).to_owned();
            let redirect_uri = provider_default_redirect_uri(&cfg.provider)
                .map(String::from)
                .ok_or_else(|| anyhow::anyhow!("no redirect_uri configured for {account_name}"))?;
            (auth_url, client_id, token_url, scope, redirect_uri)
        };

        let code_verifier = pkce::generate_code_verifier();
        let code_challenge = pkce::compute_code_challenge(&code_verifier);
        let state = pkce::generate_state();

        let full_auth_url = pkce::build_auth_url(
            &auth_url,
            &client_id,
            &redirect_uri,
            &scope,
            &code_challenge,
            &state,
        );

        // Store pending auth for code exchange.
        self.pending_auths.write().await.insert(
            state.clone(),
            PendingAuth {
                account: account_name.to_owned(),
                code_verifier,
                redirect_uri,
                token_url,
                client_id,
            },
        );

        // Emit ReauthRequired event.
        let _ = self.event_tx.send(CredentialEvent::ReauthRequired {
            account: account_name.to_owned(),
            auth_url: full_auth_url.clone(),
            user_code: None,
        });

        Ok(ReauthResponse {
            account: account_name.to_owned(),
            auth_url: full_auth_url,
            user_code: None,
            state: Some(state),
        })
    }

    /// Complete an OAuth authorization code exchange.
    ///
    /// Called from the callback endpoint with the `code` and `state` returned
    /// by the authorization server.
    pub async fn complete_reauth(&self, state: &str, code: &str) -> anyhow::Result<()> {
        let pending = self
            .pending_auths
            .write()
            .await
            .remove(state)
            .ok_or_else(|| anyhow::anyhow!("unknown or expired auth state"))?;

        let token = crate::credential::pkce::exchange_code(
            &self.http,
            &pending.token_url,
            &pending.client_id,
            code,
            &pending.code_verifier,
            &pending.redirect_uri,
        )
        .await?;

        self.seed(
            &pending.account,
            token.access_token,
            token.refresh_token,
            Some(token.expires_in),
        )
        .await?;

        tracing::info!(account = %pending.account, "reauth completed, credentials seeded");
        Ok(())
    }

    /// Spawn refresh loops for all currently registered accounts.
    pub fn spawn_refresh_loops(self: &Arc<Self>) {
        let broker = Arc::clone(self);
        tokio::spawn(async move {
            let accounts_snapshot: Vec<String> =
                broker.accounts.read().await.keys().cloned().collect();
            for name in accounts_snapshot {
                let b = Arc::clone(&broker);
                tokio::spawn(async move {
                    b.refresh_loop(&name).await;
                });
            }
        });
    }

    /// Refresh loop for a single account.
    async fn refresh_loop(self: &Arc<Self>, account_name: &str) {
        let margin = crate::credential::refresh_margin_secs();
        loop {
            let (token_url, client_id, refresh_token, expires_at) = {
                let accounts = self.accounts.read().await;
                let Some(state) = accounts.get(account_name) else {
                    return;
                };
                let token_url = match state
                    .config
                    .token_url
                    .as_deref()
                    .or_else(|| provider_default_token_url(&state.config.provider))
                {
                    Some(u) => u.to_owned(),
                    None => {
                        // No token URL configured — drop lock before sleeping.
                        drop(accounts);
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };
                let client_id = state
                    .config
                    .client_id
                    .clone()
                    .or_else(|| {
                        provider_default_client_id(&state.config.provider).map(String::from)
                    })
                    .unwrap_or_default();
                let refresh_token = match &state.refresh_token {
                    Some(rt) => rt.clone(),
                    None => {
                        // No refresh token. If token has no expiry (long-lived)
                        // or hasn't expired yet, keep current status and wait.
                        // Otherwise mark expired.
                        let is_expired = state.expires_at > 0 && state.expires_at <= epoch_secs();
                        drop(accounts);
                        if is_expired {
                            let mut accounts = self.accounts.write().await;
                            if let Some(s) = accounts.get_mut(account_name) {
                                s.status = AccountStatus::Expired;
                            }
                        }
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                };
                (token_url, client_id, refresh_token, state.expires_at)
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
                    let err_str = e.to_string();
                    let has_device_auth = {
                        let mut accounts = self.accounts.write().await;
                        if let Some(s) = accounts.get_mut(account_name) {
                            s.status = AccountStatus::Expired;
                        }
                        accounts
                            .get(account_name)
                            .and_then(|s| s.config.device_auth_url.as_ref())
                            .is_some()
                    };

                    let _ = self.event_tx.send(CredentialEvent::RefreshFailed {
                        account: account_name.to_owned(),
                        error: err_str.clone(),
                    });
                    tracing::warn!(account = %account_name, err = %err_str, "credential refresh failed");

                    if err_str.contains("invalid_grant") && has_device_auth {
                        // Auto-start device code reauth for headless environments.
                        tracing::info!(account = %account_name, "invalid_grant detected, initiating device code reauth");
                        if let Err(re) = self.initiate_reauth(account_name).await {
                            tracing::warn!(account = %account_name, err = %re, "auto-reauth failed");
                        }
                        // Give user time to complete device code authorization.
                        tokio::time::sleep(Duration::from_secs(300)).await;
                    } else {
                        // Retry in 60 seconds.
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                }
            }
        }
    }

    /// Persist current credentials to disk.
    async fn persist(&self, accounts: &HashMap<String, AccountState>) {
        let dir = crate::credential::state_dir();
        let path = dir.join("credentials.json");
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!(err = %e, "failed to create state dir");
                return;
            }
        }
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
            // Save config for accounts not in the original static config.
            if !self.static_names.contains(name) {
                persisted.dynamic_accounts.push(state.config.clone());
            }
        }
        if let Err(e) = crate::credential::persist::save(&path, &persisted) {
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

/// Response from initiating a reauth flow.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReauthResponse {
    pub account: String,
    pub auth_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    /// PKCE `state` parameter — returned so the UI can submit the authorization
    /// code via the exchange endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

fn epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
