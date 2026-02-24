// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;

use crate::driver::AgentState;
use crate::event::InputEvent;
use crate::test_support::{StoreBuilder, StoreCtx, StubNudgeEncoder, StubRespondEncoder};

/// Helper: call `handle_input` with a JSON payload and return the received InputEvent.
async fn send_input(payload: &[u8]) -> Option<InputEvent> {
    let StoreCtx { store, mut input_rx, .. } =
        StoreBuilder::new().child_pid(1234).build();
    super::handle_input(&store, payload).await;
    input_rx.try_recv().ok()
}

// ── handle_input ──────────────────────────────────────────────────────────

#[tokio::test]
async fn input_text_only() -> anyhow::Result<()> {
    let event = send_input(br#"{"text":"hello"}"#).await;
    match event {
        Some(InputEvent::Write(data)) => {
            assert_eq!(data, Bytes::from("hello"));
        }
        other => anyhow::bail!("expected Write, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn input_text_with_enter() -> anyhow::Result<()> {
    let event = send_input(br#"{"text":"yes","enter":true}"#).await;
    match event {
        Some(InputEvent::Write(data)) => {
            assert_eq!(data, Bytes::from("yes\r"));
        }
        other => anyhow::bail!("expected Write, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn input_empty_text_with_enter() -> anyhow::Result<()> {
    let event = send_input(br#"{"text":"","enter":true}"#).await;
    match event {
        Some(InputEvent::Write(data)) => {
            assert_eq!(data, Bytes::from("\r"));
        }
        other => anyhow::bail!("expected Write, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn input_invalid_json_is_ignored() -> anyhow::Result<()> {
    let event = send_input(b"not json").await;
    assert!(event.is_none(), "invalid JSON should not produce an InputEvent");
    Ok(())
}

#[tokio::test]
async fn input_missing_text_field_is_ignored() -> anyhow::Result<()> {
    let event = send_input(br#"{"enter":true}"#).await;
    assert!(event.is_none(), "missing 'text' field should not produce an InputEvent");
    Ok(())
}

// ── handle_nudge ──────────────────────────────────────────────────────────

#[tokio::test]
async fn nudge_with_message() -> anyhow::Result<()> {
    let StoreCtx { store, mut input_rx, .. } = StoreBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Idle)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    store.ready.store(true, Ordering::Release);

    super::handle_nudge(&store, br#"{"message":"please continue"}"#).await;

    // StubNudgeEncoder passes through message bytes unchanged.
    let event = input_rx.try_recv();
    assert!(event.is_ok(), "nudge should have sent an InputEvent");
    Ok(())
}

#[tokio::test]
async fn nudge_empty_message_defaults_to_continue() -> anyhow::Result<()> {
    let StoreCtx { store, mut input_rx, .. } = StoreBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Idle)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    store.ready.store(true, Ordering::Release);

    super::handle_nudge(&store, br#"{}"#).await;

    let event = input_rx.try_recv();
    assert!(event.is_ok(), "empty nudge should default to 'continue'");
    Ok(())
}

#[tokio::test]
async fn nudge_invalid_json_uses_default() -> anyhow::Result<()> {
    let StoreCtx { store, mut input_rx, .. } = StoreBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Idle)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    store.ready.store(true, Ordering::Release);

    super::handle_nudge(&store, b"bad json").await;

    let event = input_rx.try_recv();
    assert!(event.is_ok(), "invalid JSON nudge should still send 'continue'");
    Ok(())
}

// ── handle_respond ────────────────────────────────────────────────────────

#[tokio::test]
async fn respond_invalid_json_is_ignored() -> anyhow::Result<()> {
    let StoreCtx { store, mut input_rx, .. } = StoreBuilder::new()
        .child_pid(1234)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build();
    store.ready.store(true, Ordering::Release);

    super::handle_respond(&store, b"not json").await;

    let event = input_rx.try_recv();
    assert!(event.is_err(), "invalid JSON respond should produce no InputEvent");
    Ok(())
}

#[tokio::test]
async fn respond_valid_json_parses_fields() -> anyhow::Result<()> {
    // With no prompt state, respond will fail at the encoder level (no active prompt),
    // but the deserialization path should succeed.
    let StoreCtx { store, .. } = StoreBuilder::new()
        .child_pid(1234)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build();
    store.ready.store(true, Ordering::Release);

    // This should not panic — it should gracefully handle the "not in prompt" case.
    super::handle_respond(
        &store,
        br#"{"accept":true,"option":1,"text":"ok","answers":[]}"#,
    )
    .await;
    Ok(())
}

// ── epoch_ms ──────────────────────────────────────────────────────────────

#[test]
fn epoch_ms_is_reasonable() {
    let ms = super::epoch_ms();
    // Sanity: should be after 2025-01-01 and before 2100-01-01.
    assert!(ms > 1_735_689_600_000, "epoch_ms too small: {ms}");
    assert!(ms < 4_102_444_800_000, "epoch_ms too large: {ms}");
}
