// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::{encode_response, resolve_permission_option, resolve_plan_option};
use crate::driver::claude::encoding::ClaudeRespondEncoder;
use crate::driver::{AgentState, PromptContext, PromptKind};

// ---------------------------------------------------------------------------
// resolve_permission_option
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// resolve_plan_option
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// encode_response: options_fallback
// ---------------------------------------------------------------------------

fn fallback_prompt(kind: PromptKind) -> PromptContext {
    PromptContext {
        kind,
        subtype: None,
        tool: None,
        input: None,
        auth_url: None,
        options: vec!["Accept".to_string(), "Cancel".to_string()],
        options_fallback: true,
        questions: vec![],
        question_current: 0,
        ready: true,
    }
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
    let prompt = PromptContext {
        kind: PromptKind::Permission,
        subtype: None,
        tool: None,
        input: None,
        auth_url: None,
        options: vec!["Yes".to_string(), "No".to_string()],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
        ready: true,
    };
    let state = AgentState::Prompt { prompt };
    let (steps, _) = encode_response(&state, &encoder, Some(true), None, None, &[]).unwrap();
    // Non-fallback should use the encoder's digit format (e.g. "1\r")
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"1\r");
}

// ---------------------------------------------------------------------------
// encode_response: Setup prompt
// ---------------------------------------------------------------------------

#[test]
fn setup_prompt_defaults_to_option_1() {
    let encoder = ClaudeRespondEncoder::default();
    let prompt = PromptContext {
        kind: PromptKind::Setup,
        subtype: Some("theme_picker".to_owned()),
        tool: None,
        input: None,
        auth_url: None,
        options: vec!["Dark mode".to_string(), "Light mode".to_string()],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
        ready: true,
    };
    let state = AgentState::Prompt { prompt };
    let (steps, count) = encode_response(&state, &encoder, None, None, None, &[]).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"1\r");
    assert_eq!(count, 0);
}

#[test]
fn setup_prompt_respects_explicit_option() {
    let encoder = ClaudeRespondEncoder::default();
    let prompt = PromptContext {
        kind: PromptKind::Setup,
        subtype: Some("theme_picker".to_owned()),
        tool: None,
        input: None,
        auth_url: None,
        options: vec!["Dark mode".to_string(), "Light mode".to_string()],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
        ready: true,
    };
    let state = AgentState::Prompt { prompt };
    let (steps, _) = encode_response(&state, &encoder, None, Some(2), None, &[]).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"2\r");
}
