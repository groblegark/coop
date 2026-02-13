// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, Detector};

use super::LogDetector;

/// Build a LogDetector with fast test-appropriate poll interval.
fn test_log_detector(log_path: std::path::PathBuf) -> Box<LogDetector> {
    Box::new(LogDetector {
        log_path,
        start_offset: 0,
        // Short poll for tests; production uses config.log_poll() (3s default).
        poll_interval: std::time::Duration::from_millis(50),
        last_message: None,
        raw_message_tx: None,
        usage: None,
    })
}

#[tokio::test]
async fn log_detector_parses_lines_and_emits_states() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");

    // Write data before starting the detector — the first poll tick reads immediately.
    // Use a user message (→ Working) followed by an assistant text-only (→ Idle).
    // Note: non-meaningful types like "system" and "progress" are intentionally
    // ignored by parse_claude_state to avoid spurious Tier 2 Working emissions.
    std::fs::write(
        &log_path,
        concat!(
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
        ),
    )?;

    let detector = test_log_detector(log_path);
    assert_eq!(detector.tier(), 2);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // Wait for states to arrive
    let mut states = Vec::new();
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some((state, _cause, _)) = state_rx.recv().await {
            states.push(state.clone());
            if matches!(state, AgentState::Idle) {
                break;
            }
        }
    })
    .await;

    shutdown.cancel();
    let _ = handle.await;

    // Should have received at least Working (user) and Idle (assistant text-only)
    assert!(timeout.is_ok(), "timed out waiting for states");
    assert!(states.iter().any(|s| matches!(s, AgentState::Working)));
    assert!(states.iter().any(|s| matches!(s, AgentState::Idle)));
    Ok(())
}

#[tokio::test]
async fn log_detector_skips_non_assistant_lines() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");

    // Write data before starting the detector.
    std::fs::write(&log_path, "{\"type\":\"user\",\"message\":{\"content\":[]}}\n")?;

    let detector = test_log_detector(log_path);
    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // User messages produce Working (not Idle)
    let timeout =
        tokio::time::timeout(std::time::Duration::from_secs(2), async { state_rx.recv().await })
            .await;

    shutdown.cancel();
    let _ = handle.await;

    if let Ok(Some((state, _cause, _))) = timeout {
        assert!(matches!(state, AgentState::Working));
    }
    Ok(())
}

#[tokio::test]
async fn stdout_detector_parses_jsonl_bytes() -> anyhow::Result<()> {
    let (bytes_tx, bytes_rx) = mpsc::channel(32);
    let detector = Box::new(super::new_stdout_detector(bytes_rx, None, None));
    assert_eq!(detector.tier(), 3);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // Send a JSONL line as raw bytes
    bytes_tx
        .send(Bytes::from(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{}}]}}\n",
        ))
        .await?;

    let state = tokio::time::timeout(std::time::Duration::from_secs(2), state_rx.recv()).await;

    shutdown.cancel();
    let _ = handle.await;

    match state {
        Ok(Some((AgentState::Working, _cause, _))) => {} // tool_use → Working
        other => anyhow::bail!("expected Working, got {other:?}"),
    }
    Ok(())
}

/// Helper: create a HookDetector with a named pipe, run it, and send events.
async fn run_hook_detector(events: Vec<&str>) -> anyhow::Result<Vec<AgentState>> {
    use crate::driver::hook_recv::HookReceiver;
    use tokio::io::AsyncWriteExt;

    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook.pipe");
    let receiver = HookReceiver::new(&pipe_path)?;
    let detector = Box::new(super::new_hook_detector(receiver, None));
    assert_eq!(detector.tier(), 1);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, sd).await;
    });

    // Write events from another task
    let pipe = pipe_path.clone();
    let events_owned: Vec<String> = events.iter().map(|s| s.to_string()).collect();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let mut file = match tokio::fs::OpenOptions::new().write(true).open(&pipe).await {
            Ok(f) => f,
            Err(_) => return,
        };
        for event in events_owned {
            let _ = file.write_all(event.as_bytes()).await;
            let _ = file.write_all(b"\n").await;
        }
    });

    let mut states = Vec::new();
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while let Some((state, _cause, _)) = state_rx.recv().await {
            states.push(state);
            if states.len() >= events.len() {
                break;
            }
        }
    })
    .await;

    shutdown.cancel();
    let _ = handle.await;

    if timeout.is_err() && states.is_empty() {
        anyhow::bail!("timed out waiting for states");
    }
    Ok(states)
}

#[tokio::test]
async fn hook_detector_notification_idle_prompt() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"notification","data":{"notification_type":"idle_prompt"}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Idle));
    Ok(())
}

#[tokio::test]
async fn hook_detector_notification_permission_prompt() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"notification","data":{"notification_type":"permission_prompt"}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Prompt { .. }));
    if let AgentState::Prompt { prompt } = &states[0] {
        assert_eq!(prompt.kind, crate::driver::PromptKind::Permission);
    }
    Ok(())
}

#[tokio::test]
async fn hook_detector_pre_tool_use_ask_user() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"pre_tool_use","data":{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which DB?","options":[{"label":"PostgreSQL"},{"label":"SQLite"}]}]}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    if let AgentState::Prompt { prompt } = &states[0] {
        assert_eq!(prompt.kind, crate::driver::PromptKind::Question);
        assert_eq!(prompt.questions.len(), 1);
        assert_eq!(prompt.questions[0].question, "Which DB?");
        assert_eq!(prompt.questions[0].options, vec!["PostgreSQL", "SQLite"]);
    } else {
        anyhow::bail!("expected Prompt(Question), got {:?}", states[0]);
    }
    Ok(())
}

#[tokio::test]
async fn hook_detector_pre_tool_use_exit_plan_mode() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"pre_tool_use","data":{"tool_name":"ExitPlanMode","tool_input":{}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Prompt { .. }));
    if let AgentState::Prompt { prompt } = &states[0] {
        assert_eq!(prompt.kind, crate::driver::PromptKind::Plan);
        assert!(prompt.ready, "hook-detected Plan prompt should be immediately ready");
    }
    Ok(())
}

#[tokio::test]
async fn hook_detector_pre_tool_use_enter_plan_mode() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"pre_tool_use","data":{"tool_name":"EnterPlanMode","tool_input":{}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Working));
    Ok(())
}

#[tokio::test]
async fn hook_detector_tool_complete() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"post_tool_use","data":{"tool_name":"Bash","tool_input":{"command":"ls"}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Working));
    Ok(())
}

#[tokio::test]
async fn hook_detector_stop() -> anyhow::Result<()> {
    let states =
        run_hook_detector(vec![r#"{"event":"stop","data":{"stop_hook_active":false}}"#]).await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Idle));
    Ok(())
}
