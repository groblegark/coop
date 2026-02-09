// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::hook_recv::HookReceiver;
use crate::driver::jsonl_stdout::JsonlParser;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};
use crate::event::HookEvent;

use super::state::parse_gemini_state;

/// Tier 1 detector: receives push events from Gemini's hook system.
///
/// Maps hook events to agent states:
/// - `ToolComplete` -> `Working`
/// - `SessionEnd` -> `WaitingForInput`
/// - `Notification("ToolPermission")` -> `Prompt(Permission)`
pub struct HookDetector {
    pub receiver: HookReceiver,
}

impl Detector for HookDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut receiver = self.receiver;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = receiver.next_event() => {
                        let state = match event {
                            Some(HookEvent::SessionEnd) => {
                                AgentState::WaitingForInput
                            }
                            Some(HookEvent::ToolComplete { .. }) => {
                                AgentState::Working
                            }
                            Some(HookEvent::Notification { notification_type }) => {
                                match notification_type.as_str() {
                                    "ToolPermission" => AgentState::Prompt {
                                        prompt: PromptContext {
                                            kind: PromptKind::Permission,
                                            tool: None,
                                            input_preview: None,
                                            screen_lines: vec![],
                                            questions: vec![],
                                            question_current: 0,
                                        },
                                    },
                                    _ => continue,
                                }
                            }
                            Some(HookEvent::AgentStop) => AgentState::WaitingForInput,
                            Some(HookEvent::PreToolUse { tool, tool_input }) => {
                                AgentState::Prompt {
                                    prompt: PromptContext {
                                        kind: PromptKind::Permission,
                                        tool: Some(tool),
                                        input_preview: tool_input
                                            .as_ref()
                                            .and_then(|v| serde_json::to_string(v).ok()),
                                        screen_lines: vec![],
                                        questions: vec![],
                                        question_current: 0,
                                    },
                                }
                            }
                            None => break,
                        };
                        let _ = state_tx.send(state).await;
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        1
    }
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
        state_tx: mpsc::Sender<AgentState>,
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
                                        let _ = state_tx.send(state).await;
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
