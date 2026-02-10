// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Generic hook-based detector shared by all agent drivers.
//!
//! Each agent provides a mapping function from [`HookEvent`] to
//! `(AgentState, cause)` pairs; the select loop is identical.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::hook_recv::HookReceiver;
use crate::driver::{AgentState, Detector, HookEvent};

/// Tier 1 detector that maps hook events to agent states via a
/// caller-supplied closure.
pub struct HookDetector<F>
where
    F: Fn(HookEvent) -> Option<(AgentState, String)> + Send + 'static,
{
    pub receiver: HookReceiver,
    pub map_event: F,
}

impl<F> Detector for HookDetector<F>
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
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = receiver.next_event() => {
                        match event {
                            Some(hook_event) => {
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
