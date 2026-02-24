// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker: static API key store with pool load balancing.
//!
//! Accounts are loaded from `--credential-config` at startup. Each account
//! holds a long-lived API key (no refresh loops, no OAuth flows). Keys can
//! be set at runtime via `set_token()` or `add_account()`.
//!
//! When the `legacy-oauth` feature is enabled, the broker also manages OAuth
//! token refresh loops, PKCE flows, device code authorization, and credential
//! persistence.

use std::collections::HashMap;
#[cfg(feature = "legacy-oauth")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
#[cfg(feature = "legacy-oauth")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, RwLock};

#[cfg(feature = "legacy-oauth")]
use crate::credential::persist::{PersistedAccount, PersistedCredentials};
#[cfg(feature = "legacy-oauth")]
use crate::credential::refresh::refresh_with_retries;
#[cfg(feature = "legacy-oauth")]
use crate::credential::{
    provider_default_client_id, provider_default_device_auth_url,
    provider_default_device_token_url, provider_default_pkce_auth_url,
    provider_default_pkce_token_url, provider_default_scopes,
};
use crate::credential::{
    provider_default_env_key, AccountConfig, AccountStatus, CredentialConfig, CredentialEvent,
};

/// Set of account names that were defined in the original static config file
/// (used to distinguish dynamic accounts for persistence).
#[cfg(feature = "legacy-oauth")]
type StaticNames = std::collections::HashSet<String>;

/// Runtime state for a single account.
struct AccountState {
    config: AccountConfig,
    status: AccountStatus,
    /// The API key or token for this account.
    api_key: Option<String>,
    #[cfg(feature = "legacy-oauth")]
    refresh_token: Option<String>,
    #[cfg(feature = "legacy-oauth")]
    expires_at: u64, // epoch seconds
}

/// In-flight OAuth authorization code + PKCE flow.
#[cfg(feature = "legacy-oauth")]
struct PendingAuth {
    account: String,
    code_verifier: String,
    redirect_uri: String,
    token_url: String,
    client_id: String,
    state: String,
    /// Full authorization URL (for reuse when same account is requested again).
    auth_url: String,
}

/// The credential broker manages API keys for configured accounts.
///
/// Without `legacy-oauth`: simple static key store, no refresh loops.
/// With `legacy-oauth`: full OAuth token management with refresh, PKCE, device code.
pub struct CredentialBroker {
    accounts: RwLock<HashMap<String, AccountState>>,
    #[cfg(feature = "legacy-oauth")]
    static_names: StaticNames,
    #[cfg(feature = "legacy-oauth")]
    pending_auths: RwLock<HashMap<String, PendingAuth>>,
    event_tx: broadcast::Sender<CredentialEvent>,
    #[cfg(feature = "legacy-oauth")]
    http: reqwest::Client,
    #[cfg(feature = "legacy-oauth")]
    persist_dir: Option<PathBuf>,
    /// Per-account session counts for pool load balancing.
    session_counts: RwLock<HashMap<String, AtomicU32>>,
}

// ── Constructor ─────────────────────────────────────────────────────────

#[cfg(not(feature = "legacy-oauth"))]
impl CredentialBroker {
    /// Create a new broker from config (static API key mode).
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
}

#[cfg(feature = "legacy-oauth")]
impl CredentialBroker {
    /// Create a new broker from config (legacy OAuth mode).
    ///
    /// `persist_dir` controls where credentials are saved to disk. Pass `None`
    /// to disable persistence entirely (useful in tests).
    pub fn new(
        config: CredentialConfig,
        event_tx: broadcast::Sender<CredentialEvent>,
        persist_dir: Option<PathBuf>,
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
                    api_key: None,
                    refresh_token: None,
                    expires_at: 0,
                },
            );
        }
        let session_counts: HashMap<String, AtomicU32> =
            accounts.keys().map(|name| (name.clone(), AtomicU32::new(0))).collect();

        Arc::new(Self {
            accounts: RwLock::new(accounts),
            static_names,
            pending_auths: RwLock::new(HashMap::new()),
            event_tx,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
                .build()
                .unwrap_or_default(),
            persist_dir,
            session_counts: RwLock::new(session_counts),
        })
    }
}

// ── Shared methods (both modes) ─────────────────────────────────────────

impl CredentialBroker {
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
            let count = counts.get(name).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0);
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
                if v > 0 {
                    Some(v - 1)
                } else {
                    None
                }
            });
        }
        tracing::debug!(account, "pool: session unassigned");
    }

    /// List accounts that are unhealthy.
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

// ── Static-only methods (default, no legacy-oauth) ──────────────────────

#[cfg(not(feature = "legacy-oauth"))]
impl CredentialBroker {
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

            let state =
                AccountState { config: config.clone(), status, api_key: access_token.clone() };
            accounts.insert(name.clone(), state);

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

        self.session_counts.write().await.entry(name.clone()).or_insert_with(|| AtomicU32::new(0));

        tracing::info!(account = %name, "account added");
        Ok(())
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

    /// Get the pool status: per-account utilization info.
    pub async fn pool_status(&self) -> Vec<PoolAccountInfo> {
        let accounts = self.accounts.read().await;
        let counts = self.session_counts.read().await;

        accounts
            .iter()
            .map(|(name, state)| {
                let session_count =
                    counts.get(name).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0);
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
}

// ── Legacy OAuth methods ────────────────────────────────────────────────

#[cfg(feature = "legacy-oauth")]
impl CredentialBroker {
    /// Load persisted credentials and seed account states.
    pub async fn load_persisted(&self, creds: &PersistedCredentials) {
        let mut accounts = self.accounts.write().await;

        // Restore dynamic account configs first.
        for acct_config in &creds.dynamic_accounts {
            if !accounts.contains_key(&acct_config.name) {
                accounts.insert(
                    acct_config.name.clone(),
                    AccountState {
                        config: acct_config.clone(),
                        status: AccountStatus::Expired,
                        api_key: None,
                        refresh_token: None,
                        expires_at: 0,
                    },
                );
            }
        }

        for (name, persisted) in &creds.accounts {
            if let Some(state) = accounts.get_mut(name) {
                state.api_key = Some(persisted.access_token.clone());
                state.refresh_token = persisted.refresh_token.clone();
                state.expires_at = persisted.expires_at;
                if persisted.expires_at > epoch_secs() {
                    state.status = AccountStatus::Healthy;
                } else {
                    state.status = AccountStatus::Expired;
                }
            }
        }
    }

    /// Set tokens for an account.
    pub async fn set_token(
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

        state.api_key = Some(access_token.clone());
        state.refresh_token = refresh_token;
        state.expires_at = if state.config.reauth {
            epoch_secs() + expires_in.unwrap_or(DEFAULT_EXPIRES_IN)
        } else {
            0
        };
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

        self.persist(&accounts).await;
        Ok(())
    }

    /// Dynamically add a new account at runtime.
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
            let expires_at = if !config.reauth {
                0
            } else if has_token {
                epoch_secs() + expires_in.unwrap_or(DEFAULT_EXPIRES_IN)
            } else {
                0
            };
            let status = if has_token { AccountStatus::Healthy } else { AccountStatus::Expired };

            let state = AccountState {
                config: config.clone(),
                status,
                api_key: access_token.clone(),
                refresh_token,
                expires_at,
            };
            accounts.insert(name.clone(), state);

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

        self.session_counts.write().await.entry(name.clone()).or_insert_with(|| AtomicU32::new(0));

        // Spawn a refresh loop for the new account.
        let broker = Arc::clone(self);
        let loop_name = name.clone();
        tokio::spawn(async move {
            broker.refresh_loop(&loop_name).await;
        });

        tracing::info!(account = %name, "dynamic account added");
        Ok(())
    }

    /// Get status info for all accounts.
    pub async fn status_list(&self) -> Vec<AccountStatusInfo> {
        let accounts = self.accounts.read().await;
        let now = epoch_secs();
        accounts
            .iter()
            .map(|(name, state)| {
                let expires_in = if state.config.reauth
                    && state.status != AccountStatus::Expired
                    && state.expires_at > now
                {
                    Some(state.expires_at - now)
                } else {
                    None
                };
                AccountStatusInfo {
                    name: name.clone(),
                    provider: state.config.provider.clone(),
                    status: state.status,
                    has_api_key: state.api_key.is_some(),
                    expires_in_secs: expires_in,
                    has_refresh_token: state.refresh_token.is_some(),
                    reauth: state.config.reauth,
                }
            })
            .collect()
    }

    /// Get the pool status: per-account utilization info.
    pub async fn pool_status(&self) -> Vec<PoolAccountInfo> {
        let accounts = self.accounts.read().await;
        let counts = self.session_counts.read().await;
        let now = epoch_secs();

        accounts
            .iter()
            .map(|(name, state)| {
                let session_count =
                    counts.get(name).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0);
                let expires_in =
                    if state.expires_at > now { Some(state.expires_at - now) } else { None };
                PoolAccountInfo {
                    name: name.clone(),
                    provider: state.config.provider.clone(),
                    status: state.status,
                    session_count,
                    has_api_key: state.api_key.is_some(),
                    expires_in_secs: expires_in,
                    has_refresh_token: state.refresh_token.is_some(),
                }
            })
            .collect()
    }

    // ── OAuth reauth ──────────────────────────────────────────────────────

    /// Initiate OAuth reauth flow for an account.
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
                || provider_default_device_auth_url(&acct_state.config.provider).is_some()
        };

        if has_device_auth {
            match self.initiate_device_code_reauth(account_name).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    tracing::warn!(account = %account_name, err = %e, "device code flow failed, falling back to PKCE");
                }
            }
        }

        self.initiate_pkce_reauth(account_name).await
    }

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
                .or_else(|| provider_default_device_auth_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no device_auth_url for {account_name}"))?;
            let client_id = cfg
                .client_id
                .clone()
                .or_else(|| provider_default_client_id(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no client_id configured for {account_name}"))?;
            let token_url = cfg
                .token_url
                .clone()
                .or_else(|| provider_default_device_token_url(&cfg.provider).map(String::from))
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

        let _ = self.event_tx.send(CredentialEvent::ReauthRequired {
            account: account_name.to_owned(),
            auth_url: verification_uri.clone(),
            user_code: Some(user_code.clone()),
        });

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
                        .set_token(
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
                .or_else(|| provider_default_pkce_auth_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no auth_url configured for {account_name}"))?;
            let client_id = cfg
                .client_id
                .clone()
                .or_else(|| provider_default_client_id(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no client_id configured for {account_name}"))?;
            let token_url = cfg
                .token_url
                .clone()
                .or_else(|| provider_default_pkce_token_url(&cfg.provider).map(String::from))
                .or_else(|| provider_default_device_token_url(&cfg.provider).map(String::from))
                .ok_or_else(|| anyhow::anyhow!("no token_url configured for {account_name}"))?;
            let scope = provider_default_scopes(&cfg.provider).to_owned();
            let redirect_uri = provider_default_redirect_uri(&cfg.provider)
                .map(String::from)
                .ok_or_else(|| anyhow::anyhow!("no redirect_uri configured for {account_name}"))?;
            (auth_url, client_id, token_url, scope, redirect_uri)
        };

        // Reuse existing pending auth for the same account.
        {
            let existing = self.pending_auths.read().await;
            for pending in existing.values() {
                if pending.account == account_name {
                    tracing::debug!(account = %account_name, state = %pending.state, "reusing existing PKCE session");
                    return Ok(ReauthResponse {
                        account: account_name.to_owned(),
                        auth_url: pending.auth_url.clone(),
                        user_code: None,
                        state: Some(pending.state.clone()),
                    });
                }
            }
        }

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

        self.pending_auths.write().await.insert(
            state.clone(),
            PendingAuth {
                account: account_name.to_owned(),
                code_verifier,
                redirect_uri,
                token_url,
                client_id,
                state: state.clone(),
                auth_url: full_auth_url.clone(),
            },
        );

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
            &pending.state,
        )
        .await?;

        self.set_token(
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

    async fn refresh_loop(self: &Arc<Self>, account_name: &str) {
        let margin = crate::credential::refresh_margin_secs();
        loop {
            let (token_url, client_id, refresh_token, expires_at) = {
                let accounts = self.accounts.read().await;
                let Some(state) = accounts.get(account_name) else {
                    return;
                };
                if !state.config.reauth {
                    drop(accounts);
                    tokio::time::sleep(Duration::from_secs(300)).await;
                    continue;
                }
                let token_url = match state
                    .config
                    .token_url
                    .as_deref()
                    .or_else(|| provider_default_device_token_url(&state.config.provider))
                {
                    Some(u) => u.to_owned(),
                    None => {
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
                        let is_expired = state.expires_at > 0 && state.expires_at <= epoch_secs();
                        let is_new_account = state.expires_at == 0 && state.api_key.is_none();
                        let needs_reauth = is_expired || is_new_account;
                        drop(accounts);
                        if needs_reauth {
                            let mut accounts = self.accounts.write().await;
                            if let Some(s) = accounts.get_mut(account_name) {
                                s.status = AccountStatus::Expired;
                            }
                            drop(accounts);
                            tracing::info!(account = %account_name, "no refresh token, auto-initiating reauth");
                            if let Err(e) = self.initiate_reauth(account_name).await {
                                tracing::warn!(account = %account_name, err = %e, "auto-reauth initiation failed");
                            }
                            tokio::time::sleep(Duration::from_secs(300)).await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                        continue;
                    }
                };
                (token_url, client_id, refresh_token, state.expires_at)
            };

            let now = epoch_secs();
            let refresh_at = expires_at.saturating_sub(margin);
            if refresh_at > now {
                tokio::time::sleep(Duration::from_secs(refresh_at - now)).await;
            }

            {
                let mut accounts = self.accounts.write().await;
                if let Some(s) = accounts.get_mut(account_name) {
                    s.status = AccountStatus::Refreshing;
                }
            }

            match refresh_with_retries(&self.http, &token_url, &client_id, &refresh_token, 5).await
            {
                Ok(token) => {
                    let mut accounts = self.accounts.write().await;
                    if let Some(state) = accounts.get_mut(account_name) {
                        state.api_key = Some(token.access_token.clone());
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
                        accounts.get(account_name).is_some_and(|s| {
                            s.config.device_auth_url.is_some()
                                || provider_default_device_auth_url(&s.config.provider).is_some()
                        })
                    };

                    let _ = self.event_tx.send(CredentialEvent::RefreshFailed {
                        account: account_name.to_owned(),
                        error: err_str.clone(),
                    });
                    tracing::warn!(account = %account_name, err = %err_str, "credential refresh failed");

                    if err_str.contains("invalid_grant") && has_device_auth {
                        tracing::info!(account = %account_name, "invalid_grant detected, initiating device code reauth");
                        if let Err(re) = self.initiate_reauth(account_name).await {
                            tracing::warn!(account = %account_name, err = %re, "auto-reauth failed");
                        }
                        tokio::time::sleep(Duration::from_secs(300)).await;
                    } else {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                }
            }
        }
    }

    async fn persist(&self, accounts: &HashMap<String, AccountState>) {
        let Some(ref dir) = self.persist_dir else {
            return;
        };
        let path = dir.join("credentials.json");
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!(err = %e, "failed to create state dir");
                return;
            }
        }
        let mut persisted = PersistedCredentials::default();
        for (name, state) in accounts {
            if let Some(ref token) = state.api_key {
                persisted.accounts.insert(
                    name.clone(),
                    PersistedAccount {
                        access_token: token.clone(),
                        refresh_token: state.refresh_token.clone(),
                        expires_at: state.expires_at,
                    },
                );
            }
            if !self.static_names.contains(name) {
                persisted.dynamic_accounts.push(state.config.clone());
            }
        }
        if let Err(e) = crate::credential::persist::save(&path, &persisted) {
            tracing::warn!(err = %e, "failed to persist credentials");
        }
    }
}

// ── Public types ────────────────────────────────────────────────────────

/// Status info for an account (returned by the API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AccountStatusInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    pub has_api_key: bool,
    #[cfg(feature = "legacy-oauth")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    #[cfg(feature = "legacy-oauth")]
    pub has_refresh_token: bool,
    #[cfg(feature = "legacy-oauth")]
    pub reauth: bool,
}

/// Pool utilization info for an account (returned by the pool API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolAccountInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    pub session_count: u32,
    pub has_api_key: bool,
    #[cfg(feature = "legacy-oauth")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
    #[cfg(feature = "legacy-oauth")]
    pub has_refresh_token: bool,
}

/// Response from initiating a reauth flow.
#[cfg(feature = "legacy-oauth")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReauthResponse {
    pub account: String,
    pub auth_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Default expiry for tokens without an explicit `expires_in` (11 months).
#[cfg(feature = "legacy-oauth")]
const DEFAULT_EXPIRES_IN: u64 = 11 * 30 * 24 * 3600;

#[cfg(feature = "legacy-oauth")]
fn epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
