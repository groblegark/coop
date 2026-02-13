// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Mux self-registration client.
//!
//! Coop automatically registers itself with the mux server on startup,
//! re-registers periodically as a heartbeat, and deregisters on shutdown.
//! By default it connects to `http://127.0.0.1:9800` (coopmux's default port).
//! Override with `COOP_MUX_URL` or set `COOP_MUX_URL=""` to disable.

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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
}

/// Spawn the mux registration client.
///
/// Reads configuration from environment variables and spawns a background task.
/// Falls back to the default mux URL (`http://127.0.0.1:9800`) when
/// `COOP_MUX_URL` is not set. Set `COOP_MUX_URL=""` to disable registration.
pub async fn spawn_if_configured(
    session_id: &str,
    default_port: Option<u16>,
    auth_token: Option<&str>,
    shutdown: CancellationToken,
) {
    let mux_url = match std::env::var("COOP_MUX_URL") {
        Ok(v) if v.is_empty() => return, // explicit disable
        Ok(v) => v,
        Err(_) => "http://127.0.0.1:9800".to_owned(), // default coopmux port
    };
    let coop_url = std::env::var("COOP_URL")
        .unwrap_or_else(|_| format!("http://127.0.0.1:{}", default_port.unwrap_or(0)));
    let reg = MuxRegistration {
        mux_url,
        mux_token: std::env::var("COOP_MUX_TOKEN").ok(),
        session_id: session_id.to_owned(),
        coop_url,
        coop_token: auth_token.map(str::to_owned),
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

    // Register with retries (quiet — mux may not be running yet).
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
                debug!(
                    mux = %base,
                    attempt = attempt + 1,
                    err = %e,
                    "mux registration attempt failed, retrying in {:?}",
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
        info!(mux = %base, "mux not available, will retry periodically");
    }

    // Heartbeat loop: re-register periodically (runs forever, regardless of
    // initial registration success — allows late-started mux to pick up sessions).
    let heartbeat = std::time::Duration::from_secs(60);
    loop {
        tokio::select! {
            _ = tokio::time::sleep(heartbeat) => {
                match register(&client, &base, &config).await {
                    Ok(()) => {
                        if !registered {
                            info!(mux = %base, session = %config.session_id, "registered with mux");
                            registered = true;
                        }
                    }
                    Err(e) => {
                        debug!(mux = %base, err = %e, "mux re-registration failed");
                    }
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

/// Detect optional Kubernetes metadata from environment variables.
///
/// Returns `Value::Null` when not running in Kubernetes (i.e. `KUBERNETES_SERVICE_HOST`
/// is not set). When running in K8s, returns a JSON object with available pod metadata.
pub fn detect_metadata() -> Value {
    detect_metadata_with(|name| std::env::var(name).ok())
}

/// Inner implementation that accepts a lookup function for testability.
fn detect_metadata_with(get_env: impl Fn(&str) -> Option<String>) -> Value {
    if get_env("KUBERNETES_SERVICE_HOST").is_none() {
        return Value::Null;
    }

    let env_fields: &[(&str, &str)] = &[
        ("pod", "POD_NAME"),
        ("pod", "HOSTNAME"),
        ("namespace", "POD_NAMESPACE"),
        ("node", "NODE_NAME"),
        ("ip", "POD_IP"),
        ("service_account", "POD_SERVICE_ACCOUNT"),
    ];

    let mut k8s = serde_json::Map::new();
    for &(field, var) in env_fields {
        // Skip if we already have this field (POD_NAME takes priority over HOSTNAME for "pod").
        if k8s.contains_key(field) {
            continue;
        }
        if let Some(val) = get_env(var) {
            k8s.insert(field.to_owned(), Value::String(val));
        }
    }

    serde_json::json!({ "k8s": k8s })
}

/// POST /api/v1/sessions to register this coop instance.
async fn register(
    client: &reqwest::Client,
    base: &str,
    config: &MuxRegistration,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/v1/sessions");
    let metadata = detect_metadata();
    let body = serde_json::json!({
        "url": config.coop_url,
        "auth_token": config.coop_token,
        "id": config.session_id,
        "metadata": metadata,
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

#[cfg(test)]
#[path = "mux_client_tests.rs"]
mod tests;
