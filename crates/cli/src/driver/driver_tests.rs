// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::{disruption_option, PromptContext, PromptKind};

fn prompt(kind: PromptKind, subtype: Option<&str>) -> PromptContext {
    PromptContext {
        kind,
        subtype: subtype.map(String::from),
        tool: None,
        input: None,
        auth_url: None,
        options: vec![],
        options_fallback: false,
        questions: vec![],
        question_current: 0,
        ready: true,
    }
}

// -- Disruptions (should return Some(option)) --

#[test]
fn disruption_security_notes() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("security_notes"))), Some(1));
}

#[test]
fn disruption_login_success() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("login_success"))), Some(1));
}

#[test]
fn disruption_terminal_setup() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("terminal_setup"))), Some(1));
}

#[test]
fn disruption_theme_picker() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("theme_picker"))), Some(1));
}

#[test]
fn disruption_settings_error() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("settings_error"))), Some(2));
}

#[test]
fn disruption_trust() {
    assert_eq!(disruption_option(&prompt(PromptKind::Permission, Some("trust"))), Some(1));
}

// -- Elicitations (should return None) --

#[test]
fn elicitation_oauth_login() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("oauth_login"))), None);
}

#[test]
fn elicitation_login_method() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("login_method"))), None);
}

#[test]
fn elicitation_startup_trust() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("startup_trust"))), None);
}

#[test]
fn elicitation_startup_bypass() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("startup_bypass"))), None);
}

#[test]
fn elicitation_startup_login() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, Some("startup_login"))), None);
}

#[test]
fn elicitation_tool_permission() {
    assert_eq!(disruption_option(&prompt(PromptKind::Permission, Some("tool"))), None);
}

#[test]
fn elicitation_plan() {
    assert_eq!(disruption_option(&prompt(PromptKind::Plan, None)), None);
}

#[test]
fn elicitation_question() {
    assert_eq!(disruption_option(&prompt(PromptKind::Question, None)), None);
}

#[test]
fn elicitation_setup_no_subtype() {
    assert_eq!(disruption_option(&prompt(PromptKind::Setup, None)), None);
}

#[test]
fn elicitation_permission_no_subtype() {
    assert_eq!(disruption_option(&prompt(PromptKind::Permission, None)), None);
}
