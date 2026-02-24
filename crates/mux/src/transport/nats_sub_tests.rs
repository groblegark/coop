// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::config::MuxConfig;
use crate::state::{MuxEvent, MuxState, SessionTransport};

fn test_config() -> MuxConfig {
    MuxConfig {
        host: "127.0.0.1".into(),
        port: 0,
        auth_token: None,
        screen_poll_ms: 500,
        status_poll_ms: 2000,
        health_check_ms: 10000,
        max_health_failures: 3,
        launch: None,
        credential_config: None,
        prewarm_capacity: 64,
        prewarm_poll_ms: 15000,
        state_dir: None,
        api_key_file: None,
        #[cfg(debug_assertions)]
        hot: false,
    }
}

fn test_state() -> Arc<MuxState> {
    Arc::new(MuxState::new(test_config(), CancellationToken::new()))
}

// ── handle_announce ───────────────────────────────────────────────────────

#[tokio::test]
async fn announce_online_registers_session() -> anyhow::Result<()> {
    let state = test_state();
    let mut last_announce = HashMap::new();

    let payload = serde_json::to_vec(&serde_json::json!({
        "event": "online",
        "session_id": "sess-1",
        "url": "http://127.0.0.1:9090",
        "agent": "claude",
        "ts": 1234567890
    }))?;

    super::handle_announce(&state, "coop.mux", "sess-1", &payload, &mut last_announce).await;

    let sessions = state.sessions.read().await;
    assert!(sessions.contains_key("sess-1"), "session should be registered");
    let entry = &sessions["sess-1"];
    assert_eq!(entry.url, "http://127.0.0.1:9090");
    assert!(matches!(entry.transport, SessionTransport::Nats { .. }));
    if let SessionTransport::Nats { ref prefix } = entry.transport {
        assert_eq!(prefix, "coop.mux");
    }
    assert!(last_announce.contains_key("sess-1"));
    Ok(())
}

#[tokio::test]
async fn announce_heartbeat_updates_timestamp() -> anyhow::Result<()> {
    let state = test_state();
    let mut last_announce = HashMap::new();

    // First online.
    let payload = serde_json::to_vec(&serde_json::json!({
        "event": "online",
        "session_id": "sess-1",
        "url": "http://127.0.0.1:9090",
        "ts": 1000
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &payload, &mut last_announce).await;
    let t1 = last_announce["sess-1"];

    // Small delay so timestamps differ.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Heartbeat — should NOT create a duplicate entry.
    let payload = serde_json::to_vec(&serde_json::json!({
        "event": "heartbeat",
        "session_id": "sess-1",
        "ts": 2000
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &payload, &mut last_announce).await;
    let t2 = last_announce["sess-1"];

    assert!(t2 >= t1, "heartbeat should update timestamp");
    let sessions = state.sessions.read().await;
    assert_eq!(sessions.len(), 1, "should still be one session");
    Ok(())
}

#[tokio::test]
async fn announce_offline_removes_nats_session() -> anyhow::Result<()> {
    let state = test_state();
    let mut last_announce = HashMap::new();

    // Register.
    let online = serde_json::to_vec(&serde_json::json!({
        "event": "online",
        "session_id": "sess-1",
        "url": "http://127.0.0.1:9090",
        "ts": 1000
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &online, &mut last_announce).await;

    // Offline.
    let offline = serde_json::to_vec(&serde_json::json!({
        "event": "offline",
        "session_id": "sess-1",
        "ts": 2000
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &offline, &mut last_announce).await;

    let sessions = state.sessions.read().await;
    assert!(!sessions.contains_key("sess-1"), "session should be removed");
    assert!(!last_announce.contains_key("sess-1"));
    Ok(())
}

#[tokio::test]
async fn announce_invalid_json_is_ignored() -> anyhow::Result<()> {
    let state = test_state();
    let mut last_announce = HashMap::new();

    super::handle_announce(&state, "coop.mux", "sess-1", b"bad json", &mut last_announce).await;

    let sessions = state.sessions.read().await;
    assert!(sessions.is_empty());
    Ok(())
}

#[tokio::test]
async fn announce_online_emits_session_online_event() -> anyhow::Result<()> {
    let state = test_state();
    let mut event_rx = state.feed.event_tx.subscribe();
    let mut last_announce = HashMap::new();

    let payload = serde_json::to_vec(&serde_json::json!({
        "event": "online",
        "session_id": "sess-1",
        "url": "http://127.0.0.1:9090",
        "ts": 1234
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &payload, &mut last_announce).await;

    let event = event_rx.try_recv()?;
    match event {
        MuxEvent::SessionOnline { session, url, .. } => {
            assert_eq!(session, "sess-1");
            assert_eq!(url, "http://127.0.0.1:9090");
        }
        other => anyhow::bail!("expected SessionOnline, got {other:?}"),
    }
    Ok(())
}

// ── handle_status ─────────────────────────────────────────────────────────

#[tokio::test]
async fn status_updates_cached_status() -> anyhow::Result<()> {
    let state = test_state();
    let mut last_announce = HashMap::new();

    // Register a session first.
    let online = serde_json::to_vec(&serde_json::json!({
        "event": "online",
        "session_id": "sess-1",
        "url": "http://127.0.0.1:9090",
        "ts": 1000
    }))?;
    super::handle_announce(&state, "coop.mux", "sess-1", &online, &mut last_announce).await;

    // Send status update.
    let status_payload = serde_json::to_vec(&serde_json::json!({
        "session_id": "sess-1",
        "state": "working",
        "pid": 1234,
        "uptime_secs": 42,
        "exit_code": null,
        "screen_seq": 10,
        "bytes_read": 1024,
        "bytes_written": 512,
        "ws_clients": 0,
        "fetched_at": 9999
    }))?;
    super::handle_status(&state, "sess-1", &status_payload).await;

    let sessions = state.sessions.read().await;
    let entry = &sessions["sess-1"];
    let cached = entry.cached_status.read().await;
    let Some(status) = cached.as_ref() else {
        anyhow::bail!("status should be cached");
    };
    assert_eq!(status.state, "working");
    assert_eq!(status.pid, Some(1234));
    assert_eq!(status.uptime_secs, 42);
    assert_eq!(status.screen_seq, 10);
    Ok(())
}

#[tokio::test]
async fn status_for_unknown_session_is_ignored() -> anyhow::Result<()> {
    let state = test_state();

    let status_payload = serde_json::to_vec(&serde_json::json!({
        "session_id": "nonexistent",
        "state": "idle",
        "pid": null,
        "uptime_secs": 0,
        "exit_code": null,
        "screen_seq": 0,
        "bytes_read": 0,
        "bytes_written": 0,
        "ws_clients": 0,
        "fetched_at": 0
    }))?;
    // Should not panic.
    super::handle_status(&state, "nonexistent", &status_payload).await;
    Ok(())
}

#[tokio::test]
async fn status_invalid_json_is_ignored() -> anyhow::Result<()> {
    let state = test_state();
    // Should not panic.
    super::handle_status(&state, "sess-1", b"not json").await;
    Ok(())
}

// ── handle_state ──────────────────────────────────────────────────────────

#[tokio::test]
async fn state_emits_transition_event() -> anyhow::Result<()> {
    let state = test_state();
    let mut event_rx = state.feed.event_tx.subscribe();

    let state_payload = serde_json::to_vec(&serde_json::json!({
        "prev": "idle",
        "next": "working",
        "seq": 5,
        "cause": "hook",
        "last_message": "doing stuff"
    }))?;
    super::handle_state(&state, "sess-1", &state_payload).await;

    let event = event_rx.try_recv()?;
    match event {
        MuxEvent::Transition { session, prev, next, seq, cause, last_message, .. } => {
            assert_eq!(session, "sess-1");
            assert_eq!(prev, "idle");
            assert_eq!(next, "working");
            assert_eq!(seq, 5);
            assert_eq!(cause, "hook");
            assert_eq!(last_message.as_deref(), Some("doing stuff"));
        }
        other => anyhow::bail!("expected Transition, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn state_with_minimal_fields() -> anyhow::Result<()> {
    let state = test_state();
    let mut event_rx = state.feed.event_tx.subscribe();

    // Only required fields — all optional fields default.
    let state_payload = serde_json::to_vec(&serde_json::json!({
        "prev": "starting",
        "next": "idle"
    }))?;
    super::handle_state(&state, "sess-2", &state_payload).await;

    let event = event_rx.try_recv()?;
    match event {
        MuxEvent::Transition { session, prev, next, seq, cause, .. } => {
            assert_eq!(session, "sess-2");
            assert_eq!(prev, "starting");
            assert_eq!(next, "idle");
            assert_eq!(seq, 0);
            assert!(cause.is_empty());
        }
        other => anyhow::bail!("expected Transition, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn state_invalid_json_is_ignored() -> anyhow::Result<()> {
    let state = test_state();
    let mut event_rx = state.feed.event_tx.subscribe();

    super::handle_state(&state, "sess-1", b"bad json").await;

    // No event should be emitted.
    assert!(event_rx.try_recv().is_err());
    Ok(())
}
