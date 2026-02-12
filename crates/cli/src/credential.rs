// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Centralized OAuth credential management (Credential Broker — Epic 16).
//!
//! Holds refresh tokens for one or more accounts, proactively refreshes each
//! before expiry, and broadcasts [`CredentialEvent`]s so the distribution
//! layer (16c) can push fresh tokens to agent pods.
//!
//! Static credentials (API keys) are stored but not refreshed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};

/// Default margin before expiry to trigger a refresh (15 minutes).
const DEFAULT_REFRESH_MARGIN_SECS: u64 = 900;

/// Maximum retry backoff for failed refresh attempts.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(60);

/// Initial retry backoff for failed refresh attempts.
const INITIAL_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum retries before declaring an account revoked.
const MAX_RETRIES: u32 = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration for a single OAuth account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Human-readable name (e.g. "personal", "work").
    pub name: String,
    /// Provider identifier (e.g. "claude", "anthropic").
    #[serde(default = "default_provider")]
    pub provider: String,
    /// OAuth token endpoint URL.
    #[serde(default)]
    pub token_url: Option<String>,
    /// OAuth client ID.
    #[serde(default)]
    pub client_id: Option<String>,
    /// OAuth device authorization endpoint URL.
    #[serde(default)]
    pub device_auth_url: Option<String>,
    /// Whether this is a static credential (API key, no refresh).
    #[serde(default)]
    pub r#static: bool,
    /// Seconds before expiry to trigger refresh.
    #[serde(default = "default_refresh_margin")]
    pub refresh_margin_secs: u64,
}

fn default_provider() -> String {
    "claude".to_owned()
}

fn default_refresh_margin() -> u64 {
    DEFAULT_REFRESH_MARGIN_SECS
}

/// Top-level credential broker configuration (from `--agent-config`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialConfig {
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

/// Live state of a single account.
#[derive(Debug, Clone)]
pub struct AccountState {
    pub name: String,
    pub provider: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<Instant>,
    pub status: AccountStatus,
    pub config: AccountConfig,
}

/// Health status of an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    /// Token is valid and not near expiry.
    Healthy,
    /// A refresh is currently in progress.
    Refreshing,
    /// Token has expired.
    Expired,
    /// Refresh token was revoked (e.g. `invalid_grant`).
    Revoked,
    /// Static credential (API key), no refresh needed.
    Static,
}

/// Serializable snapshot for the status API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountStatusInfo {
    pub name: String,
    pub provider: String,
    pub status: AccountStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_secs: Option<u64>,
}

/// Events broadcast by the credential broker.
#[derive(Debug, Clone)]
pub enum CredentialEvent {
    /// An account was successfully refreshed.
    Refreshed {
        account: String,
        /// Credentials as env var key-value pairs, ready for profile injection.
        credentials: HashMap<String, String>,
    },
    /// An account refresh failed after retries.
    RefreshFailed { account: String, error: String },
    /// A re-authentication flow was initiated (device code flow).
    ReauthRequired { account: String, auth_url: String, user_code: String },
}

/// OAuth token response from the provider.
#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// Token lifetime in seconds.
    #[serde(default)]
    expires_in: Option<u64>,
}

/// OAuth error response from the provider.
#[derive(Debug, Deserialize)]
struct TokenErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Response from the device authorization endpoint (RFC 8628).
#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    expires_in: u64,
    #[serde(default = "default_poll_interval")]
    interval: u64,
}

fn default_poll_interval() -> u64 {
    5
}

/// Token response during device code polling.
#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// CredentialBroker
// ---------------------------------------------------------------------------

/// Centralized credential store and refresh daemon.
pub struct CredentialBroker {
    accounts: RwLock<HashMap<String, AccountState>>,
    event_tx: broadcast::Sender<CredentialEvent>,
    http_client: reqwest::Client,
}

impl CredentialBroker {
    /// Create a new broker with the given config. Accounts start as `Expired`
    /// until seeded with initial credentials.
    pub fn new(config: &CredentialConfig) -> (Arc<Self>, broadcast::Receiver<CredentialEvent>) {
        let (event_tx, event_rx) = broadcast::channel(64);
        let mut accounts = HashMap::new();

        for acct in &config.accounts {
            accounts.insert(
                acct.name.clone(),
                AccountState {
                    name: acct.name.clone(),
                    provider: acct.provider.clone(),
                    access_token: String::new(),
                    refresh_token: None,
                    expires_at: None,
                    status: if acct.r#static {
                        AccountStatus::Static
                    } else {
                        AccountStatus::Expired
                    },
                    config: acct.clone(),
                },
            );
        }

        let broker = Arc::new(Self {
            accounts: RwLock::new(accounts),
            event_tx,
            http_client: reqwest::Client::new(),
        });

        (broker, event_rx)
    }

    /// Seed initial credentials for an account (e.g. from K8s secret mount).
    pub async fn seed(
        &self,
        name: &str,
        access_token: String,
        refresh_token: Option<String>,
        expires_in_secs: Option<u64>,
    ) -> bool {
        let mut accounts = self.accounts.write().await;
        let Some(account) = accounts.get_mut(name) else {
            return false;
        };

        account.access_token = access_token;
        account.refresh_token = refresh_token;
        account.expires_at = expires_in_secs.map(|s| Instant::now() + Duration::from_secs(s));
        account.status =
            if account.config.r#static { AccountStatus::Static } else { AccountStatus::Healthy };

        info!(account = name, "credentials seeded");
        true
    }

    /// Return a snapshot of all account statuses.
    pub async fn status(&self) -> Vec<AccountStatusInfo> {
        let accounts = self.accounts.read().await;
        let now = Instant::now();

        accounts
            .values()
            .map(|a| {
                let expires_in = a.expires_at.map(|e| e.saturating_duration_since(now).as_secs());
                let error = if a.status == AccountStatus::Revoked {
                    Some("refresh token revoked".to_owned())
                } else {
                    None
                };
                AccountStatusInfo {
                    name: a.name.clone(),
                    provider: a.provider.clone(),
                    status: a.status.clone(),
                    error,
                    expires_in_secs: expires_in,
                }
            })
            .collect()
    }

    /// Build a profile credentials map for an account (env var key → value).
    pub async fn credentials_for(&self, name: &str) -> Option<HashMap<String, String>> {
        let accounts = self.accounts.read().await;
        let account = accounts.get(name)?;
        if account.access_token.is_empty() {
            return None;
        }

        let mut creds = HashMap::new();
        // Map provider to the appropriate env var key.
        let key = match account.provider.as_str() {
            "claude" => "ANTHROPIC_API_KEY",
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" | "codex" => "OPENAI_API_KEY",
            "google" | "gemini" => "GOOGLE_API_KEY",
            _ => "ANTHROPIC_API_KEY",
        };
        creds.insert(key.to_owned(), account.access_token.clone());
        Some(creds)
    }

    /// Build a full credentials map for all healthy accounts.
    pub async fn all_credentials(&self) -> Vec<(String, HashMap<String, String>)> {
        let accounts = self.accounts.read().await;
        let mut result = Vec::new();

        for account in accounts.values() {
            if account.access_token.is_empty() {
                continue;
            }
            if account.status == AccountStatus::Revoked {
                continue;
            }
            let key = match account.provider.as_str() {
                "claude" => "ANTHROPIC_API_KEY",
                "anthropic" => "ANTHROPIC_API_KEY",
                "openai" | "codex" => "OPENAI_API_KEY",
                "google" | "gemini" => "GOOGLE_API_KEY",
                _ => "ANTHROPIC_API_KEY",
            };
            let mut creds = HashMap::new();
            creds.insert(key.to_owned(), account.access_token.clone());
            result.push((account.name.clone(), creds));
        }

        result
    }

    /// Subscribe to credential events.
    pub fn subscribe(&self) -> broadcast::Receiver<CredentialEvent> {
        self.event_tx.subscribe()
    }

    /// Run the refresh loop for all accounts. Spawns one task per OAuth account.
    ///
    /// Call this once after seeding initial credentials. The loop runs until
    /// the `shutdown` token is cancelled.
    pub async fn run(self: &Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let accounts = self.accounts.read().await;
        let names: Vec<String> =
            accounts.values().filter(|a| !a.config.r#static).map(|a| a.name.clone()).collect();
        drop(accounts);

        let mut handles = Vec::new();
        for name in names {
            let broker = Arc::clone(self);
            let sd = shutdown.clone();
            let handle = tokio::spawn(async move {
                broker.refresh_loop(&name, sd).await;
            });
            handles.push(handle);
        }

        // Wait for all refresh loops to complete.
        for handle in handles {
            let _ = handle.await;
        }
    }

    /// Refresh loop for a single account.
    async fn refresh_loop(&self, name: &str, shutdown: tokio_util::sync::CancellationToken) {
        info!(account = name, "credential refresh loop started");

        loop {
            // Compute sleep duration.
            let sleep_duration = {
                let accounts = self.accounts.read().await;
                let Some(account) = accounts.get(name) else {
                    warn!(account = name, "account removed, stopping refresh loop");
                    return;
                };

                match account.status {
                    AccountStatus::Revoked => {
                        debug!(account = name, "account revoked, refresh loop paused");
                        // Sleep long and re-check (re-auth may have restored it).
                        Duration::from_secs(30)
                    }
                    AccountStatus::Static => {
                        debug!(account = name, "static account, stopping refresh loop");
                        return;
                    }
                    _ => {
                        let margin = Duration::from_secs(account.config.refresh_margin_secs);
                        match account.expires_at {
                            Some(expires_at) => {
                                let now = Instant::now();
                                let target = expires_at.checked_sub(margin).unwrap_or(now);
                                if target > now {
                                    target - now
                                } else {
                                    // Already past the refresh window — refresh now.
                                    Duration::ZERO
                                }
                            }
                            None => {
                                // No expiry known — refresh every margin interval.
                                margin
                            }
                        }
                    }
                }
            };

            if !sleep_duration.is_zero() {
                debug!(
                    account = name,
                    sleep_secs = sleep_duration.as_secs(),
                    "sleeping until next refresh"
                );
                tokio::select! {
                    _ = tokio::time::sleep(sleep_duration) => {}
                    _ = shutdown.cancelled() => {
                        info!(account = name, "shutdown, stopping refresh loop");
                        return;
                    }
                }
            }

            // Check shutdown before refreshing.
            if shutdown.is_cancelled() {
                return;
            }

            // Attempt refresh with retries.
            self.refresh_with_retries(name).await;
        }
    }

    /// Attempt to refresh an account's token, with exponential backoff retries.
    async fn refresh_with_retries(&self, name: &str) {
        let mut backoff = INITIAL_RETRY_BACKOFF;

        for attempt in 1..=MAX_RETRIES {
            // Mark as refreshing.
            {
                let mut accounts = self.accounts.write().await;
                if let Some(a) = accounts.get_mut(name) {
                    a.status = AccountStatus::Refreshing;
                }
            }

            match self.do_refresh(name).await {
                Ok(()) => return,
                Err(RefreshError::Revoked(msg)) => {
                    error!(
                        account = name,
                        error = %msg,
                        "refresh token revoked — marking account as revoked"
                    );
                    {
                        let mut accounts = self.accounts.write().await;
                        if let Some(a) = accounts.get_mut(name) {
                            a.status = AccountStatus::Revoked;
                        }
                    }
                    let _ = self.event_tx.send(CredentialEvent::RefreshFailed {
                        account: name.to_owned(),
                        error: msg,
                    });
                    return;
                }
                Err(RefreshError::Transient(msg)) => {
                    warn!(
                        account = name,
                        attempt,
                        max = MAX_RETRIES,
                        error = %msg,
                        "refresh failed, retrying"
                    );
                    if attempt < MAX_RETRIES {
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_RETRY_BACKOFF);
                    }
                }
            }
        }

        // All retries exhausted.
        error!(account = name, "refresh failed after {MAX_RETRIES} attempts");
        {
            let mut accounts = self.accounts.write().await;
            if let Some(a) = accounts.get_mut(name) {
                a.status = AccountStatus::Expired;
            }
        }
        let _ = self.event_tx.send(CredentialEvent::RefreshFailed {
            account: name.to_owned(),
            error: format!("refresh failed after {MAX_RETRIES} retries"),
        });
    }

    /// Execute a single refresh attempt for an account.
    async fn do_refresh(&self, name: &str) -> Result<(), RefreshError> {
        let (token_url, client_id, refresh_token) = {
            let accounts = self.accounts.read().await;
            let account = accounts
                .get(name)
                .ok_or_else(|| RefreshError::Transient("account not found".into()))?;

            let token_url = account
                .config
                .token_url
                .clone()
                .ok_or_else(|| RefreshError::Transient("no token_url configured".into()))?;
            let client_id = account
                .config
                .client_id
                .clone()
                .ok_or_else(|| RefreshError::Transient("no client_id configured".into()))?;
            let refresh_token = account
                .refresh_token
                .clone()
                .ok_or_else(|| RefreshError::Transient("no refresh token available".into()))?;

            (token_url, client_id, refresh_token)
        };

        // Use url-encoded form body for the token request.
        let form_body = format!(
            "grant_type=refresh_token&client_id={}&refresh_token={}",
            urlencoded(&client_id),
            urlencoded(&refresh_token),
        );

        let resp = self
            .http_client
            .post(&token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await
            .map_err(|e| RefreshError::Transient(format!("HTTP error: {e}")))?;

        let status = resp.status();
        let body =
            resp.text().await.map_err(|e| RefreshError::Transient(format!("read body: {e}")))?;

        if !status.is_success() {
            // Try to parse as error response.
            if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
                if err.error == "invalid_grant" {
                    return Err(RefreshError::Revoked(err.error_description.unwrap_or(err.error)));
                }
                return Err(RefreshError::Transient(format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                )));
            }
            return Err(RefreshError::Transient(format!("HTTP {status}: {body}")));
        }

        let token: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| RefreshError::Transient(format!("parse response: {e}")))?;

        // Update account state.
        let credentials = {
            let mut accounts = self.accounts.write().await;
            let account = accounts
                .get_mut(name)
                .ok_or_else(|| RefreshError::Transient("account removed during refresh".into()))?;

            account.access_token = token.access_token.clone();
            if let Some(new_refresh) = token.refresh_token {
                account.refresh_token = Some(new_refresh);
            }
            account.expires_at = token.expires_in.map(|s| Instant::now() + Duration::from_secs(s));
            account.status = AccountStatus::Healthy;

            // Build credentials map for the event.
            let key = match account.provider.as_str() {
                "claude" | "anthropic" => "ANTHROPIC_API_KEY",
                "openai" | "codex" => "OPENAI_API_KEY",
                "google" | "gemini" => "GOOGLE_API_KEY",
                _ => "ANTHROPIC_API_KEY",
            };
            let mut creds = HashMap::new();
            creds.insert(key.to_owned(), token.access_token);
            creds
        };

        info!(account = name, "credentials refreshed successfully");

        let _ = self
            .event_tx
            .send(CredentialEvent::Refreshed { account: name.to_owned(), credentials });

        Ok(())
    }

    /// Initiate a device code re-authentication flow for an account (RFC 8628).
    /// Returns (auth_url, user_code) on success.
    pub async fn initiate_reauth(
        self: &Arc<Self>,
        account_name: &str,
    ) -> Result<(String, String), String> {
        let (device_auth_url, client_id) = {
            let accounts = self.accounts.read().await;
            let account = accounts
                .get(account_name)
                .ok_or_else(|| format!("unknown account: {account_name}"))?;
            let device_url = account
                .config
                .device_auth_url
                .clone()
                .ok_or_else(|| "no device_auth_url configured".to_string())?;
            let client_id = account
                .config
                .client_id
                .clone()
                .ok_or_else(|| "no client_id configured".to_string())?;
            (device_url, client_id)
        };

        // Request device code from authorization server.
        let resp = self
            .http_client
            .post(&device_auth_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!("client_id={}", urlencoded(&client_id)))
            .send()
            .await
            .map_err(|e| format!("device auth request failed: {e}"))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("device auth failed: {body}"));
        }

        let device: DeviceCodeResponse =
            resp.json().await.map_err(|e| format!("parse device response: {e}"))?;

        let auth_url = device
            .verification_uri_complete
            .clone()
            .unwrap_or_else(|| device.verification_uri.clone());
        let user_code = device.user_code.clone();

        // Broadcast reauth event.
        let _ = self.event_tx.send(CredentialEvent::ReauthRequired {
            account: account_name.to_owned(),
            auth_url: auth_url.clone(),
            user_code: user_code.clone(),
        });

        // Spawn background polling task.
        let broker = Arc::clone(self);
        let account = account_name.to_owned();
        tokio::spawn(async move {
            broker
                .poll_device_code(&account, &device.device_code, device.interval, device.expires_in)
                .await;
        });

        Ok((auth_url, user_code))
    }

    /// Poll the token endpoint for device code completion.
    async fn poll_device_code(
        &self,
        account_name: &str,
        device_code: &str,
        interval: u64,
        expires_in: u64,
    ) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(expires_in);
        let poll_interval = Duration::from_secs(interval);

        let (token_url, client_id) = {
            let accounts = self.accounts.read().await;
            let Some(account) = accounts.get(account_name) else {
                return;
            };
            let Some(ref url) = account.config.token_url else {
                return;
            };
            let Some(ref cid) = account.config.client_id else {
                return;
            };
            (url.clone(), cid.clone())
        };

        loop {
            tokio::time::sleep(poll_interval).await;
            if tokio::time::Instant::now() > deadline {
                warn!(account = account_name, "device code flow expired");
                return;
            }

            let body = format!(
                "grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id={}&device_code={}",
                urlencoded(&client_id),
                urlencoded(device_code),
            );

            let resp = match self
                .http_client
                .post(&token_url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(body)
                .send()
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    debug!(account = account_name, "device poll error: {e}");
                    continue;
                }
            };

            let text = resp.text().await.unwrap_or_default();
            let token: DeviceTokenResponse = match serde_json::from_str(&text) {
                Ok(t) => t,
                Err(_) => continue,
            };

            // Check for pending/slow_down errors.
            if let Some(ref err) = token.error {
                match err.as_str() {
                    "authorization_pending" | "slow_down" => continue,
                    "expired_token" => {
                        warn!(account = account_name, "device code expired");
                        return;
                    }
                    "access_denied" => {
                        warn!(account = account_name, "device code denied by user");
                        return;
                    }
                    other => {
                        warn!(account = account_name, error = other, "device code poll error");
                        return;
                    }
                }
            }

            // Success — we have tokens.
            if let Some(access_token) = token.access_token {
                let credentials = {
                    let mut accounts = self.accounts.write().await;
                    let Some(account) = accounts.get_mut(account_name) else {
                        return;
                    };
                    account.access_token = access_token.clone();
                    if let Some(new_refresh) = token.refresh_token {
                        account.refresh_token = Some(new_refresh);
                    }
                    account.expires_at =
                        token.expires_in.map(|s| Instant::now() + Duration::from_secs(s));
                    account.status = AccountStatus::Healthy;

                    let key = match account.provider.as_str() {
                        "claude" | "anthropic" => "ANTHROPIC_API_KEY",
                        "openai" | "codex" => "OPENAI_API_KEY",
                        "google" | "gemini" => "GOOGLE_API_KEY",
                        _ => "ANTHROPIC_API_KEY",
                    };
                    let mut creds = HashMap::new();
                    creds.insert(key.to_owned(), access_token);
                    creds
                };

                info!(account = account_name, "device code flow completed successfully");
                let _ = self.event_tx.send(CredentialEvent::Refreshed {
                    account: account_name.to_owned(),
                    credentials,
                });
                return;
            }
        }
    }
}

/// Minimal URL-encode for form values (percent-encode non-unreserved chars).
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

/// Internal error type for refresh attempts.
enum RefreshError {
    /// Permanent failure — refresh token revoked.
    Revoked(String),
    /// Temporary failure — retry with backoff.
    Transient(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Revoked(msg) => write!(f, "revoked: {msg}"),
            Self::Transient(msg) => write!(f, "transient: {msg}"),
        }
    }
}

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
