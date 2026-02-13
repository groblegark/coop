// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::record::RecordingState;
use crate::screen::{CursorPosition, ScreenSnapshot};

fn test_snapshot() -> ScreenSnapshot {
    ScreenSnapshot {
        lines: vec!["hello".to_owned()],
        ansi: vec![],
        cols: 80,
        rows: 24,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 5 },
        sequence: 1,
    }
}

#[tokio::test]
async fn enable_disable_toggle() -> anyhow::Result<()> {
    let state = RecordingState::new(None, 80, 24);
    assert!(!state.is_enabled());
    state.enable().await;
    assert!(state.is_enabled());
    state.disable();
    assert!(!state.is_enabled());
    Ok(())
}

#[tokio::test]
async fn push_when_disabled_is_noop() -> anyhow::Result<()> {
    let state = RecordingState::new(None, 80, 24);
    state.push("state", serde_json::json!({}), &test_snapshot()).await;
    assert_eq!(state.status().entries, 0);
    Ok(())
}

#[tokio::test]
async fn push_increments_seq() -> anyhow::Result<()> {
    let state = RecordingState::new(None, 80, 24);
    state.enable().await;
    state
        .push("state", serde_json::json!({"prev":"Starting","next":"Working"}), &test_snapshot())
        .await;
    state.push("hook", serde_json::json!({"hook_seq":0}), &test_snapshot()).await;
    assert_eq!(state.status().entries, 2);
    Ok(())
}

#[tokio::test]
async fn file_write_and_catchup() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let state = RecordingState::new(Some(dir.path()), 80, 24);
    state.enable().await;

    let snap = test_snapshot();
    state.push("state", serde_json::json!({"prev":"Starting","next":"Working"}), &snap).await;
    state.push("hook", serde_json::json!({"hook_seq":0}), &snap).await;

    // Catchup from 0 returns both
    let entries = state.catchup(0);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].seq, 1);
    assert_eq!(entries[1].seq, 2);

    // Catchup from 1 returns only the second
    let entries = state.catchup(1);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].seq, 2);

    Ok(())
}

#[tokio::test]
async fn download_returns_file_contents() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let state = RecordingState::new(Some(dir.path()), 80, 24);
    state.enable().await;
    state.push("state", serde_json::json!({}), &test_snapshot()).await;

    let data = state.download();
    assert!(data.is_some());
    let text = String::from_utf8(data.ok_or_else(|| anyhow::anyhow!("no data"))?)?;
    let lines: Vec<&str> = text.lines().collect();
    // Header + 1 entry
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("\"version\":1"));
    assert!(lines[1].contains("\"seq\":1"));

    Ok(())
}

#[tokio::test]
async fn broadcast_sends_entries() -> anyhow::Result<()> {
    let state = RecordingState::new(None, 80, 24);
    let mut rx = state.record_tx.subscribe();
    state.enable().await;

    state.push("state", serde_json::json!({}), &test_snapshot()).await;

    let entry = rx.try_recv()?;
    assert_eq!(entry.seq, 1);
    assert_eq!(entry.kind, "state");

    Ok(())
}
