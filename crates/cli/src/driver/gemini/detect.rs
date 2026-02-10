// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

use crate::driver::hook_detect::HookDetector;
use crate::driver::hook_recv::HookReceiver;
use crate::driver::HookEvent;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};
use crate::event::{RawHookEvent, RawMessageEvent};

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
            Some((AgentState::Working, "hook:working".into()))
        }
        HookEvent::ToolBefore { .. } => {
            // BeforeTool fires for every tool call (including
            // auto-approved ones). Map to Working; actual
            // permission prompts are detected via Notification.
            Some((AgentState::Working, "hook:working".into()))
        }
        HookEvent::TurnEnd | HookEvent::SessionEnd => Some((AgentState::Idle, "hook:idle".into())),
        HookEvent::Notification { notification_type } => match notification_type.as_str() {
            "ToolPermission" => Some((
                AgentState::Prompt {
                    prompt: PromptContext::new(PromptKind::Permission).with_subtype("tool"),
                },
                "hook:prompt(permission)".into(),
            )),
            _ => None,
        },
        HookEvent::SessionStart => None,
    }
}

/// Create a Tier 1 hook detector for Gemini.
pub fn new_hook_detector(
    receiver: HookReceiver,
    raw_hook_tx: Option<broadcast::Sender<RawHookEvent>>,
) -> impl Detector {
    HookDetector { receiver, map_event: map_gemini_hook, raw_hook_tx }
}

/// Create a Tier 3 stdout detector for Gemini.
///
/// Parses structured JSONL from Gemini's stdout stream (used when Gemini is
/// invoked with `--output-format stream-json`). Receives raw PTY bytes from
/// a channel, feeds them through a JSONL parser, and classifies each parsed
/// entry with `parse_gemini_state`.
pub fn new_stdout_detector(
    stdout_rx: mpsc::Receiver<Bytes>,
    raw_message_tx: Option<broadcast::Sender<RawMessageEvent>>,
) -> impl Detector {
    use crate::driver::stdout_detect::StdoutDetector;
    StdoutDetector {
        stdout_rx,
        classify: Box::new(|json| {
            let state = parse_gemini_state(json)?;
            let cause = format_gemini_cause(json);
            Some((state, cause))
        }),
        extract_message: None,
        last_message: None,
        raw_message_tx,
    }
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
