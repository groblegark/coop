// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS event publisher — broadcasts coop state transitions, stop events,
//! and credential events to a NATS server so external consumers can subscribe
//! without polling.
//!
//! This is the outbound counterpart to the inbound `driver::nats_recv` module.
//! The receiver consumes hook events from bd daemon; the publisher emits coop's
//! own derived events (state transitions, stop verdicts, credential refreshes)
//! back to NATS.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::credential::CredentialEvent;
use crate::event::TransitionEvent;
use crate::stop::StopEvent;

/// Configuration for NATS event publishing.
#[derive(Debug, Clone)]
pub struct NatsPubConfig {
    /// NATS server URL (e.g. "nats://127.0.0.1:4222").
    pub url: String,
    /// Auth token for NATS connection.
    pub token: Option<String>,
    /// Subject prefix for published events (default: "coop.events").
    pub prefix: String,
}

/// Publishes coop events to NATS subjects.
///
/// Subscribes to the same `broadcast` channels that HTTP/gRPC/WebSocket
/// transports use and publishes JSON payloads to NATS:
///
/// - `{prefix}.state` — agent state transitions
/// - `{prefix}.stop` — stop hook verdict events
/// - `{prefix}.credential` — credential refresh / failure events
pub struct NatsPublisher {
    client: async_nats::Client,
    prefix: String,
}

/// JSON payload for state transition events published to NATS.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct StateEventPayload {
    pub prev: String,
    pub next: String,
    pub seq: u64,
    pub cause: Option<String>,
    pub last_message: Option<String>,
}

/// JSON payload for stop events published to NATS.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct StopEventPayload {
    pub stop_type: String,
    pub signal_json: Option<String>,
    pub error_detail: Option<String>,
    pub seq: u64,
}

/// JSON payload for credential events published to NATS.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CredentialEventPayload {
    pub event_type: String,
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub ts: String,
}

impl NatsPublisher {
    /// Create a publisher from an already-connected NATS client.
    pub fn new(client: async_nats::Client, prefix: String) -> Self {
        Self { client, prefix }
    }

    /// Connect to the NATS server and return a publisher.
    pub async fn connect(config: &NatsPubConfig) -> anyhow::Result<Self> {
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

    /// Run the publisher loop, consuming events from broadcast channels
    /// and publishing them to NATS until shutdown.
    pub async fn run(
        self,
        mut state_rx: broadcast::Receiver<TransitionEvent>,
        mut stop_rx: broadcast::Receiver<StopEvent>,
        mut cred_rx: broadcast::Receiver<CredentialEvent>,
        shutdown: CancellationToken,
    ) {
        let state_subject = format!("{}.state", self.prefix);
        let stop_subject = format!("{}.stop", self.prefix);
        let cred_subject = format!("{}.credential", self.prefix);

        loop {
            tokio::select! {
                event = state_rx.recv() => {
                    match event {
                        Ok(event) => {
                            let payload = StateEventPayload {
                                prev: event.prev.as_str().to_owned(),
                                next: event.next.as_str().to_owned(),
                                seq: event.seq,
                                cause: if event.cause.is_empty() { None } else { Some(event.cause) },
                                last_message: event.last_message,
                            };
                            if let Ok(json) = serde_json::to_vec(&payload) {
                                if let Err(e) = self.client.publish(
                                    state_subject.clone(), json.into()
                                ).await {
                                    warn!("NATS publish state failed: {e}");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!("NATS publisher lagged {n} state events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                event = stop_rx.recv() => {
                    match event {
                        Ok(event) => {
                            let payload = StopEventPayload {
                                stop_type: event.r#type.as_str().to_owned(),
                                signal_json: event.signal.map(|v| v.to_string()),
                                error_detail: event.error_detail,
                                seq: event.seq,
                            };
                            if let Ok(json) = serde_json::to_vec(&payload) {
                                if let Err(e) = self.client.publish(
                                    stop_subject.clone(), json.into()
                                ).await {
                                    warn!("NATS publish stop failed: {e}");
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            debug!("NATS publisher lagged {n} stop events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                event = cred_rx.recv() => {
                    match event {
                        Ok(event) => {
                            let payload = match event {
                                CredentialEvent::RefreshFailed { account, error } => {
                                    CredentialEventPayload {
                                        event_type: "refresh_failed".to_owned(),
                                        account,
                                        error: Some(error),
                                        ts: iso8601_now(),
                                    }
                                }
                                CredentialEvent::Refreshed { account, .. } => {
                                    CredentialEventPayload {
                                        event_type: "refreshed".to_owned(),
                                        account,
                                        error: None,
                                        ts: iso8601_now(),
                                    }
                                }
                            };
                            if let Ok(json) = serde_json::to_vec(&payload) {
                                if let Err(e) = self.client.publish(
                                    cred_subject.clone(), json.into()
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

        debug!("NATS publisher shutting down");
    }
}

/// Return the current UTC time as an ISO 8601 string (e.g. "2026-02-11T12:34:56Z").
fn iso8601_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();
    // Decompose seconds into date/time components.
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    // Convert days since epoch to Y-M-D (civil calendar).
    // Algorithm from Howard Hinnant's `civil_from_days`.
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
