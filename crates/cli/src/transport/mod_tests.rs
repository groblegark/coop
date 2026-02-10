// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use super::{encode_response, resolve_permission_option, resolve_plan_option, spawn_enter_retry};
use crate::driver::claude::encoding::ClaudeRespondEncoder;
use crate::driver::{AgentState, PromptContext, PromptKind};
use crate::event::StateChangeEvent;

#[test]
fn permission_option_takes_precedence_over_accept() {
    assert_eq!(resolve_permission_option(Some(true), Some(2)), 2);
    assert_eq!(resolve_permission_option(Some(false), Some(1)), 1);
}

#[test]
fn permission_accept_true_maps_to_1() {
    assert_eq!(resolve_permission_option(Some(true), None), 1);
}

#[test]
fn permission_accept_false_maps_to_3() {
    assert_eq!(resolve_permission_option(Some(false), None), 3);
}

#[test]
fn permission_both_none_defaults_to_3() {
    assert_eq!(resolve_permission_option(None, None), 3);
}

#[test]
fn plan_option_takes_precedence_over_accept() {
    assert_eq!(resolve_plan_option(Some(true), Some(3)), 3);
    assert_eq!(resolve_plan_option(Some(false), Some(1)), 1);
}

#[test]
fn plan_accept_true_maps_to_2() {
    assert_eq!(resolve_plan_option(Some(true), None), 2);
}

#[test]
fn plan_accept_false_maps_to_4() {
    assert_eq!(resolve_plan_option(Some(false), None), 4);
}

#[test]
fn plan_both_none_defaults_to_4() {
    assert_eq!(resolve_plan_option(None, None), 4);
}

fn fallback_prompt(kind: PromptKind) -> PromptContext {
    PromptContext::new(kind)
        .with_options(vec!["Accept".to_string(), "Cancel".to_string()])
        .with_options_fallback()
        .with_ready()
}

#[yare::parameterized(
    perm_accept_true = { PromptKind::Permission, Some(true), None, b"\r" as &[u8] },
    perm_accept_false = { PromptKind::Permission, Some(false), None, b"\x1b" },
    perm_option_1 = { PromptKind::Permission, None, Some(1), b"\r" },
    perm_option_2 = { PromptKind::Permission, None, Some(2), b"\x1b" },
    perm_none_defaults_esc = { PromptKind::Permission, None, None, b"\x1b" },
    plan_accept_true = { PromptKind::Plan, Some(true), None, b"\r" },
    plan_accept_false = { PromptKind::Plan, Some(false), None, b"\x1b" },
    plan_option_1 = { PromptKind::Plan, None, Some(1), b"\r" },
    plan_option_2 = { PromptKind::Plan, None, Some(2), b"\x1b" },
)]
fn fallback_encoding(kind: PromptKind, accept: Option<bool>, option: Option<u32>, expected: &[u8]) {
    let encoder = ClaudeRespondEncoder::default();
    let state = AgentState::Prompt { prompt: fallback_prompt(kind) };
    let (steps, _) = encode_response(&state, &encoder, accept, option, None, &[]).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, expected);
}

#[test]
fn non_fallback_permission_uses_encoder() {
    let encoder = ClaudeRespondEncoder::default();
    let prompt = PromptContext::new(PromptKind::Permission)
        .with_options(vec!["Yes".to_string(), "No".to_string()])
        .with_ready();
    let state = AgentState::Prompt { prompt };
    let (steps, _) = encode_response(&state, &encoder, Some(true), None, None, &[]).unwrap();
    // Non-fallback should use the encoder's digit format (e.g. "1\r")
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"1\r");
}

#[test]
fn setup_prompt_defaults_to_option_1() {
    let encoder = ClaudeRespondEncoder::default();
    let prompt = PromptContext::new(PromptKind::Setup)
        .with_subtype("theme_picker")
        .with_options(vec!["Dark mode".to_string(), "Light mode".to_string()])
        .with_ready();
    let state = AgentState::Prompt { prompt };
    let (steps, count) = encode_response(&state, &encoder, None, None, None, &[]).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].bytes, b"1");
    assert_eq!(steps[1].bytes, b"\r");
    assert_eq!(count, 0);
}

#[test]
fn setup_prompt_respects_explicit_option() {
    let encoder = ClaudeRespondEncoder::default();
    let prompt = PromptContext::new(PromptKind::Setup)
        .with_subtype("theme_picker")
        .with_options(vec!["Dark mode".to_string(), "Light mode".to_string()])
        .with_ready();
    let state = AgentState::Prompt { prompt };
    let (steps, _) = encode_response(&state, &encoder, None, Some(2), None, &[]).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].bytes, b"2");
    assert_eq!(steps[1].bytes, b"\r");
}

#[tokio::test]
async fn enter_retry_sends_cr_on_timeout() -> anyhow::Result<()> {
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(16);
    let (state_tx, _) = tokio::sync::broadcast::channel::<StateChangeEvent>(16);
    let state_rx = state_tx.subscribe();
    let activity = Arc::new(tokio::sync::Notify::new());

    let _cancel =
        spawn_enter_retry(input_tx, state_rx, activity, std::time::Duration::from_millis(50));

    // Wait for the retry to fire
    let event =
        tokio::time::timeout(std::time::Duration::from_millis(200), input_rx.recv()).await?;

    match event {
        Some(crate::event::InputEvent::Write(data)) => {
            assert_eq!(&data[..], b"\r");
        }
        other => panic!("expected Write(\\r), got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn enter_retry_cancelled_by_state_transition() -> anyhow::Result<()> {
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(16);
    let _keep_alive = input_tx.clone(); // keep channel open after task exits
    let (state_tx, _) = tokio::sync::broadcast::channel::<StateChangeEvent>(16);
    let state_rx = state_tx.subscribe();
    let activity = Arc::new(tokio::sync::Notify::new());

    let _cancel =
        spawn_enter_retry(input_tx, state_rx, activity, std::time::Duration::from_millis(200));

    // Send a Working state transition — should cancel the retry
    let _ = state_tx.send(StateChangeEvent {
        prev: AgentState::Idle,
        next: AgentState::Working,
        seq: 1,
        cause: "test".to_owned(),
        last_message: None,
    });

    // The retry should NOT fire
    let result = tokio::time::timeout(std::time::Duration::from_millis(300), input_rx.recv()).await;

    assert!(result.is_err(), "expected timeout (no retry), got {result:?}");
    Ok(())
}

#[tokio::test]
async fn enter_retry_cancelled_by_input_activity() -> anyhow::Result<()> {
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(16);
    let _keep_alive = input_tx.clone(); // keep channel open after task exits
    let (state_tx, _) = tokio::sync::broadcast::channel::<StateChangeEvent>(16);
    let state_rx = state_tx.subscribe();
    let activity = Arc::new(tokio::sync::Notify::new());

    let _cancel = spawn_enter_retry(
        input_tx,
        state_rx,
        Arc::clone(&activity),
        std::time::Duration::from_millis(200),
    );

    // Give the spawned task time to register the notified() future
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // Notify input activity — should cancel the retry
    activity.notify_waiters();

    // The retry should NOT fire
    let result = tokio::time::timeout(std::time::Duration::from_millis(300), input_rx.recv()).await;

    assert!(result.is_err(), "expected timeout (no retry), got {result:?}");
    Ok(())
}

#[tokio::test]
async fn enter_retry_cancelled_by_token() -> anyhow::Result<()> {
    let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(16);
    let _keep_alive = input_tx.clone(); // keep channel open after task exits
    let (state_tx, _) = tokio::sync::broadcast::channel::<StateChangeEvent>(16);
    let state_rx = state_tx.subscribe();
    let activity = Arc::new(tokio::sync::Notify::new());

    let cancel =
        spawn_enter_retry(input_tx, state_rx, activity, std::time::Duration::from_millis(200));

    // Cancel via the token (simulates next DeliveryGate::acquire)
    cancel.cancel();

    // The retry should NOT fire
    let result = tokio::time::timeout(std::time::Duration::from_millis(300), input_rx.recv()).await;

    assert!(result.is_err(), "expected timeout (no retry), got {result:?}");
    Ok(())
}
