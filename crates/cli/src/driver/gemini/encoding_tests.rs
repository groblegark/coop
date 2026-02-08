// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, QuestionAnswer, RespondEncoder};

use super::{GeminiNudgeEncoder, GeminiRespondEncoder};

#[test]
fn nudge_encodes_message_with_cr() {
    let encoder = GeminiNudgeEncoder;
    let steps = encoder.encode("Fix the bug");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"Fix the bug\r");
    assert!(steps[0].delay_after.is_none());
}

#[test]
fn nudge_with_multiline_message() {
    let encoder = GeminiNudgeEncoder;
    let steps = encoder.encode("line1\nline2");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"line1\nline2\r");
}

#[yare::parameterized(
    accept = { true, b"y\r" as &[u8] },
    deny   = { false, b"n\r" },
)]
fn permission_encoding(accept: bool, expected: &[u8]) {
    let encoder = GeminiRespondEncoder::default();
    let steps = encoder.encode_permission(accept);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, expected);
}

#[test]
fn plan_accept() {
    let encoder = GeminiRespondEncoder::default();
    let steps = encoder.encode_plan(true, None);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"y\r");
    assert!(steps[0].delay_after.is_none());
}

#[test]
fn plan_reject_with_feedback() {
    let encoder = GeminiRespondEncoder::default();
    let steps = encoder.encode_plan(false, Some("Don't modify the schema"));
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].bytes, b"n\r");
    assert_eq!(steps[0].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[1].bytes, b"Don't modify the schema\r");
    assert!(steps[1].delay_after.is_none());
}

#[test]
fn plan_reject_without_feedback() {
    let encoder = GeminiRespondEncoder::default();
    let steps = encoder.encode_plan(false, None);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"n\r");
    assert!(steps[0].delay_after.is_none());
}

#[test]
fn question_with_option_number() {
    let encoder = GeminiRespondEncoder::default();
    let answers = [QuestionAnswer {
        option: Some(2),
        text: None,
    }];
    let steps = encoder.encode_question(&answers, 1);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"2\r");
}

#[test]
fn question_with_freeform_text() {
    let encoder = GeminiRespondEncoder::default();
    let answers = [QuestionAnswer {
        option: None,
        text: Some("Use Redis instead".to_string()),
    }];
    let steps = encoder.encode_question(&answers, 1);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"Use Redis instead\r");
}

#[test]
fn question_with_neither_option_nor_text() {
    let encoder = GeminiRespondEncoder::default();
    let steps = encoder.encode_question(&[], 0);
    assert!(steps.is_empty());
}
