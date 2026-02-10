// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS event publisher — broadcasts coop state transitions and stop events
//! to a NATS server so external consumers can subscribe without polling.
//!
//! This is the outbound counterpart to the inbound `driver::nats_recv` module.
//! The receiver consumes hook events from bd daemon; the publisher emits coop's
//! own derived events (state transitions, stop verdicts) back to NATS.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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
        shutdown: CancellationToken,
    ) {
        let state_subject = format!("{}.state", self.prefix);
        let stop_subject = format!("{}.stop", self.prefix);

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
                _ = shutdown.cancelled() => break,
            }
        }

        debug!("NATS publisher shutting down");
    }
}
