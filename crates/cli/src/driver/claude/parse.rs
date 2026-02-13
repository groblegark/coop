// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::Value;

use crate::driver::AgentState;

use super::prompt::extract_ask_user_context;

/// Extract a semantic cause string from a Claude session log JSONL entry.
///
/// Uses the given `prefix` ("log" or "stdout") to build the cause.
pub fn format_claude_cause(json: &Value, prefix: &str) -> String {
    if json.get("error").is_some() {
        return format!("{prefix}:error");
    }

    let entry_type = json.get("type").and_then(|v| v.as_str());
    if entry_type != Some("assistant") {
        if entry_type == Some("user") {
            return format!("{prefix}:user");
        }
        return format!("{prefix}:working");
    }

    let Some(content) =
        json.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_array())
    else {
        return format!("{prefix}:idle");
    };

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str());
        match block_type {
            Some("tool_use") => {
                let tool = block.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                return format!("{prefix}:tool({tool})");
            }
            Some("thinking") => return format!("{prefix}:thinking"),
            _ => {}
        }
    }

    format!("{prefix}:idle")
}

/// Extract the concatenated text content from an assistant JSONL entry.
///
/// Returns `None` for non-assistant entries (caller must NOT clear existing value)
/// or assistant messages with no `type: "text"` blocks.
pub fn extract_assistant_text(json: &Value) -> Option<String> {
    if json.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let content = json.get("message")?.get("content")?.as_array()?;
    let texts: Vec<&str> = content
        .iter()
        .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("text"))
        .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
        .collect();
    if texts.is_empty() {
        return None;
    }
    let joined = texts.join("\n");
    let trimmed = joined.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Parse a Claude session log JSONL entry into an [`AgentState`].
///
/// Returns `None` if the entry cannot be meaningfully classified (e.g.
/// missing required fields on an assistant message).
pub fn parse_claude_state(json: &Value) -> Option<AgentState> {
    // Error field takes priority
    if let Some(error) = json.get("error") {
        return Some(AgentState::Error { detail: error.as_str().unwrap_or("unknown").to_string() });
    }

    let entry_type = json.get("type").and_then(|v| v.as_str());

    // Detect user interrupts: Escape during response writes text containing
    // "[Request interrupted by user]"; rejecting a tool use writes an entry
    // with `toolUseResult: "User rejected tool use"`. Both are turn
    // boundaries — the agent is idle. Other user messages → working.
    if entry_type == Some("user") {
        let is_interrupt = json
            .get("toolUseResult")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s == "User rejected tool use")
            || json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b.get("type").and_then(|v| v.as_str()) == Some("text")
                            && b.get("text")
                                .and_then(|v| v.as_str())
                                .is_some_and(|t| t.contains("[Request interrupted by user]"))
                    })
                });
        return if is_interrupt { Some(AgentState::Idle) } else { Some(AgentState::Working) };
    }

    // Only assistant messages carry meaningful state transitions.
    // Other types (progress, system, file-history-snapshot, etc.) are
    // session metadata — not agent state signals. Emitting Working for
    // these would let Tier 2 spuriously escalate over a Tier 1 Idle
    // (e.g. a stop_hook_summary arriving after the Stop hook FIFO event).
    if entry_type != Some("assistant") {
        return None;
    }

    let content = json.get("message")?.get("content")?.as_array()?;

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str());
        match block_type {
            Some("tool_use") => {
                let tool = block.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                return match tool {
                    "AskUserQuestion" => {
                        Some(AgentState::Prompt { prompt: extract_ask_user_context(block) })
                    }
                    _ => Some(AgentState::Working),
                };
            }
            Some("thinking") => return Some(AgentState::Working),
            _ => {}
        }
    }

    // Assistant message with no tool_use or thinking blocks — idle
    Some(AgentState::Idle)
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
