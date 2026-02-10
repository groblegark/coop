// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use crate::driver::AgentState;

use super::parse_gemini_state;

#[yare::parameterized(
    init_event = {
        json!({"type": "init", "session_id": "abc123", "model": "gemini-2.0-flash-exp", "timestamp": "2025-10-10T12:00:00.000Z"}),
        Some(AgentState::Working)
    },
    user_message = {
        json!({"type": "message", "role": "user", "content": "List files", "timestamp": "2025-10-10T12:00:01.000Z"}),
        Some(AgentState::Working)
    },
    assistant_message = {
        json!({"type": "message", "role": "assistant", "content": "Here are the files", "delta": true, "timestamp": "2025-10-10T12:00:02.000Z"}),
        Some(AgentState::Working)
    },
    tool_use = {
        json!({"type": "tool_use", "tool_name": "Bash", "tool_id": "bash-123", "parameters": {"command": "ls"}, "timestamp": "2025-10-10T12:00:03.000Z"}),
        Some(AgentState::Working)
    },
    tool_result = {
        json!({"type": "tool_result", "tool_id": "bash-123", "status": "success", "output": "file1.txt", "timestamp": "2025-10-10T12:00:04.000Z"}),
        Some(AgentState::Working)
    },
    result_success = {
        json!({"type": "result", "status": "success", "stats": {"total_tokens": 250}, "timestamp": "2025-10-10T12:00:05.000Z"}),
        Some(AgentState::WaitingForInput)
    },
    result_error = {
        json!({"type": "result", "status": "error", "error": {"type": "api", "message": "quota exceeded"}, "timestamp": "2025-10-10T12:00:05.000Z"}),
        Some(AgentState::WaitingForInput)
    },
    error_event = {
        json!({"type": "error", "message": "rate limit exceeded", "timestamp": "2025-10-10T12:00:05.000Z"}),
        Some(AgentState::Error { detail: "rate limit exceeded".to_string() })
    },
    error_without_message = {
        json!({"type": "error", "timestamp": "2025-10-10T12:00:05.000Z"}),
        Some(AgentState::Error { detail: "unknown".to_string() })
    },
    unknown_type = {
        json!({"type": "custom_event"}),
        None
    },
    missing_type = {
        json!({"data": "something"}),
        None
    },
)]
fn state_from_stream_json(entry: serde_json::Value, expected: Option<AgentState>) {
    assert_eq!(parse_gemini_state(&entry), expected);
}
