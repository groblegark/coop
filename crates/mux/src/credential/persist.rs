// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential persistence: load/save to JSON file with atomic writes.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::credential::AccountConfig;

/// Persisted credential state for all accounts.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PersistedCredentials {
    pub accounts: HashMap<String, PersistedAccount>,
    /// Configs for accounts added dynamically at runtime (not in static config file).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dynamic_accounts: Vec<AccountConfig>,
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
///
/// Uses a unique temp filename (PID + counter) to avoid corruption when
/// concurrent saves race on the same `.tmp` file — a shorter write can leave
/// trailing bytes from a longer previous write.
pub fn save(path: &Path, creds: &PersistedCredentials) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    let json = serde_json::to_string_pretty(creds)?;
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        "{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        seq,
    );
    let tmp_path = path.with_file_name(tmp_name);
    std::fs::write(&tmp_path, json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
