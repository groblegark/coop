// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential brokering: OAuth token refresh and distribution.
//!
//! Activated by `--credential-config <path>`. Manages token freshness for
//! registered accounts and pushes fresh credentials to coop sessions as profiles.

pub mod broker;
pub mod device_code;
pub mod distributor;
pub mod oauth;
pub mod persist;
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
    /// Device authorization URL for reauth flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_auth_url: Option<String>,
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
#[derive(Debug, Clone)]
pub enum CredentialEvent {
    /// Fresh credentials are available for distribution.
    Refreshed { account: String, credentials: HashMap<String, String> },
    /// A refresh attempt failed.
    RefreshFailed { account: String, error: String },
    /// User interaction required (device code flow).
    ReauthRequired { account: String, auth_url: String, user_code: String },
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
