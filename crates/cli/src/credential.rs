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
    RefreshFailed {
        account: String,
        error: String,
    },
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
        account.status = if account.config.r#static {
            AccountStatus::Static
        } else {
            AccountStatus::Healthy
        };

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
                let expires_in = a
                    .expires_at
                    .map(|e| e.saturating_duration_since(now).as_secs());
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
    pub async fn run(
        self: &Arc<Self>,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let accounts = self.accounts.read().await;
        let names: Vec<String> = accounts
            .values()
            .filter(|a| !a.config.r#static)
            .map(|a| a.name.clone())
            .collect();
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
    async fn refresh_loop(
        &self,
        name: &str,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
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
                        let margin =
                            Duration::from_secs(account.config.refresh_margin_secs);
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
        let body = resp
            .text()
            .await
            .map_err(|e| RefreshError::Transient(format!("read body: {e}")))?;

        if !status.is_success() {
            // Try to parse as error response.
            if let Ok(err) = serde_json::from_str::<TokenErrorResponse>(&body) {
                if err.error == "invalid_grant" {
                    return Err(RefreshError::Revoked(
                        err.error_description.unwrap_or(err.error),
                    ));
                }
                return Err(RefreshError::Transient(format!(
                    "{}: {}",
                    err.error,
                    err.error_description.unwrap_or_default()
                )));
            }
            return Err(RefreshError::Transient(format!(
                "HTTP {status}: {body}"
            )));
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
            account.expires_at =
                token.expires_in.map(|s| Instant::now() + Duration::from_secs(s));
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

        let _ = self.event_tx.send(CredentialEvent::Refreshed {
            account: name.to_owned(),
            credentials,
        });

        Ok(())
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
