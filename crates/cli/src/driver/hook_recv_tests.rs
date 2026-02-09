// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::event::HookEvent;

use super::{parse_hook_line, HookReceiver};

#[test]
fn parses_tool_complete_event() {
    let event = parse_hook_line(
        r#"{"event":"post_tool_use","data":{"tool_name":"Bash","tool_input":{"command":"ls"}}}"#,
    );
    assert_eq!(event, Some(HookEvent::ToolComplete { tool: "Bash".to_string() }));
}

#[test]
fn parses_stop_event() {
    let event = parse_hook_line(
        r#"{"event":"stop","data":{"hook_event_name":"Stop","stop_hook_active":false}}"#,
    );
    assert_eq!(event, Some(HookEvent::AgentStop));
}

#[test]
fn parses_session_end_event() {
    let event = parse_hook_line(r#"{"event":"session_end"}"#);
    assert_eq!(event, Some(HookEvent::SessionEnd));
}

#[test]
fn parses_notification_idle_prompt() {
    let event =
        parse_hook_line(r#"{"event":"notification","data":{"notification_type":"idle_prompt"}}"#);
    assert_eq!(
        event,
        Some(HookEvent::Notification { notification_type: "idle_prompt".to_string() })
    );
}

#[test]
fn parses_notification_permission_prompt() {
    let event = parse_hook_line(
        r#"{"event":"notification","data":{"notification_type":"permission_prompt"}}"#,
    );
    assert_eq!(
        event,
        Some(HookEvent::Notification { notification_type: "permission_prompt".to_string() })
    );
}

#[test]
fn parses_pre_tool_use_ask_user() {
    let event = parse_hook_line(
        r#"{"event":"pre_tool_use","data":{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which DB?"}]}}}"#,
    );
    match event {
        Some(HookEvent::PreToolUse { tool, tool_input }) => {
            assert_eq!(tool, "AskUserQuestion");
            assert!(tool_input.is_some());
            let input = tool_input.unwrap();
            assert!(input.get("questions").is_some());
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

#[test]
fn parses_pre_tool_use_exit_plan() {
    let event = parse_hook_line(
        r#"{"event":"pre_tool_use","data":{"tool_name":"ExitPlanMode","tool_input":{}}}"#,
    );
    match event {
        Some(HookEvent::PreToolUse { tool, tool_input }) => {
            assert_eq!(tool, "ExitPlanMode");
            assert!(tool_input.is_some());
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

#[test]
fn parses_pre_tool_use_without_tool_input() {
    let event = parse_hook_line(r#"{"event":"pre_tool_use","data":{"tool_name":"EnterPlanMode"}}"#);
    match event {
        Some(HookEvent::PreToolUse { tool, tool_input }) => {
            assert_eq!(tool, "EnterPlanMode");
            assert!(tool_input.is_none());
        }
        other => panic!("expected PreToolUse, got {other:?}"),
    }
}

#[test]
fn notification_missing_type_returns_none() {
    let event = parse_hook_line(r#"{"event":"notification","data":{}}"#);
    assert_eq!(event, None);
}

#[test]
fn pre_tool_use_missing_tool_name_returns_none() {
    let event = parse_hook_line(r#"{"event":"pre_tool_use","data":{}}"#);
    assert_eq!(event, None);
}

#[test]
fn parses_after_tool_event() {
    let event = parse_hook_line(
        r#"{"event":"after_tool","data":{"tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"llmContent":"output"}}}"#,
    );
    assert_eq!(event, Some(HookEvent::ToolComplete { tool: "Bash".to_string() }));
}

#[test]
fn after_tool_missing_tool_name_returns_empty() {
    let event = parse_hook_line(r#"{"event":"after_tool","data":{}}"#);
    assert_eq!(event, Some(HookEvent::ToolComplete { tool: "".to_string() }));
}

#[test]
fn ignores_malformed_lines() {
    assert_eq!(parse_hook_line("not json"), None);
    assert_eq!(parse_hook_line("{}"), None);
    assert_eq!(parse_hook_line(r#"{"event":"unknown_event"}"#), None);
    assert_eq!(parse_hook_line(""), None);
}

#[test]
fn creates_pipe_and_cleans_up() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("test.pipe");

    {
        let recv = HookReceiver::new(&pipe_path)?;
        assert!(pipe_path.exists());
        assert_eq!(recv.pipe_path(), pipe_path);
    }
    // Drop should remove the pipe
    assert!(!pipe_path.exists());
    Ok(())
}

#[tokio::test]
async fn reads_event_from_pipe() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook.pipe");

    let mut recv = HookReceiver::new(&pipe_path)?;

    // Write to the pipe from another task
    let pipe = pipe_path.clone();
    tokio::spawn(async move {
        // Small delay to let the reader open first
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Open for write explicitly (tokio::fs::write uses create+truncate which
        // doesn't work on FIFOs)
        let mut file = match tokio::fs::OpenOptions::new().write(true).open(&pipe).await {
            Ok(f) => f,
            Err(_) => return,
        };
        use tokio::io::AsyncWriteExt;
        let _ = file.write_all(b"{\"event\":\"stop\",\"data\":{}}\n").await;
    });

    let event = recv.next_event().await;
    assert_eq!(event, Some(HookEvent::AgentStop));
    Ok(())
}
