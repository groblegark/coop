// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential brokering: static API key store with distribution.
//!
//! Always initialized. Optionally pre-populated from `--credential-config <path>`.
//! Stores API keys for configured accounts and pushes them to coop sessions
//! as profiles. Accounts can also be added dynamically at runtime.

pub mod broker;
pub mod distributor;

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
/// Legacy OAuth fields (`token_url`, `client_id`, `auth_url`, `device_auth_url`,
/// `reauth`) are kept for deserialization compatibility with existing config files
/// but are ignored at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    /// Display name for this account.
    pub name: String,
    /// Provider identifier: "claude", "openai", "gemini", etc.
    pub provider: String,
    /// Explicit env var name for the credential. Falls back to provider default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_key: Option<String>,
    /// Legacy: OAuth token URL (ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    /// Legacy: OAuth client ID (ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Legacy: OAuth authorization URL (ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// Legacy: OAuth device authorization endpoint (ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth_url: Option<String>,
    /// Legacy: whether this account supports OAuth reauth (ignored).
    #[serde(default = "default_true")]
    pub reauth: bool,
}

pub fn default_true() -> bool {
    true
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
}

/// Status of an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Healthy,
    /// Account exists but has no API key configured.
    Missing,
    /// Legacy: kept for deserialization compatibility.
    Refreshing,
    /// Legacy: kept for deserialization compatibility.
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
