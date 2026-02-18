// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker: manages account states, runs refresh loops, emits events.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, RwLock};

use crate::credential::persist::{PersistedAccount, PersistedCredentials};
use crate::credential::refresh::refresh_with_retries;
use crate::credential::{
    provider_default_client_id, provider_default_device_auth_url,
    provider_default_device_token_url, provider_default_env_key, provider_default_pkce_auth_url,
    provider_default_pkce_token_url, provider_default_scopes, AccountConfig, AccountStatus,
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
    state: String,
    /// Full authorization URL (for reuse when same account is requested again).
    auth_url: String,
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
    /// Directory for credential persistence. `None` disables persistence (used in tests).
    persist_dir: Option<PathBuf>,
    /// Per-account session counts for pool load balancing.
    /// Key: account name, Value: number of sessions assigned to this account.
    session_counts: RwLock<HashMap<String, AtomicU32>>,
}

impl CredentialBroker {
    /// Create a new broker from config.
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
                    access_token: None,
                    refresh_token: None,
                    expires_at: 0,
                },
            );
        }
        // Initialize session counts for each account.
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
                if persisted.expires_at > epoch_secs() {
                    state.status = AccountStatus::Healthy;
                } else {
                    state.status = AccountStatus::Expired;
                }
            }
        }
    }

    /// Set tokens for an account (from API call or CLI).
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

        state.access_token = Some(access_token.clone());
        state.refresh_token = refresh_token;
        // Non-refreshable credentials (reauth: false) have no expiry.
        state.expires_at = if state.config.reauth {
            epoch_secs() + expires_in.unwrap_or(DEFAULT_EXPIRES_IN)
        } else {
            0
        };
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
            // Non-refreshable credentials (reauth: false) have no expiry.
            let expires_at = if config.reauth {
                epoch_secs() + expires_in.unwrap_or(DEFAULT_EXPIRES_IN)
            } else {
                0
            };
            let status = if has_token { AccountStatus::Healthy } else { AccountStatus::Expired };

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

        // Initialize pool session count for the new account.
        self.session_counts
            .write()
            .await
            .entry(name.clone())
            .or_insert_with(|| AtomicU32::new(0));

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
                // Only show expires_in for refreshable credentials that aren't expired.
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
                    expires_in_secs: expires_in,
                    has_refresh_token: state.refresh_token.is_some(),
                    reauth: state.config.reauth,
                }
            })
            .collect()
    }

    // ── Pool load balancing ─────────────────────────────────────────────

    /// Pick the least-loaded healthy account for a new session.
    ///
    /// If `preferred` is given and the account is healthy, return it.
    /// Otherwise, select the healthy account with the fewest assigned sessions.
    /// Returns `None` only if no healthy accounts exist.
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
            // Saturating subtract to avoid underflow.
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
        let now = epoch_secs();

        accounts
            .iter()
            .map(|(name, state)| {
                let session_count = counts
                    .get(name)
                    .map(|c| c.load(Ordering::Relaxed))
                    .unwrap_or(0);
                let expires_in = if state.expires_at > now {
                    Some(state.expires_at - now)
                } else {
                    None
                };
                PoolAccountInfo {
                    name: name.clone(),
                    provider: state.config.provider.clone(),
                    status: state.status,
                    session_count,
                    expires_in_secs: expires_in,
                    has_refresh_token: state.refresh_token.is_some(),
                }
            })
            .collect()
    }

    /// List accounts that have gone unhealthy, with the sessions that need reassignment.
    pub async fn unhealthy_accounts(&self) -> Vec<String> {
        let accounts = self.accounts.read().await;
        accounts
            .iter()
            .filter(|(_, state)| state.status == AccountStatus::Expired)
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

    // ── OAuth reauth ──────────────────────────────────────────────────────

    /// Initiate OAuth reauth flow for an account.
    ///
    /// Tries device code flow (RFC 8628) first, falls back to authorization
    /// code + PKCE if the device auth endpoint is unavailable or fails.
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

        // Reuse existing pending auth for the same account if one exists.
        // This prevents creating multiple PKCE sessions — the user would open
        // auth_url_A (with challenge_A) but the exchange would use verifier_B,
        // causing "Code challenge failed".
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

        // Store pending auth for code exchange.
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

    /// Refresh loop for a single account.
    async fn refresh_loop(self: &Arc<Self>, account_name: &str) {
        let margin = crate::credential::refresh_margin_secs();
        loop {
            let (token_url, client_id, refresh_token, expires_at) = {
                let accounts = self.accounts.read().await;
                let Some(state) = accounts.get(account_name) else {
                    return;
                };
                // Non-renewable accounts (e.g. long-lived tokens) don't refresh.
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
                        // Otherwise mark expired and auto-initiate reauth.
                        let is_expired = state.expires_at > 0 && state.expires_at <= epoch_secs();
                        // New accounts (expires_at == 0, no access token) are also expired.
                        let is_new_account = state.expires_at == 0 && state.access_token.is_none();
                        let needs_reauth = is_expired || is_new_account;
                        drop(accounts);
                        if needs_reauth {
                            let mut accounts = self.accounts.write().await;
                            if let Some(s) = accounts.get_mut(account_name) {
                                s.status = AccountStatus::Expired;
                            }
                            drop(accounts);
                            // Auto-initiate reauth for expired accounts with no
                            // refresh token (e.g. newly added accounts). Only try
                            // once — if it fails, fall through to the 60s sleep.
                            // pending_auths reuse protection prevents duplicate sessions.
                            tracing::info!(account = %account_name, "no refresh token, auto-initiating reauth");
                            if let Err(e) = self.initiate_reauth(account_name).await {
                                tracing::warn!(account = %account_name, err = %e, "auto-reauth initiation failed");
                            }
                            // Give user time to complete authorization.
                            tokio::time::sleep(Duration::from_secs(300)).await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
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
    pub reauth: bool,
}

/// Pool utilization info for an account (returned by the pool API).
#[derive(Debug, Clone, serde::Serialize)]
pub struct PoolAccountInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    pub session_count: u32,
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

/// Default expiry for tokens without an explicit `expires_in` (11 months).
const DEFAULT_EXPIRES_IN: u64 = 11 * 30 * 24 * 3600;

fn epoch_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
