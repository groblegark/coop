// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Broker registration client.
//!
//! When `COOP_BROKER_URL` is set, a coop instance registers itself with the
//! broker on startup so the broker can health-check it, stream its terminal
//! output to the mux dashboard, and push refreshed credentials.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::registry::RegisterRequest;

/// How often to re-register (heartbeat) with the broker.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// Timeout for registration HTTP requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for the broker registration client.
#[derive(Debug, Clone)]
pub struct BrokerClientConfig {
    /// Broker base URL (e.g. "http://coop-broker:8080").
    pub broker_url: String,
    /// Auth token for the broker API.
    pub broker_token: Option<String>,
    /// This pod's name (used as registration key).
    pub pod_name: String,
    /// This pod's coop URL reachable from the broker (e.g. "http://10.42.0.5:8080").
    pub coop_url: String,
}

impl BrokerClientConfig {
    /// Build config from environment and CLI args.
    ///
    /// Returns `None` if `COOP_BROKER_URL` is not set (non-broker mode).
    pub fn from_config(config: &crate::config::Config) -> Option<Self> {
        let broker_url = config.broker_url.as_ref()?.trim_end_matches('/').to_owned();

        let broker_token = config
            .broker_token
            .clone()
            .or_else(|| std::env::var("BD_DAEMON_TOKEN").ok());

        // Pod name: explicit > HOSTNAME > "unknown"
        let pod_name = config
            .broker_pod_name
            .clone()
            .or_else(|| std::env::var("HOSTNAME").ok())
            .unwrap_or_else(|| "unknown".into());

        // Advertised coop URL: use pod IP (from POD_IP env) or fall back to hostname.
        // The broker needs to reach this pod's coop API for health checks and WS streams.
        let port = config.port.unwrap_or(8080);
        let coop_url = if let Ok(pod_ip) = std::env::var("POD_IP") {
            format!("http://{}:{}", pod_ip, port)
        } else if let Ok(hostname) = std::env::var("HOSTNAME") {
            format!("http://{}:{}", hostname, port)
        } else {
            format!("http://127.0.0.1:{}", port)
        };

        Some(Self { broker_url, broker_token, pod_name, coop_url })
    }
}

/// Run the broker registration loop.
///
/// Registers on startup, then re-registers periodically as a heartbeat.
/// Deregisters on shutdown.
pub async fn run(config: BrokerClientConfig, shutdown: CancellationToken) {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap_or_default();

    // Initial registration with retries.
    let mut registered = false;
    for attempt in 1..=5u32 {
        match register(&client, &config).await {
            Ok(()) => {
                registered = true;
                break;
            }
            Err(e) => {
                warn!(
                    attempt,
                    broker = %config.broker_url,
                    "broker registration failed: {e}"
                );
                let delay = Duration::from_secs(2u64.pow(attempt.min(4)));
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    _ = shutdown.cancelled() => return,
                }
            }
        }
    }

    if !registered {
        error!(
            broker = %config.broker_url,
            "failed to register with broker after retries, giving up"
        );
        return;
    }

    // Heartbeat loop — re-register periodically.
    loop {
        tokio::select! {
            _ = tokio::time::sleep(HEARTBEAT_INTERVAL) => {}
            _ = shutdown.cancelled() => break,
        }
        if let Err(e) = register(&client, &config).await {
            debug!(broker = %config.broker_url, "heartbeat re-register failed: {e}");
        }
    }

    // Deregister on shutdown.
    if let Err(e) = deregister(&client, &config).await {
        debug!(broker = %config.broker_url, "deregister failed: {e}");
    }
}

async fn register(client: &reqwest::Client, config: &BrokerClientConfig) -> Result<(), String> {
    let body = RegisterRequest {
        name: config.pod_name.clone(),
        coop_url: config.coop_url.clone(),
        profiles_needed: vec![],
        auth_token: None,
    };

    let url = format!("{}/api/v1/broker/register", config.broker_url);
    let mut req = client.post(&url).json(&body);
    if let Some(ref token) = config.broker_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = req.send().await.map_err(|e| format!("{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {text}"));
    }

    info!(
        pod = %config.pod_name,
        coop_url = %config.coop_url,
        broker = %config.broker_url,
        "registered with broker"
    );
    Ok(())
}

async fn deregister(client: &reqwest::Client, config: &BrokerClientConfig) -> Result<(), String> {
    let url = format!("{}/api/v1/broker/deregister", config.broker_url);
    let body = serde_json::json!({ "name": config.pod_name });
    let mut req = client.post(&url).json(&body);
    if let Some(ref token) = config.broker_token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    let resp = req.send().await.map_err(|e| format!("{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {text}"));
    }

    info!(pod = %config.pod_name, broker = %config.broker_url, "deregistered from broker");
    Ok(())
}
