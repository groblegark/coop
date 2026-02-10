// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS-based Tier 1 detector — mirrors hook_detect.rs but reads from JetStream.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::driver::nats_recv::NatsReceiver;
use crate::driver::{AgentState, Detector, HookEvent};
use crate::event::RawHookEvent;

/// Tier 1 detector that reads hook events from NATS JetStream and maps them
/// to agent states via a caller-supplied closure.
///
/// This is the network counterpart to [`HookDetector`](super::hook_detect::HookDetector)
/// which reads from a local named pipe. Both produce the same `HookEvent` values
/// and map them through the same agent-specific closure.
pub struct NatsDetector<F>
where
    F: Fn(HookEvent) -> Option<(AgentState, String)> + Send + 'static,
{
    pub receiver: NatsReceiver,
    pub map_event: F,
    /// Optional sender for raw hook JSON broadcast.
    pub raw_hook_tx: Option<broadcast::Sender<RawHookEvent>>,
}

impl<F> Detector for NatsDetector<F>
where
    F: Fn(HookEvent) -> Option<(AgentState, String)> + Send + 'static,
{
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut receiver = self.receiver;
            let map_event = self.map_event;
            let raw_hook_tx = self.raw_hook_tx;

            // Connect to NATS (with retry on initial connect).
            if let Err(e) = receiver.connect().await {
                warn!("NATS detector failed to connect: {e}");
                return;
            }

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = receiver.next_event() => {
                        match event {
                            Some((hook_event, raw_json)) => {
                                if let Some(ref tx) = raw_hook_tx {
                                    let _ = tx.send(RawHookEvent { json: raw_json });
                                }
                                if let Some(pair) = map_event(hook_event) {
                                    let _ = state_tx.send(pair).await;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        1
    }
}
