// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential brokering: OAuth token refresh and distribution.
//!
//! Always initialized. Optionally pre-populated from `--credential-config <path>`.
//! Manages token freshness for registered accounts and pushes fresh credentials
//! to coop sessions as profiles. Accounts can also be added dynamically at runtime.

pub mod broker;
pub mod device_code;
pub mod distributor;
pub mod oauth;
pub mod persist;
pub mod pkce;
pub mod refresh;

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level credential configuration loaded from `--credential-config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialConfig {
    /// Named credential accounts.
    pub accounts: Vec<AccountConfig>,
}

/// Configuration for a single credential account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Display name for this account.
    pub name: String,
    /// Provider identifier: "claude", "openai", "gemini", etc.
    pub provider: String,
    /// Explicit env var name for the credential. Falls back to provider default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// OAuth token URL for refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// OAuth client ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth authorization URL for reauth flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// OAuth device authorization endpoint (RFC 8628). When set, the broker
    /// uses device code flow instead of auth code + PKCE.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth_url: Option<String>,
    /// Whether this account supports OAuth reauth/refresh. Defaults to `true`.
    /// Set to `false` for tokens pasted directly (API keys, long-lived tokens).
    #[serde(default = "default_true")]
    pub reauth: bool,
}

pub fn default_true() -> bool {
    true
}

/// Refresh margin in seconds (`COOP_MUX_REFRESH_MARGIN_SECS`, default 900).
pub fn refresh_margin_secs() -> u64 {
    std::env::var("COOP_MUX_REFRESH_MARGIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(900)
}

/// Resolve the state directory for mux data (credentials, etc.).
///
/// Checks `COOP_MUX_STATE_DIR`, then `$XDG_STATE_HOME/coop/mux`,
/// then `$HOME/.local/state/coop/mux`.
pub fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("COOP_MUX_STATE_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("coop/mux");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/state/coop/mux");
    }
    PathBuf::from(".coop/mux")
}

/// Events emitted by the credential broker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum CredentialEvent {
    /// Fresh credentials are available for distribution.
    Refreshed { account: String, credentials: HashMap<String, String> },
    /// A refresh attempt failed.
    #[serde(rename = "refresh:failed")]
    RefreshFailed { account: String, error: String },
    /// User interaction required (OAuth reauth flow).
    #[serde(rename = "reauth:required")]
    ReauthRequired {
        account: String,
        auth_url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_code: Option<String>,
    },
}

/// Status of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Healthy,
    Refreshing,
    Expired,
}

/// Resolve the default env var name for a provider.
pub fn provider_default_env_key(provider: &str) -> &str {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        _ => "API_KEY",
    }
}

/// Resolve the default OAuth token URL for device code exchange and refresh.
pub fn provider_default_token_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://platform.claude.com/v1/oauth/token"),
        _ => None,
    }
}

/// Resolve the OAuth token URL for authorization code (PKCE) exchange.
///
/// Claude's PKCE flow uses `claude.ai` (matching the authorize endpoint),
/// while device code and refresh use `platform.claude.com`.
pub fn provider_default_auth_code_token_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://claude.ai/oauth/token"),
        _ => None,
    }
}

/// Resolve the default OAuth device authorization URL for a provider (RFC 8628).
pub fn provider_default_device_auth_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://console.anthropic.com/v1/oauth/device/code"),
        _ => None,
    }
}

/// Resolve the default OAuth client ID for a provider.
pub fn provider_default_client_id(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        _ => None,
    }
}

/// Resolve the default OAuth authorization URL for a provider.
pub fn provider_default_auth_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://claude.ai/oauth/authorize"),
        _ => None,
    }
}

/// Resolve the default OAuth redirect URI for a provider.
///
/// Some providers (e.g. Claude) register a platform-hosted redirect URI that
/// displays the authorization code for the user to copy, rather than
/// redirecting back to a local server.
pub fn provider_default_redirect_uri(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://platform.claude.com/oauth/code/callback"),
        _ => None,
    }
}

/// Resolve the default OAuth scopes for a provider.
pub fn provider_default_scopes(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => {
            "user:profile user:inference user:sessions:claude_code user:mcp_servers"
        }
        _ => "",
    }
}
