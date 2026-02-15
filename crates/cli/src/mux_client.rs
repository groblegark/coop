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
    /// Agent type (e.g. "claude", "gemini").
    pub agent: String,
    /// Metadata labels from `--label` CLI flags.
    pub labels: Vec<String>,
}

/// Spawn the mux registration client.
///
/// `mux_url` is the resolved URL from `Config::mux_url()`. Pass `None` to
/// disable registration (e.g. in tests).
pub async fn spawn_if_configured(
    session_id: &str,
    default_port: Option<u16>,
    auth_token: Option<&str>,
    mux_url: Option<String>,
    agent: &str,
    labels: &[String],
    shutdown: CancellationToken,
) {
    let mux_url = match mux_url {
        Some(url) => url,
        None => return,
    };
    let coop_url = match (std::env::var("COOP_URL"), default_port) {
        (Ok(url), _) => url,
        (Err(_), Some(port)) => {
            // In Kubernetes, use POD_IP so the mux can reach us across pods.
            let host = std::env::var("POD_IP").unwrap_or_else(|_| "127.0.0.1".to_owned());
            format!("http://{host}:{port}")
        }
        (Err(_), None) => return, // no HTTP server, nothing to register
    };
    let reg = MuxRegistration {
        mux_url,
        mux_token: std::env::var("COOP_MUX_TOKEN").ok(),
        session_id: session_id.to_owned(),
        coop_url,
        coop_token: auth_token.map(str::to_owned),
        agent: agent.to_owned(),
        labels: labels.to_vec(),
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
    // Use a shorter interval locally so sessions reappear quickly after mux restart.
    let heartbeat_secs = if std::env::var("KUBERNETES_SERVICE_HOST").is_ok() { 60 } else { 15 };
    let heartbeat = std::time::Duration::from_secs(heartbeat_secs);
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

/// Detect session metadata from the agent type, `--label` CLI flags,
/// and (when running in Kubernetes) pod environment variables.
///
/// Always returns a JSON object with at least `"agent"`.
pub fn detect_metadata(agent: &str, labels: &[String]) -> Value {
    detect_metadata_with(agent, labels, |name| std::env::var(name).ok())
}

/// Inner implementation that accepts a lookup function for testability.
fn detect_metadata_with(
    agent: &str,
    labels: &[String],
    get_env: impl Fn(&str) -> Option<String>,
) -> Value {
    let mut meta = serde_json::Map::new();

    // Always inject agent type.
    meta.insert("agent".to_owned(), Value::String(agent.to_owned()));

    // Parse --label flags. Dot-separated keys build nested objects:
    //   "a.b.c=v" → {"a":{"b":{"c":"v"}}}
    for label in labels {
        let Some((key, val)) = label.split_once('=') else {
            warn!(label, "ignoring malformed label (expected key=value)");
            continue;
        };
        if key.is_empty() {
            warn!(label, "ignoring label with empty key");
            continue;
        }
        insert_nested(&mut meta, key, val);
    }

    // K8s metadata (only when running in Kubernetes).
    if get_env("KUBERNETES_SERVICE_HOST").is_some() {
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

        meta.insert("k8s".to_owned(), Value::Object(k8s));
    }

    Value::Object(meta)
}

/// Insert a value at a dot-separated key path, creating nested objects as needed.
fn insert_nested(map: &mut serde_json::Map<String, Value>, key: &str, val: &str) {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().unwrap_or(key);

    let mut current = map;
    for part in parts {
        // Ensure intermediate keys are objects.
        let entry =
            current.entry(part.to_owned()).or_insert_with(|| Value::Object(Default::default()));
        current = match entry.as_object_mut() {
            Some(obj) => obj,
            None => return, // non-object at intermediate key, skip
        };
    }
    current.insert(leaf.to_owned(), Value::String(val.to_owned()));
}

/// POST /api/v1/sessions to register this coop instance.
async fn register(
    client: &reqwest::Client,
    base: &str,
    config: &MuxRegistration,
) -> anyhow::Result<()> {
    let url = format!("{base}/api/v1/sessions");
    let metadata = detect_metadata(&config.agent, &config.labels);
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
