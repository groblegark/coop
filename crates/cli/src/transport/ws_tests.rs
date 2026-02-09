// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use crate::driver::AgentState;
use crate::test_support::{AnyhowExt, AppStateBuilder, StubNudgeEncoder};
use crate::transport::ws::{handle_client_message, ClientMessage, ServerMessage, SubscriptionMode};

#[test]
fn ping_pong_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Ping {};
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"ping\""));

    let pong = ServerMessage::Pong {};
    let json = serde_json::to_string(&pong).anyhow()?;
    assert!(json.contains("\"type\":\"pong\""));
    Ok(())
}

#[test]
fn screen_request_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::ScreenRequest {};
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"screen_request\""));
    Ok(())
}

#[test]
fn output_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Output { data: "aGVsbG8=".to_owned(), offset: 0 };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"output\""));
    assert!(json.contains("\"data\":\"aGVsbG8=\""));
    Ok(())
}

#[test]
fn state_change_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::StateChange {
        prev: "working".to_owned(),
        next: "waiting_for_input".to_owned(),
        seq: 42,
        prompt: Box::new(None),
        error_detail: None,
        error_category: None,
        cause: String::new(),
    };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"state_change\""));
    assert!(json.contains("\"prev\":\"working\""));
    assert!(json.contains("\"next\":\"waiting_for_input\""));
    // Error fields should be absent (skip_serializing_if = None)
    assert!(!json.contains("error_detail"), "json: {json}");
    assert!(!json.contains("error_category"), "json: {json}");
    // Cause should be absent when empty (skip_serializing_if)
    assert!(!json.contains("cause"), "json: {json}");
    Ok(())
}

#[test]
fn state_change_with_error_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::StateChange {
        prev: "working".to_owned(),
        next: "error".to_owned(),
        seq: 5,
        prompt: Box::new(None),
        error_detail: Some("rate_limit_error".to_owned()),
        error_category: Some("rate_limited".to_owned()),
        cause: "log:error".to_owned(),
    };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"state_change\""));
    assert!(json.contains("\"next\":\"error\""));
    assert!(json.contains("\"error_detail\":\"rate_limit_error\""), "json: {json}");
    assert!(json.contains("\"error_category\":\"rate_limited\""), "json: {json}");
    Ok(())
}

#[test]
fn subscription_mode_default_is_all() -> anyhow::Result<()> {
    let mode: SubscriptionMode = serde_json::from_str("\"all\"").anyhow()?;
    assert_eq!(mode, SubscriptionMode::All);
    assert_eq!(SubscriptionMode::default(), SubscriptionMode::All);
    Ok(())
}

#[test]
fn subscription_modes_deserialize() -> anyhow::Result<()> {
    let raw: SubscriptionMode = serde_json::from_str("\"raw\"").anyhow()?;
    assert_eq!(raw, SubscriptionMode::Raw);

    let screen: SubscriptionMode = serde_json::from_str("\"screen\"").anyhow()?;
    assert_eq!(screen, SubscriptionMode::Screen);

    let state: SubscriptionMode = serde_json::from_str("\"state\"").anyhow()?;
    assert_eq!(state, SubscriptionMode::State);
    Ok(())
}

#[test]
fn error_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Error {
        code: "BAD_REQUEST".to_owned(),
        message: "invalid input".to_owned(),
    };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"error\""));
    assert!(json.contains("\"code\":\"BAD_REQUEST\""));
    Ok(())
}

#[test]
fn exit_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Exit { code: Some(0), signal: None };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"exit\""));
    assert!(json.contains("\"code\":0"));
    Ok(())
}

#[test]
fn replay_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Replay { offset: 1024 };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"replay\""));
    assert!(json.contains("\"offset\":1024"));
    Ok(())
}

#[test]
fn auth_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Auth { token: "secret123".to_owned() };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"auth\""));
    assert!(json.contains("\"token\":\"secret123\""));
    Ok(())
}

#[test]
fn client_message_roundtrip() -> anyhow::Result<()> {
    let messages = vec![
        r#"{"type":"input","text":"hello"}"#,
        r#"{"type":"input_raw","data":"aGVsbG8="}"#,
        r#"{"type":"keys","keys":["Enter"]}"#,
        r#"{"type":"resize","cols":200,"rows":50}"#,
        r#"{"type":"screen_request"}"#,
        r#"{"type":"state_request"}"#,
        r#"{"type":"nudge","message":"fix bug"}"#,
        r#"{"type":"respond","accept":true}"#,
        r#"{"type":"replay","offset":0}"#,
        r#"{"type":"auth","token":"tok"}"#,
        r#"{"type":"signal","signal":"SIGINT"}"#,
        r#"{"type":"shutdown"}"#,
        r#"{"type":"ping"}"#,
    ];

    for json in messages {
        let _msg: ClientMessage = serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("failed to parse '{json}': {e}"))?;
    }
    Ok(())
}

#[test]
fn shutdown_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Shutdown {};
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"shutdown\""));

    // Roundtrip
    let _: ClientMessage = serde_json::from_str(r#"{"type":"shutdown"}"#)
        .map_err(|e| anyhow::anyhow!("failed to parse shutdown: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Integration tests using handle_client_message
// ---------------------------------------------------------------------------

fn ws_test_state(
    agent: AgentState,
) -> (Arc<crate::transport::state::AppState>, tokio::sync::mpsc::Receiver<crate::event::InputEvent>)
{
    AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(agent)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build()
}

#[tokio::test]
async fn state_request_returns_error_fields() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Error { detail: "authentication_error".to_owned() })
        .build();

    let msg = ClientMessage::StateRequest {};
    let reply = handle_client_message(&state, msg, "test-client", &mut true).await;
    match reply {
        Some(ServerMessage::StateChange { next, error_detail, error_category, .. }) => {
            assert_eq!(next, "error");
            assert_eq!(error_detail.as_deref(), Some("authentication_error"));
            assert_eq!(error_category.as_deref(), Some("unauthorized"));
        }
        other => anyhow::bail!("expected StateChange, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn resize_zero_cols_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let msg = ClientMessage::Resize { cols: 0, rows: 24 };
    let reply = handle_client_message(&state, msg, "test-client", &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "BAD_REQUEST");
        }
        other => anyhow::bail!("expected Error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn resize_zero_rows_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let msg = ClientMessage::Resize { cols: 80, rows: 0 };
    let reply = handle_client_message(&state, msg, "test-client", &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "BAD_REQUEST");
        }
        other => anyhow::bail!("expected Error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn nudge_rejected_when_agent_working() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    state.ready.store(true, std::sync::atomic::Ordering::Release);
    let client_id = "test-ws";

    let msg = ClientMessage::Nudge { message: "hello".to_owned() };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "AGENT_BUSY");
        }
        other => anyhow::bail!("expected AgentBusy error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn nudge_accepted_when_agent_waiting() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::WaitingForInput);
    state.ready.store(true, std::sync::atomic::Ordering::Release);
    let client_id = "test-ws";

    let msg = ClientMessage::Nudge { message: "hello".to_owned() };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    assert!(reply.is_none(), "expected None (success), got {reply:?}");
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_token() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    assert!(!state.lifecycle.shutdown.is_cancelled());

    let msg = ClientMessage::Shutdown {};
    let reply = handle_client_message(&state, msg, "test-ws", &mut true).await;
    assert!(reply.is_none(), "expected None (success), got {reply:?}");
    assert!(state.lifecycle.shutdown.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn shutdown_requires_auth() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);

    let msg = ClientMessage::Shutdown {};
    let reply = handle_client_message(&state, msg, "test-ws", &mut false).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "UNAUTHORIZED");
        }
        other => anyhow::bail!("expected Unauthorized error, got {other:?}"),
    }
    assert!(!state.lifecycle.shutdown.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn signal_delivers_sigint() -> anyhow::Result<()> {
    let (state, mut rx) = ws_test_state(AgentState::Working);
    let client_id = "test-ws";

    let msg = ClientMessage::Signal { signal: "SIGINT".to_owned() };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    assert!(reply.is_none(), "expected None (success), got {reply:?}");

    let event = rx.recv().await;
    assert!(
        matches!(event, Some(crate::event::InputEvent::Signal(crate::event::PtySignal::Int))),
        "expected Signal(Int), got {event:?}"
    );
    Ok(())
}

#[tokio::test]
async fn signal_rejects_unknown() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let client_id = "test-ws";

    let msg = ClientMessage::Signal { signal: "SIGFOO".to_owned() };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "BAD_REQUEST");
        }
        other => anyhow::bail!("expected BadRequest error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn keys_rejects_unknown_key() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let client_id = "test-ws";

    let msg = ClientMessage::Keys { keys: vec!["Enter".to_owned(), "SuperKey".to_owned()] };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, message }) => {
            assert_eq!(code, "BAD_REQUEST");
            assert!(message.contains("SuperKey"), "message should mention the bad key: {message}");
        }
        other => anyhow::bail!("expected BadRequest error, got {other:?}"),
    }
    Ok(())
}

#[test]
fn signal_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Signal { signal: "SIGTERM".to_owned() };
    let json = serde_json::to_string(&msg).anyhow()?;
    assert!(json.contains("\"type\":\"signal\""));
    assert!(json.contains("\"signal\":\"SIGTERM\""));
    Ok(())
}
