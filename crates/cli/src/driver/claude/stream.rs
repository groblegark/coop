// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::hook_recv::HookReceiver;
use crate::driver::jsonl_stdout::JsonlParser;
use crate::driver::log_watch::LogWatcher;
use crate::driver::HookEvent;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};

use super::parse::{extract_assistant_text, format_claude_cause, parse_claude_state};
use super::prompt::extract_ask_user_from_tool_input;

/// Tier 1 detector: receives push events from Claude's hook system.
///
/// Maps hook events to agent states:
/// - `AgentStop` / `SessionEnd` → `WaitingForInput`
/// - `ToolComplete` → `Working`
/// - `Notification(idle_prompt)` → `WaitingForInput`
/// - `Notification(permission_prompt)` → `Prompt(Permission)`
/// - `PreToolUse(AskUserQuestion)` → `Prompt(Question)` with context
/// - `PreToolUse(ExitPlanMode)` → `Prompt(Plan)`
/// - `PreToolUse(EnterPlanMode)` → `Working`
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
                            Some(HookEvent::AgentStop) | Some(HookEvent::SessionEnd) => {
                                (AgentState::WaitingForInput, "hook:idle".to_owned())
                            }
                            Some(HookEvent::ToolComplete { .. }) => {
                                (AgentState::Working, "hook:working".to_owned())
                            }
                            Some(HookEvent::Notification { notification_type }) => {
                                match notification_type.as_str() {
                                    "idle_prompt" => (AgentState::WaitingForInput, "hook:idle".to_owned()),
                                    "permission_prompt" => (AgentState::Prompt {
                                        prompt: PromptContext {
                                            kind: PromptKind::Permission,
                                            subtype: None,
                                            tool: None,
                                            input: None,
                                            auth_url: None,
                                            options: vec![],
                                            options_fallback: false,
                                            questions: vec![],
                                            question_current: 0,
                                            ready: false,
                                        },
                                    }, "hook:prompt(permission)".to_owned()),
                                    _ => continue,
                                }
                            }
                            Some(HookEvent::PreToolUse { ref tool, ref tool_input }) => {
                                match tool.as_str() {
                                    "AskUserQuestion" => (AgentState::Prompt {
                                        prompt: extract_ask_user_from_tool_input(tool_input.as_ref()),
                                    }, "hook:prompt(question)".to_owned()),
                                    "ExitPlanMode" => (AgentState::Prompt {
                                        prompt: PromptContext {
                                            kind: PromptKind::Plan,
                                            subtype: None,
                                            tool: None,
                                            input: None,
                                            auth_url: None,
                                            options: vec![],
                                            options_fallback: false,
                                            questions: vec![],
                                            question_current: 0,
                                            ready: false,
                                        },
                                    }, "hook:prompt(plan)".to_owned()),
                                    "EnterPlanMode" => (AgentState::Working, "hook:working".to_owned()),
                                    _ => continue,
                                }
                            }
                            Some(_) => continue,
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

/// Tier 2 detector: watches Claude's session log file for new JSONL entries.
///
/// Parses each new line with `parse_claude_state` and emits the resulting
/// state to the composite detector.
pub struct LogDetector {
    pub log_path: PathBuf,
    /// Byte offset to start reading from (used for session resume).
    pub start_offset: u64,
    /// Fallback poll interval for the log watcher.
    pub poll_interval: Duration,
    /// Shared last assistant message text (written directly, bypasses detector pipeline).
    pub last_message: Option<Arc<RwLock<Option<String>>>>,
}

impl Detector for LogDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let watcher = if self.start_offset > 0 {
                LogWatcher::with_offset(self.log_path, self.start_offset)
            } else {
                LogWatcher::new(self.log_path)
            }
            .with_poll_interval(self.poll_interval);
            let (line_tx, mut line_rx) = mpsc::channel(32);
            let watch_shutdown = shutdown.clone();
            let last_message = self.last_message;

            tokio::spawn(async move {
                watcher.run(line_tx, watch_shutdown).await;
            });

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    batch = line_rx.recv() => {
                        match batch {
                            Some(lines) => {
                                for line in &lines {
                                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                                        if let Some(text) = extract_assistant_text(&json) {
                                            if let Some(ref lm) = last_message {
                                                *lm.write().await = Some(text);
                                            }
                                        }
                                        if let Some(state) = parse_claude_state(&json) {
                                            let cause = format_claude_cause(&json, "log");
                                            let _ = state_tx.send((state, cause)).await;
                                        }
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
        2
    }
}

/// Tier 3 detector: parses structured JSONL from Claude's stdout stream.
///
/// Used when Claude is invoked with `--print --output-format stream-json`.
/// Receives raw PTY bytes from a channel, feeds them through a JSONL parser,
/// and classifies each parsed entry.
pub struct StdoutDetector {
    pub stdout_rx: mpsc::Receiver<Bytes>,
    /// Shared last assistant message text (written directly, bypasses detector pipeline).
    pub last_message: Option<Arc<RwLock<Option<String>>>>,
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
            let last_message = self.last_message;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    data = stdout_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                for json in parser.feed(&bytes) {
                                    if let Some(text) = extract_assistant_text(&json) {
                                        if let Some(ref lm) = last_message {
                                            *lm.write().await = Some(text);
                                        }
                                    }
                                    if let Some(state) = parse_claude_state(&json) {
                                        let cause = format_claude_cause(&json, "stdout");
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
#[path = "stream_tests.rs"]
mod tests;
