// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, Detector};

#[tokio::test]
async fn stdout_detector_parses_gemini_stream_json() -> anyhow::Result<()> {
    let (bytes_tx, bytes_rx) = mpsc::channel(32);
    let detector = Box::new(super::new_stdout_detector(bytes_rx, None));
    assert_eq!(detector.tier(), 3);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // Send a tool_use event as raw bytes
    bytes_tx
        .send(Bytes::from(
            "{\"type\":\"tool_use\",\"tool_name\":\"Bash\",\"tool_id\":\"bash-1\",\"parameters\":{\"command\":\"ls\"},\"timestamp\":\"2025-10-10T12:00:00.000Z\"}\n",
        ))
        .await?;

    let state = tokio::time::timeout(std::time::Duration::from_secs(5), state_rx.recv()).await;

    shutdown.cancel();
    let _ = handle.await;

    match state {
        Ok(Some((AgentState::Working, _cause, _))) => {}
        other => anyhow::bail!("expected Working, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn stdout_detector_detects_result_as_idle() -> anyhow::Result<()> {
    let (bytes_tx, bytes_rx) = mpsc::channel(32);
    let detector = Box::new(super::new_stdout_detector(bytes_rx, None));

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    bytes_tx
        .send(Bytes::from(
            "{\"type\":\"result\",\"status\":\"success\",\"timestamp\":\"2025-10-10T12:00:00.000Z\"}\n",
        ))
        .await?;

    let state = tokio::time::timeout(std::time::Duration::from_secs(5), state_rx.recv()).await;

    shutdown.cancel();
    let _ = handle.await;

    match state {
        Ok(Some((AgentState::Idle, _cause, _))) => {}
        other => anyhow::bail!("expected Idle, got {other:?}"),
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
    let expected_count = events_owned.len();
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
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some((state, _cause, _)) = state_rx.recv().await {
            states.push(state);
            if states.len() >= expected_count {
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
async fn hook_detector_before_agent() -> anyhow::Result<()> {
    let states =
        run_hook_detector(vec![r#"{"event":"before_agent","data":{"prompt":"Fix the bug"}}"#])
            .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Working));
    Ok(())
}

#[tokio::test]
async fn hook_detector_after_tool() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"after_tool","data":{"tool_name":"Bash","tool_input":{"command":"ls"},"tool_response":{"llmContent":"output"}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Working));
    Ok(())
}

#[tokio::test]
async fn hook_detector_session_end() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![r#"{"event":"session_end"}"#]).await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Idle));
    Ok(())
}

#[tokio::test]
async fn hook_detector_notification_tool_permission() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"notification","data":{"notification_type":"ToolPermission","message":"Allow Bash?","details":"command: rm -rf"}}"#,
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
async fn hook_detector_pre_tool_use_maps_to_working() -> anyhow::Result<()> {
    let states = run_hook_detector(vec![
        r#"{"event":"pre_tool_use","data":{"tool_name":"Bash","tool_input":{"command":"ls"}}}"#,
    ])
    .await?;

    assert_eq!(states.len(), 1);
    assert!(matches!(states[0], AgentState::Working));
    Ok(())
}
