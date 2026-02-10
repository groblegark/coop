// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::hook_detect::HookDetector;
use crate::driver::hook_recv::HookReceiver;
use crate::driver::jsonl_stdout::JsonlParser;
use crate::driver::HookEvent;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};

use super::parse::{format_gemini_cause, parse_gemini_state};

/// Map a Gemini hook event to an `(AgentState, cause)` pair.
///
/// - `TurnStart` / `ToolBefore` / `ToolAfter` → `Working`
/// - `TurnEnd` / `SessionEnd` → `Idle`
/// - `Notification("ToolPermission")` → `Prompt(Permission)`
///
/// Returns `None` for events that should be ignored (e.g. `SessionStart`, unrecognised notifications).
pub fn map_gemini_hook(event: HookEvent) -> Option<(AgentState, String)> {
    match event {
        HookEvent::TurnStart | HookEvent::ToolAfter { .. } => {
            Some((AgentState::Working, "hook:working".to_owned()))
        }
        HookEvent::ToolBefore { .. } => {
            // BeforeTool fires for every tool call (including
            // auto-approved ones). Map to Working; actual
            // permission prompts are detected via Notification.
            Some((AgentState::Working, "hook:working".to_owned()))
        }
        HookEvent::TurnEnd | HookEvent::SessionEnd => {
            Some((AgentState::Idle, "hook:idle".to_owned()))
        }
        HookEvent::Notification { notification_type } => match notification_type.as_str() {
            "ToolPermission" => Some((
                AgentState::Prompt {
                    prompt: PromptContext::new(PromptKind::Permission).with_subtype("tool"),
                },
                "hook:prompt(permission)".to_owned(),
            )),
            _ => None,
        },
        HookEvent::SessionStart => None,
    }
}

/// Create a Tier 1 hook detector for Gemini.
pub fn new_hook_detector(receiver: HookReceiver) -> impl Detector {
    HookDetector { receiver, map_event: map_gemini_hook }
}

/// Tier 3 detector: parses structured JSONL from Gemini's stdout stream.
///
/// Used when Gemini is invoked with `--output-format stream-json`.
/// Receives raw PTY bytes from a channel, feeds them through a JSONL parser,
/// and classifies each parsed entry.
pub struct StdoutDetector {
    pub stdout_rx: mpsc::Receiver<Bytes>,
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

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    data = stdout_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                for json in parser.feed(&bytes) {
                                    if let Some(state) = parse_gemini_state(&json) {
                                        let cause = format_gemini_cause(&json);
                                        let _ = state_tx.send((state, cause)).await;
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

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
