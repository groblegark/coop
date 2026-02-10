// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::parse_nats_payload;
use crate::driver::HookEvent;

#[test]
fn parse_post_tool_use() {
    let payload = br#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::ToolAfter { tool: "Bash".into() });
}

#[test]
fn parse_stop() {
    let payload = br#"{"hook_event_name":"Stop"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::TurnEnd);
}

#[test]
fn parse_session_end() {
    let payload = br#"{"hook_event_name":"SessionEnd"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::SessionEnd);
}

#[test]
fn parse_session_start() {
    let payload = br#"{"hook_event_name":"SessionStart"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::SessionStart);
}

#[test]
fn parse_user_prompt_submit() {
    let payload = br#"{"hook_event_name":"UserPromptSubmit"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::TurnStart);
}

#[test]
fn parse_notification() {
    let payload = br#"{"hook_event_name":"Notification","notification_type":"idle_prompt"}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    assert_eq!(event, HookEvent::Notification { notification_type: "idle_prompt".into() });
}

#[test]
fn parse_pre_tool_use() {
    let payload = br#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"question":"What?"}}"#;
    let (event, _json) = parse_nats_payload(payload).unwrap();
    match event {
        HookEvent::ToolBefore { tool, tool_input } => {
            assert_eq!(tool, "AskUserQuestion");
            assert!(tool_input.is_some());
        }
        _ => panic!("expected ToolBefore"),
    }
}

#[test]
fn parse_unknown_event_returns_none() {
    let payload = br#"{"hook_event_name":"PreCompact"}"#;
    assert!(parse_nats_payload(payload).is_none());
}

#[test]
fn parse_invalid_json_returns_none() {
    assert!(parse_nats_payload(b"not json").is_none());
}

#[test]
fn parse_notification_without_type_returns_none() {
    let payload = br#"{"hook_event_name":"Notification"}"#;
    assert!(parse_nats_payload(payload).is_none());
}

#[test]
fn parse_pre_tool_use_without_tool_returns_none() {
    let payload = br#"{"hook_event_name":"PreToolUse"}"#;
    assert!(parse_nats_payload(payload).is_none());
}
