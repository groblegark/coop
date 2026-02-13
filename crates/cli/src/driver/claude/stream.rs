// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::hook_detect::HookDetector;
use crate::driver::hook_recv::HookReceiver;
use crate::driver::log_watch::LogWatcher;
use crate::driver::HookEvent;
use crate::driver::{AgentState, Detector, DetectorEmission, PromptContext, PromptKind};
use crate::event::{RawHookEvent, RawMessageEvent};
use crate::usage::UsageState;

use super::parse::{extract_assistant_text, format_claude_cause, parse_claude_state};
use super::prompt::extract_ask_user_from_tool_input;

/// Map a Claude hook event to an `(AgentState, cause)` pair.
///
/// - `TurnEnd` / `SessionEnd` → `Idle`
/// - `Notification(idle_prompt)` → `Idle`
/// - `Notification(permission_prompt)` → `Prompt(Permission)`
/// - `ToolBefore(AskUserQuestion)` → `Prompt(Question)` with context
/// - `ToolBefore(ExitPlanMode)` → `Prompt(Plan)`
/// - `ToolBefore(EnterPlanMode)` → `Working`
/// - `ToolAfter` → `Working`
///
/// Returns `None` for events that should be ignored (e.g. `SessionStart`, unrecognised notifications).
pub fn map_claude_hook(event: HookEvent) -> Option<(AgentState, String)> {
    match event {
        HookEvent::TurnEnd | HookEvent::SessionEnd => {
            Some((AgentState::Idle, "hook:idle".to_owned()))
        }
        HookEvent::ToolAfter { .. } => Some((AgentState::Working, "hook:working".into())),
        HookEvent::Notification { notification_type } => match notification_type.as_str() {
            "idle_prompt" => Some((AgentState::Idle, "hook:idle".into())),
            "permission_prompt" => Some((
                AgentState::Prompt { prompt: PromptContext::new(PromptKind::Permission) },
                "hook:prompt(permission)".into(),
            )),
            _ => None,
        },
        HookEvent::ToolBefore { ref tool, ref tool_input } => match tool.as_str() {
            "AskUserQuestion" => Some((
                AgentState::Prompt {
                    prompt: extract_ask_user_from_tool_input(tool_input.as_ref()),
                },
                "hook:prompt(question)".into(),
            )),
            "ExitPlanMode" => {
                let mut ctx = PromptContext::new(PromptKind::Plan);
                if let Some(input) = tool_input {
                    if let Ok(s) = serde_json::to_string(input) {
                        ctx = ctx.with_input(s);
                    }
                }
                ctx = ctx.with_ready();
                Some((AgentState::Prompt { prompt: ctx }, "hook:prompt(plan)".into()))
            }
            "EnterPlanMode" => Some((AgentState::Working, "hook:working".into())),
            _ => None,
        },
        HookEvent::TurnStart => Some((AgentState::Working, "hook:working".into())),
        HookEvent::SessionStart => None,
    }
}

/// Create a Tier 1 hook detector for Claude.
pub fn new_hook_detector(
    receiver: HookReceiver,
    raw_hook_tx: Option<broadcast::Sender<RawHookEvent>>,
) -> impl Detector {
    HookDetector { receiver, map_event: map_claude_hook, raw_hook_tx }
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
    /// Optional sender for raw message JSON broadcast.
    pub raw_message_tx: Option<broadcast::Sender<RawMessageEvent>>,
    /// Optional usage tracking state.
    pub usage: Option<Arc<UsageState>>,
}

impl Detector for LogDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<DetectorEmission>,
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
            let raw_message_tx = self.raw_message_tx;
            let usage = self.usage;

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
                                        if let Some(ref tx) = raw_message_tx {
                                            let _ = tx.send(RawMessageEvent {
                                                json: json.clone(),
                                                source: "log".to_owned(),
                                            });
                                        }
                                        if let Some(text) = extract_assistant_text(&json) {
                                            if let Some(ref lm) = last_message {
                                                *lm.write().await = Some(text);
                                            }
                                        }
                                        if let Some(state) = parse_claude_state(&json) {
                                            let cause = format_claude_cause(&json, "log");
                                            // Interrupt-detected idle is a high-confidence
                                            // signal — emit at Tier 1 so it can override
                                            // the hook detector's stale Working state.
                                            let tier_override = if cause == "log:user" {
                                                Some(1)
                                            } else {
                                                None
                                            };
                                            let _ = state_tx.send((state, cause, tier_override)).await;
                                        }
                                        if let Some(ref u) = usage {
                                            if let Some(delta) = crate::usage::extract_usage_delta(&json) {
                                                u.accumulate(delta).await;
                                            }
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

/// Create a Tier 3 stdout detector for Claude.
///
/// Parses structured JSONL from Claude's stdout stream (used when Claude is
/// invoked with `--print --output-format stream-json`). Classifies each entry
/// with `parse_claude_state` and extracts assistant message text.
pub fn new_stdout_detector(
    stdout_rx: mpsc::Receiver<Bytes>,
    last_message: Option<Arc<RwLock<Option<String>>>>,
    raw_message_tx: Option<broadcast::Sender<RawMessageEvent>>,
) -> impl Detector {
    use crate::driver::stdout_detect::StdoutDetector;
    StdoutDetector {
        stdout_rx,
        classify: Box::new(|json| {
            let state = parse_claude_state(json)?;
            let cause = format_claude_cause(json, "stdout");
            Some((state, cause))
        }),
        extract_message: Some(Box::new(extract_assistant_text)),
        last_message,
        raw_message_tx,
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
