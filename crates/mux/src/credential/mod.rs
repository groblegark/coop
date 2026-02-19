// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential brokering: static API key store with distribution.
//!
//! Always initialized. Optionally pre-populated from `--credential-config <path>`.
//! Stores API keys for configured accounts and pushes them to coop sessions
//! as profiles. Accounts can also be added dynamically at runtime.
//!
//! When the `legacy-oauth` feature is enabled, this module also provides OAuth
//! token refresh loops, PKCE, device code flows, and credential persistence.

pub mod broker;
#[cfg(feature = "legacy-oauth")]
pub mod device_code;
pub mod distributor;
#[cfg(feature = "legacy-oauth")]
pub mod oauth;
#[cfg(feature = "legacy-oauth")]
pub mod persist;
#[cfg(feature = "legacy-oauth")]
pub mod pkce;
#[cfg(feature = "legacy-oauth")]
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
///
/// OAuth fields (`token_url`, `client_id`, `auth_url`, `device_auth_url`,
/// `reauth`) are always deserialized for config-file compatibility. They are
/// used at runtime only when the `legacy-oauth` feature is enabled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Display name for this account.
    pub name: String,
    /// Provider identifier: "claude", "openai", "gemini", etc.
    pub provider: String,
    /// Explicit env var name for the credential. Falls back to provider default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// OAuth token URL (active with `legacy-oauth`, ignored otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// OAuth client ID (active with `legacy-oauth`, ignored otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth authorization URL (active with `legacy-oauth`, ignored otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// OAuth device authorization endpoint (active with `legacy-oauth`, ignored otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth_url: Option<String>,
    /// Whether this account supports OAuth reauth (active with `legacy-oauth`, ignored otherwise).
    #[serde(default = "default_true")]
    pub reauth: bool,
}

pub fn default_true() -> bool {
    true
}

/// Refresh margin in seconds (`COOP_MUX_REFRESH_MARGIN_SECS`, default 900).
#[cfg(feature = "legacy-oauth")]
pub fn refresh_margin_secs() -> u64 {
    std::env::var("COOP_MUX_REFRESH_MARGIN_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(900)
}

/// Resolve the state directory for mux data.
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
    /// A credential operation failed.
    #[serde(rename = "refresh:failed")]
    RefreshFailed { account: String, error: String },
    /// User interaction required (OAuth reauth flow).
    /// Only emitted when `legacy-oauth` feature is enabled.
    #[cfg(feature = "legacy-oauth")]
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
    /// Account exists but has no API key configured.
    Missing,
    /// Account is refreshing credentials (legacy-oauth only).
    Refreshing,
    /// Account credentials have expired (legacy-oauth only).
    Expired,
}

/// Resolve the default env var name for a provider.
pub fn provider_default_env_key(provider: &str) -> &str {
    match provider.to_lowercase().as_str() {
        // Claude provider uses OAuth tokens, not API keys. Using
        // CLAUDE_CODE_OAUTH_TOKEN avoids the "Detected a custom API key"
        // prompt in Claude Code that ANTHROPIC_API_KEY triggers.
        "claude" | "anthropic" => "CLAUDE_CODE_OAUTH_TOKEN",
        "openai" => "OPENAI_API_KEY",
        "gemini" | "google" => "GEMINI_API_KEY",
        _ => "API_KEY",
    }
}

// ── OAuth provider defaults (legacy-oauth feature only) ─────────────────

/// Device code flow: authorization endpoint (RFC 8628).
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_device_auth_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://console.anthropic.com/v1/oauth/device/code"),
        _ => None,
    }
}

/// Device code flow: token endpoint (also used for refresh).
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_device_token_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://platform.claude.com/v1/oauth/token"),
        _ => None,
    }
}

/// PKCE flow: authorization endpoint.
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_pkce_auth_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://claude.ai/oauth/authorize"),
        _ => None,
    }
}

/// PKCE flow: token endpoint.
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_pkce_token_url(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://platform.claude.com/v1/oauth/token"),
        _ => None,
    }
}

/// Resolve the default OAuth client ID for a provider.
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_client_id(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("9d1c250a-e61b-44d9-88ed-5944d1962f5e"),
        _ => None,
    }
}

/// Resolve the default OAuth redirect URI for a provider.
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_redirect_uri(provider: &str) -> Option<&'static str> {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => Some("https://platform.claude.com/oauth/code/callback"),
        _ => None,
    }
}

/// Resolve the default OAuth scopes for a provider.
#[cfg(feature = "legacy-oauth")]
pub fn provider_default_scopes(provider: &str) -> &'static str {
    match provider.to_lowercase().as_str() {
        "claude" | "anthropic" => {
            "user:profile user:inference user:sessions:claude_code user:mcp_servers"
        }
        _ => "",
    }
}
