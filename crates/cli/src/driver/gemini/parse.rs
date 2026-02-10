// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::Value;

use crate::driver::AgentState;

/// Extract a semantic cause string from a Gemini stream-json JSONL event.
pub fn format_gemini_cause(json: &Value) -> String {
    match json.get("type").and_then(|v| v.as_str()) {
        Some("result") => "stdout:idle".to_owned(),
        Some("error") => "stdout:error".to_owned(),
        _ => "stdout:working".to_owned(),
    }
}

/// Parse a Gemini stream-json JSONL event into an [`AgentState`].
///
/// Handles the event types from `--output-format stream-json`:
/// - `init`, `message`, `tool_use`, `tool_result` -> `Working`
/// - `result` -> `Idle`
/// - `error` -> `Error { detail }`
///
/// Returns `None` if the entry cannot be classified.
pub fn parse_gemini_state(json: &Value) -> Option<AgentState> {
    match json.get("type").and_then(|v| v.as_str()) {
        Some("result") => Some(AgentState::Idle),
        Some("error") => {
            let detail =
                json.get("message").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            Some(AgentState::Error { detail })
        }
        Some("init" | "message" | "tool_use" | "tool_result") => Some(AgentState::Working),
        _ => None,
    }
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
