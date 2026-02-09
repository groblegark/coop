// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::event::HookEvent;

use super::parse_nats_payload;

#[test]
fn parses_post_tool_use() {
    let payload =
        br#"{"hook_event_name":"PostToolUse","tool_name":"Bash","session_id":"abc","cwd":"/tmp"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(event, Some(HookEvent::ToolComplete { tool: "Bash".to_string() }));
}

#[test]
fn parses_stop() {
    let payload = br#"{"hook_event_name":"Stop","session_id":"abc","cwd":"/tmp"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(event, Some(HookEvent::AgentStop));
}

#[test]
fn parses_session_end() {
    let payload = br#"{"hook_event_name":"SessionEnd","session_id":"abc"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(event, Some(HookEvent::SessionEnd));
}

#[test]
fn parses_notification() {
    let payload =
        br#"{"hook_event_name":"Notification","notification_type":"idle_prompt","session_id":"abc"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(
        event,
        Some(HookEvent::Notification { notification_type: "idle_prompt".to_string() })
    );
}

#[test]
fn parses_notification_permission_prompt() {
    let payload =
        br#"{"hook_event_name":"Notification","notification_type":"permission_prompt","session_id":"abc"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(
        event,
        Some(HookEvent::Notification { notification_type: "permission_prompt".to_string() })
    );
}

#[test]
fn parses_pre_tool_use_with_input() {
    let payload = br#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which DB?"}]},"session_id":"abc"}"#;
    match parse_nats_payload(payload) {
        Some(HookEvent::PreToolUse { tool, tool_input }) => {
            assert_eq!(tool, "AskUserQuestion");
            assert!(tool_input.is_some());
            let input = tool_input.as_ref().and_then(|v| v.get("questions"));
            assert!(input.is_some());
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

#[test]
fn parses_pre_tool_use_exit_plan_mode() {
    let payload =
        br#"{"hook_event_name":"PreToolUse","tool_name":"ExitPlanMode","tool_input":{},"session_id":"abc"}"#;
    match parse_nats_payload(payload) {
        Some(HookEvent::PreToolUse { tool, .. }) => {
            assert_eq!(tool, "ExitPlanMode");
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

#[test]
fn notification_missing_type_returns_none() {
    let payload = br#"{"hook_event_name":"Notification","session_id":"abc"}"#;
    assert_eq!(parse_nats_payload(payload), None);
}

#[test]
fn pre_tool_use_missing_tool_name_returns_none() {
    let payload = br#"{"hook_event_name":"PreToolUse","session_id":"abc"}"#;
    assert_eq!(parse_nats_payload(payload), None);
}

#[test]
fn ignores_unknown_event_types() {
    let payload = br#"{"hook_event_name":"SessionStart","session_id":"abc"}"#;
    assert_eq!(parse_nats_payload(payload), None);

    let payload = br#"{"hook_event_name":"UserPromptSubmit","session_id":"abc"}"#;
    assert_eq!(parse_nats_payload(payload), None);

    let payload = br#"{"hook_event_name":"SubagentStart","session_id":"abc"}"#;
    assert_eq!(parse_nats_payload(payload), None);
}

#[test]
fn ignores_malformed_json() {
    assert_eq!(parse_nats_payload(b"not json"), None);
    assert_eq!(parse_nats_payload(b"{}"), None);
    assert_eq!(parse_nats_payload(b""), None);
}

#[test]
fn handles_extra_fields_gracefully() {
    // bd daemon includes cwd, transcript_path, permission_mode, published_at, model
    // — these should be silently ignored.
    let payload = br#"{"hook_event_name":"Stop","session_id":"abc","cwd":"/home/user","transcript_path":"/tmp/log.jsonl","permission_mode":"default","published_at":"2026-02-08T12:00:00Z","model":"claude-sonnet-4-5-20250929"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(event, Some(HookEvent::AgentStop));
}

#[test]
fn post_tool_use_missing_tool_name_uses_empty() {
    let payload = br#"{"hook_event_name":"PostToolUse","session_id":"abc"}"#;
    let event = parse_nats_payload(payload);
    assert_eq!(event, Some(HookEvent::ToolComplete { tool: String::new() }));
}
