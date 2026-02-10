// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Generic stdout-based JSONL detector shared by all agent drivers.
//!
//! Each agent provides a classify function from parsed JSON to
//! `(AgentState, cause)` pairs; the select loop is identical.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::jsonl_stdout::JsonlParser;
use crate::driver::{AgentState, Detector};
use crate::event::RawMessageEvent;

/// Classifies a parsed JSON entry into an `(AgentState, cause)` pair.
type ClassifyFn = Box<dyn Fn(&serde_json::Value) -> Option<(AgentState, String)> + Send>;

/// Extracts the last assistant message text from a parsed JSON entry.
type ExtractMessageFn = Box<dyn Fn(&serde_json::Value) -> Option<String> + Send>;

/// Tier 3 detector that parses JSONL from an agent's stdout stream,
/// classifying each entry via caller-supplied closures.
pub struct StdoutDetector {
    pub stdout_rx: mpsc::Receiver<Bytes>,
    /// Classifies a parsed JSON entry into an `(AgentState, cause)` pair.
    pub classify: ClassifyFn,
    /// Optional extractor for the last assistant message text.
    pub extract_message: Option<ExtractMessageFn>,
    /// Shared last assistant message text (written directly, bypasses detector pipeline).
    pub last_message: Option<Arc<RwLock<Option<String>>>>,
    /// Optional sender for raw message JSON broadcast.
    pub raw_message_tx: Option<broadcast::Sender<RawMessageEvent>>,
}

impl Detector for StdoutDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut parser = JsonlParser::new();
            let mut stdout_rx = self.stdout_rx;
            let classify = self.classify;
            let extract_message = self.extract_message;
            let last_message = self.last_message;
            let raw_message_tx = self.raw_message_tx;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    data = stdout_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                for json in parser.feed(&bytes) {
                                    if let Some(ref tx) = raw_message_tx {
                                        let _ = tx.send(RawMessageEvent {
                                            json: json.clone(),
                                            source: "stdout".to_owned(),
                                        });
                                    }
                                    if let Some(ref extract) = extract_message {
                                        if let Some(text) = extract(&json) {
                                            if let Some(ref lm) = last_message {
                                                *lm.write().await = Some(text);
                                            }
                                        }
                                    }
                                    if let Some(pair) = classify(&json) {
                                        let _ = state_tx.send(pair).await;
                                    }
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
        3
    }
}
