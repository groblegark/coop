// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential persistence: load/save to JSON file with atomic writes.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted credential state for all accounts.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedCredentials {
    pub accounts: HashMap<String, PersistedAccount>,
}

/// Persisted state for a single account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedAccount {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Expiry as epoch seconds.
    #[serde(default)]
    pub expires_at: u64,
}

/// Load persisted credentials from a JSON file.
pub fn load(path: &Path) -> anyhow::Result<PersistedCredentials> {
    let contents = std::fs::read_to_string(path)?;
    let creds: PersistedCredentials = serde_json::from_str(&contents)?;
    Ok(creds)
}

/// Save persisted credentials to a JSON file atomically (write tmp + rename).
pub fn save(path: &Path, creds: &PersistedCredentials) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(creds)?;
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
