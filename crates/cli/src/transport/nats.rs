// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS event publisher — broadcasts coop events to NATS subjects.

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::transport::Store;

/// Publishes coop events to NATS subjects as JSON.
pub struct NatsPublisher {
    client: async_nats::Client,
    prefix: String,
}

impl NatsPublisher {
    /// Connect to the NATS server at `url` and configure the subject prefix.
    pub async fn connect(url: &str, prefix: &str) -> anyhow::Result<Self> {
        let client = async_nats::connect(url).await?;
        Ok(Self { client, prefix: prefix.to_owned() })
    }

    /// Subscribe to all broadcast channels and publish events until shutdown.
    pub async fn run(self, store: &Store, shutdown: CancellationToken) {
        let mut state_rx = store.channels.state_tx.subscribe();
        let mut prompt_rx = store.channels.prompt_tx.subscribe();
        let mut hook_rx = store.channels.hook_tx.subscribe();
        let mut stop_rx = store.stop.stop_tx.subscribe();
        let mut start_rx = store.start.start_tx.subscribe();
        let mut usage_rx = store.usage.usage_tx.subscribe();

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                event = state_rx.recv() => {
                    self.handle(event, &format!("{}.state", self.prefix)).await;
                }
                event = prompt_rx.recv() => {
                    self.handle(event, &format!("{}.prompt", self.prefix)).await;
                }
                event = hook_rx.recv() => {
                    self.handle(event, &format!("{}.hook", self.prefix)).await;
                }
                event = stop_rx.recv() => {
                    self.handle(event, &format!("{}.stop", self.prefix)).await;
                }
                event = start_rx.recv() => {
                    self.handle(event, &format!("{}.start", self.prefix)).await;
                }
                event = usage_rx.recv() => {
                    self.handle(event, &format!("{}.usage", self.prefix)).await;
                }
            }
        }
    }

    /// Serialize and publish a single event, logging errors without propagating.
    async fn handle<T: serde::Serialize>(
        &self,
        result: Result<T, broadcast::error::RecvError>,
        subject: &str,
    ) {
        match result {
            Ok(event) => {
                let payload = match serde_json::to_vec(&event) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("nats: failed to serialize event for {subject}: {e}");
                        return;
                    }
                };
                if let Err(e) = self.client.publish(subject.to_owned(), payload.into()).await {
                    tracing::warn!("nats: publish to {subject} failed: {e}");
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!("nats: {subject} subscriber lagged by {n}");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::debug!("nats: {subject} channel closed");
            }
        }
    }
}
