// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS credential event publisher for coopmux.
//!
//! Subscribes to the credential broker's `broadcast::Receiver<CredentialEvent>`
//! and publishes JSON payloads to `{prefix}.events.credential` so external
//! consumers can react without polling the HTTP API.

use serde::Serialize;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::credential::CredentialEvent;
use crate::NatsConfig;

/// JSON payload for credential events published to NATS.
#[derive(Debug, Serialize)]
pub struct CredentialEventPayload {
    pub event_type: String,
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    pub ts: String,
}

/// Publishes credential events to a NATS subject.
pub struct NatsPublisher {
    client: async_nats::Client,
    prefix: String,
}

impl NatsPublisher {
    /// Connect to the NATS server and return a publisher.
    pub async fn connect(config: &NatsConfig) -> anyhow::Result<Self> {
        let mut opts = async_nats::ConnectOptions::new();
        if let Some(ref token) = config.token {
            opts = opts.token(token.clone());
        }
        opts = opts.retry_on_initial_connect();

        info!(url = %config.url, prefix = %config.prefix, "connecting NATS publisher");
        let client = opts.connect(&config.url).await?;
        info!("NATS publisher connected");

        Ok(Self { client, prefix: config.prefix.clone() })
    }

    /// Run the publisher loop, consuming `CredentialEvent`s and publishing
    /// them to NATS until shutdown.
    pub async fn run(
        self,
        mut cred_rx: broadcast::Receiver<CredentialEvent>,
        shutdown: CancellationToken,
    ) {
        let subject = format!("{}.events.credential", self.prefix);

        loop {
            tokio::select! {
                event = cred_rx.recv() => {
                    match event {
                        Ok(cred_event) => {
                            let payload = match cred_event {
                                CredentialEvent::Refreshed { account, .. } => {
                                    CredentialEventPayload {
                                        event_type: "refreshed".to_owned(),
                                        account,
                                        error: None,
                                        auth_url: None,
                                        user_code: None,
                                        ts: iso8601_now(),
                                    }
                                }
                                CredentialEvent::RefreshFailed { account, error } => {
                                    CredentialEventPayload {
                                        event_type: "refresh_failed".to_owned(),
                                        account,
                                        error: Some(error),
                                        auth_url: None,
                                        user_code: None,
                                        ts: iso8601_now(),
                                    }
                                }
                                CredentialEvent::ReauthRequired { account, auth_url, user_code } => {
                                    CredentialEventPayload {
                                        event_type: "reauth_required".to_owned(),
                                        account,
                                        error: None,
                                        auth_url: if auth_url.is_empty() { None } else { Some(auth_url) },
                                        user_code,
                                        ts: iso8601_now(),
                                    }
                                }
                            };
                            if let Ok(json) = serde_json::to_vec(&payload) {
                                if let Err(e) = self.client.publish(
                                    subject.clone(), json.into()
                                ).await {
                                    warn!("NATS publish credential failed: {e}");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!("NATS publisher lagged {n} credential events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = shutdown.cancelled() => break,
            }
        }

        debug!("NATS credential publisher shutting down");
    }
}

/// Return the current UTC time as an ISO 8601 string (e.g. "2026-02-14T01:23:45Z").
fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    // Civil calendar from days since epoch (Howard Hinnant's algorithm).
    let days = secs / 86400;
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}
