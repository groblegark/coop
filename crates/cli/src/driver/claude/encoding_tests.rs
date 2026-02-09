// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use crate::driver::{NudgeEncoder, QuestionAnswer, RespondEncoder};

use super::{ClaudeNudgeEncoder, ClaudeRespondEncoder};

#[test]
fn nudge_encodes_message_with_cr() {
    let encoder = ClaudeNudgeEncoder;
    let steps = encoder.encode("Fix the bug");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"Fix the bug\r");
    assert!(steps[0].delay_after.is_none());
}

#[test]
fn nudge_with_multiline_message() {
    let encoder = ClaudeNudgeEncoder;
    let steps = encoder.encode("line1\nline2");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"line1\nline2\r");
}

#[yare::parameterized(
    accept = { true, b"y\r" as &[u8] },
    deny   = { false, b"n\r" },
)]
fn permission_encoding(accept: bool, expected: &[u8]) {
    let encoder = ClaudeRespondEncoder::default();
    let steps = encoder.encode_permission(accept);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, expected);
}

#[test]
fn plan_accept() {
    let encoder = ClaudeRespondEncoder::default();
    let steps = encoder.encode_plan(true, None);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"y\r");
    assert!(steps[0].delay_after.is_none());
}

#[test]
fn plan_reject_with_feedback() {
    let encoder = ClaudeRespondEncoder::default();
    let steps = encoder.encode_plan(false, Some("Don't modify the schema"));
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].bytes, b"n\r");
    assert_eq!(steps[0].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[1].bytes, b"Don't modify the schema\r");
    assert!(steps[1].delay_after.is_none());
}

#[test]
fn plan_reject_without_feedback() {
    let encoder = ClaudeRespondEncoder::default();
    let steps = encoder.encode_plan(false, None);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"n\r");
    assert!(steps[0].delay_after.is_none());
}

// ---------------------------------------------------------------------------
// Single-question mode (total_questions <= 1)
// ---------------------------------------------------------------------------

#[test]
fn question_single_with_option_number() {
    let encoder = ClaudeRespondEncoder::default();
    let answers = [QuestionAnswer {
        option: Some(2),
        text: None,
    }];
    let steps = encoder.encode_question(&answers, 1);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"2\r");
}

#[test]
fn question_single_with_freeform_text() {
    let encoder = ClaudeRespondEncoder::default();
    let answers = [QuestionAnswer {
        option: None,
        text: Some("Use Redis instead".to_string()),
    }];
    let steps = encoder.encode_question(&answers, 1);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"Use Redis instead\r\r");
}

#[test]
fn question_with_empty_answers() {
    let encoder = ClaudeRespondEncoder::default();
    let steps = encoder.encode_question(&[], 1);
    assert!(steps.is_empty());
}

// ---------------------------------------------------------------------------
// One-at-a-time mode (single answer, total_questions > 1)
// ---------------------------------------------------------------------------

#[test]
fn question_one_at_a_time_emits_digit_only() {
    let encoder = ClaudeRespondEncoder::default();
    let answers = [QuestionAnswer {
        option: Some(1),
        text: None,
    }];
    // Single answer in a multi-question dialog → just digit, no CR.
    let steps = encoder.encode_question(&answers, 3);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].bytes, b"1");
    assert!(steps[0].delay_after.is_none());
}

// ---------------------------------------------------------------------------
// All-at-once mode (multiple answers)
// ---------------------------------------------------------------------------

#[test]
fn question_all_at_once_emits_sequence_with_delays() {
    let encoder = ClaudeRespondEncoder::default();
    let answers = [
        QuestionAnswer {
            option: Some(1),
            text: None,
        },
        QuestionAnswer {
            option: Some(2),
            text: None,
        },
    ];
    let steps = encoder.encode_question(&answers, 2);
    // Two answer steps + one confirm step.
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].bytes, b"1");
    assert_eq!(steps[0].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[1].bytes, b"2");
    assert_eq!(steps[1].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[2].bytes, b"\r");
    assert!(steps[2].delay_after.is_none());
}

#[test]
fn question_all_at_once_freeform_mixed() {
    let encoder = ClaudeRespondEncoder::default();
    let answers = [
        QuestionAnswer {
            option: Some(1),
            text: None,
        },
        QuestionAnswer {
            option: None,
            text: Some("custom answer".to_string()),
        },
    ];
    let steps = encoder.encode_question(&answers, 2);
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].bytes, b"1");
    assert_eq!(steps[0].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[1].bytes, b"custom answer\r");
    assert_eq!(steps[1].delay_after, Some(Duration::from_millis(100)));
    assert_eq!(steps[2].bytes, b"\r");
    assert!(steps[2].delay_after.is_none());
}
