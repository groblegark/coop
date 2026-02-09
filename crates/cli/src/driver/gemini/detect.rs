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

use super::state::{format_gemini_cause, parse_gemini_state};

/// Tier 1 detector: receives push events from Gemini's hook system.
///
/// Maps hook events to agent states:
/// - `AgentStart` / `PreToolUse` / `ToolComplete` -> `Working`
/// - `AgentStop` / `SessionEnd` -> `WaitingForInput`
/// - `Notification("ToolPermission")` -> `Prompt(Permission)`
pub struct HookDetector {
    pub receiver: HookReceiver,
}

impl Detector for HookDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut receiver = self.receiver;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = receiver.next_event() => {
                        let (state, cause) = match event {
                            Some(HookEvent::AgentStart) => {
                                (AgentState::Working, "hook:working".to_owned())
                            }
                            Some(HookEvent::SessionEnd) => {
                                (AgentState::WaitingForInput, "hook:idle".to_owned())
                            }
                            Some(HookEvent::ToolComplete { .. }) => {
                                (AgentState::Working, "hook:working".to_owned())
                            }
                            Some(HookEvent::Notification { notification_type }) => {
                                match notification_type.as_str() {
                                    "ToolPermission" => (AgentState::Prompt {
                                        prompt: PromptContext {
                                            kind: PromptKind::Permission,
                                            tool: None,
                                            input_preview: None,
                                            screen_lines: vec![],
                                            questions: vec![],
                                            question_current: 0,
                                        },
                                    }, "hook:prompt(permission)".to_owned()),
                                    _ => continue,
                                }
                            }
                            Some(HookEvent::AgentStop) => (AgentState::WaitingForInput, "hook:idle".to_owned()),
                            Some(HookEvent::PreToolUse { .. }) => {
                                // BeforeTool fires for every tool call (including
                                // auto-approved ones). Map to Working; actual
                                // permission prompts are detected via Notification.
                                (AgentState::Working, "hook:working".to_owned())
                            }
                            None => break,
                        };
                        let _ = state_tx.send((state, cause)).await;
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
