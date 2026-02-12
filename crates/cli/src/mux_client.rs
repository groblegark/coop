// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Mux self-registration client.
//!
//! When `COOP_MUX_URL` is set, coop automatically registers itself with the
//! mux server on startup, re-registers periodically as a heartbeat, and
//! deregisters on shutdown.

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Configuration for the mux registration client.
pub struct MuxRegistration {
    /// Base URL of the mux server (e.g. `http://localhost:9800`).
    pub mux_url: String,
    /// Auth token for the mux API.
    pub mux_token: Option<String>,
    /// Session ID for this coop instance.
    pub session_id: String,
    /// URL where mux can reach this coop instance.
    pub coop_url: String,
    /// Auth token for this coop instance (passed to mux for upstream calls).
    pub coop_token: Option<String>,
    /// Credential account names this session wants distributed as profiles.
    pub profiles_needed: Vec<String>,
}

/// Spawn the mux registration client if `COOP_MUX_URL` is set.
///
/// Reads configuration from environment variables and spawns a background task.
/// Returns immediately if `COOP_MUX_URL` is not set.
pub async fn spawn_if_configured(
    session_id: &str,
    default_port: Option<u16>,
    auth_token: Option<&str>,
    shutdown: CancellationToken,
) {
    let Ok(mux_url) = std::env::var("COOP_MUX_URL") else {
        return;
    };
    let coop_url = std::env::var("COOP_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{}", default_port.unwrap_or(0)));
    let profiles_needed: Vec<String> = std::env::var("COOP_MUX_PROFILES")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect();
    let reg = MuxRegistration {
        mux_url,
        mux_token: std::env::var("COOP_MUX_TOKEN").ok(),
        session_id: session_id.to_owned(),
        coop_url,
        coop_token: auth_token.map(str::to_owned),
        profiles_needed,
    };
    tokio::spawn(async move {
        run(reg, shutdown).await;
    });
}

/// Run the mux registration client until shutdown.
///
/// - Registers on startup (retries up to 5 times with backoff).
/// - Re-registers every 60s as a heartbeat.
/// - Deregisters on shutdown.
pub async fn run(config: MuxRegistration, shutdown: CancellationToken) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let base = config.mux_url.trim_end_matches('/').to_owned();

    // Register with retries.
    let mut registered = false;
    for attempt in 0..5u32 {
        if shutdown.is_cancelled() {
            return;
        }
        match register(&client, &base, &config).await {
            Ok(()) => {
                info!(mux = %base, session = %config.session_id, "registered with mux");
                registered = true;
                break;
            }
            Err(e) => {
                let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt));
                warn!(
                    mux = %base,
                    attempt = attempt + 1,
                    err = %e,
                    "mux registration failed, retrying in {:?}",
                    delay,
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.cancelled() => return,
                }
            }
        }
    }

    if !registered {
        warn!(mux = %base, "mux registration failed after 5 attempts, giving up");
        return;
    }

    // Heartbeat loop: re-register every 60s.
    let heartbeat = std::time::Duration::from_secs(60);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(heartbeat) => {
                if let Err(e) = register(&client, &base, &config).await {
                    warn!(mux = %base, err = %e, "mux heartbeat re-registration failed");
                }
            }
            _ = shutdown.cancelled() => break,
        }
    }

    // Deregister on shutdown.
    if let Err(e) = deregister(&client, &base, &config).await {
        warn!(mux = %base, err = %e, "mux deregistration failed");
    } else {
        info!(mux = %base, session = %config.session_id, "deregistered from mux");
    }
}

/// POST /api/v1/sessions to register this coop instance.
async fn register(
    client: &reqwest::Client,
    base: &str,
    config: &MuxRegistration,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/v1/sessions");
    let body = serde_json::json!({
        "url": config.coop_url,
        "auth_token": config.coop_token,
        "id": config.session_id,
        "profiles_needed": config.profiles_needed,
    });
    let mut req = client.post(&url).json(&body);
    if let Some(ref token) = config.mux_token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    resp.error_for_status()?;
    Ok(())
}

/// DELETE /api/v1/sessions/{id} to deregister this coop instance.
async fn deregister(
    client: &reqwest::Client,
    base: &str,
    config: &MuxRegistration,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/v1/sessions/{}", config.session_id);
    let mut req = client.delete(&url);
    if let Some(ref token) = config.mux_token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await?;
    resp.error_for_status()?;
    Ok(())
}
