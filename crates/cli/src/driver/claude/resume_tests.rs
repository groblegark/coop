// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::io::Write;

use super::{discover_session_log, parse_resume_state, resume_args};
use crate::driver::AgentState;

#[test]
fn discover_returns_none_for_nonexistent() -> anyhow::Result<()> {
    let result = discover_session_log("/nonexistent/path")?;
    assert!(result.is_none());
    Ok(())
}

#[test]
fn discover_returns_direct_jsonl_path() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(&log_path, "{}\n")?;

    let result = discover_session_log(log_path.to_str().ok_or(anyhow::anyhow!("path"))?)?;
    assert_eq!(result, Some(log_path));
    Ok(())
}

#[test]
fn parse_empty_log_returns_starting() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(&log_path, "")?;

    let state = parse_resume_state(&log_path)?;
    assert_eq!(state.last_state, AgentState::Starting);
    assert_eq!(state.log_offset, 0);
    Ok(())
}

#[test]
fn parse_log_with_assistant_message() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    let mut f = std::fs::File::create(&log_path)?;
    // Write a session entry with an assistant message (should yield Idle)
    writeln!(
        f,
        r#"{{"sessionId":"abc-123","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"Hello"}}]}}}}"#
    )?;

    let state = parse_resume_state(&log_path)?;
    assert_eq!(state.last_state, AgentState::Idle);
    assert!(state.log_offset > 0);
    Ok(())
}

#[test]
fn resume_args_with_session_id() {
    let args = resume_args("sess-42");
    assert_eq!(args, vec!["--resume", "sess-42"]);
}
