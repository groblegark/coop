// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use crate::screen::{CursorPosition, ScreenSnapshot};

use super::{
    extract_ask_user_context, extract_ask_user_from_tool_input, extract_permission_context,
    extract_plan_context, parse_options_from_screen,
};

#[test]
fn permission_context_extracts_tool_and_preview() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "npm install express" }
            }]
        }
    });
    let ctx = extract_permission_context(&entry);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Permission);
    assert_eq!(ctx.tool.as_deref(), Some("Bash"));
    assert!(ctx.input.is_some());
    let preview = ctx.input.as_deref().unwrap_or("");
    assert!(preview.contains("npm install express"));
}

#[test]
fn permission_context_truncates_large_input() {
    let long_text = "x".repeat(500);
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Write",
                "input": { "content": long_text }
            }]
        }
    });
    let ctx = extract_permission_context(&entry);
    let preview = ctx.input.unwrap_or_default();
    assert!(preview.len() <= 210); // 200 + "..."
    assert!(preview.ends_with("..."));
}

#[test]
fn ask_user_context_extracts_question_and_options() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {
            "questions": [{
                "question": "Which database should we use?",
                "options": [
                    { "label": "PostgreSQL", "description": "Relational" },
                    { "label": "SQLite", "description": "Embedded" },
                    { "label": "MySQL", "description": "Popular" }
                ]
            }]
        }
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Question);
    assert_eq!(ctx.questions.len(), 1);
    assert_eq!(ctx.questions[0].question, "Which database should we use?");
    assert_eq!(ctx.questions[0].options, vec!["PostgreSQL", "SQLite", "MySQL"]);
}

#[test]
fn ask_user_context_with_no_options() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {
            "questions": [{
                "question": "What do you want to do?"
            }]
        }
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.questions.len(), 1);
    assert_eq!(ctx.questions[0].question, "What do you want to do?");
    assert!(ctx.questions[0].options.is_empty());
}

#[test]
fn ask_user_context_with_empty_input() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {}
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Question);
    assert!(ctx.questions.is_empty());
}

#[test]
fn plan_context_returns_plan_kind() {
    let screen = ScreenSnapshot {
        lines: vec![
            "Plan: Implement auth system".to_string(),
            "Step 1: Add middleware".to_string(),
            "[y] Accept  [n] Reject".to_string(),
        ],
        cols: 80,
        rows: 24,
        alt_screen: false,
        cursor: CursorPosition { row: 2, col: 0 },
        sequence: 42,
    };
    let ctx = extract_plan_context(&screen);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Plan);
    assert!(ctx.auth_url.is_none());
}

#[test]
fn ask_user_from_tool_input_extracts_question_and_options() {
    let tool_input = json!({
        "questions": [{
            "question": "Which framework?",
            "options": [
                { "label": "React", "description": "Popular" },
                { "label": "Vue", "description": "Progressive" }
            ]
        }]
    });
    let ctx = extract_ask_user_from_tool_input(Some(&tool_input));
    assert_eq!(ctx.kind, crate::driver::PromptKind::Question);
    assert_eq!(ctx.questions.len(), 1);
    assert_eq!(ctx.questions[0].question, "Which framework?");
    assert_eq!(ctx.questions[0].options, vec!["React", "Vue"]);
}

#[test]
fn ask_user_from_tool_input_with_none() {
    let ctx = extract_ask_user_from_tool_input(None);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Question);
    assert!(ctx.questions.is_empty());
}

#[test]
fn ask_user_extracts_all_questions() {
    let tool_input = json!({
        "questions": [
            {
                "question": "Which database?",
                "options": [
                    { "label": "PostgreSQL" },
                    { "label": "SQLite" }
                ]
            },
            {
                "question": "Which framework?",
                "options": [
                    { "label": "Axum" },
                    { "label": "Actix" },
                    { "label": "Rocket" }
                ]
            }
        ]
    });
    let ctx = extract_ask_user_from_tool_input(Some(&tool_input));

    // All questions parsed into the questions vec.
    assert_eq!(ctx.questions.len(), 2);
    assert_eq!(ctx.questions[0].question, "Which database?");
    assert_eq!(ctx.questions[0].options, vec!["PostgreSQL", "SQLite"]);
    assert_eq!(ctx.questions[1].question, "Which framework?");
    assert_eq!(ctx.questions[1].options, vec!["Axum", "Actix", "Rocket"]);

    assert_eq!(ctx.question_current, 0);
}

#[test]
fn ask_user_single_question_populates_questions_vec() {
    let tool_input = json!({
        "questions": [{
            "question": "Which DB?",
            "options": [
                { "label": "Postgres" },
                { "label": "SQLite" }
            ]
        }]
    });
    let ctx = extract_ask_user_from_tool_input(Some(&tool_input));
    assert_eq!(ctx.questions.len(), 1);
    assert_eq!(ctx.questions[0].question, "Which DB?");
}

#[test]
fn missing_fields_produce_sensible_defaults() {
    // Permission context with no message field
    let entry = json!({});
    let ctx = extract_permission_context(&entry);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Permission);
    assert!(ctx.tool.is_none());
    assert!(ctx.input.is_none());

    // AskUser context with no input field
    let block = json!({ "type": "tool_use", "name": "AskUserQuestion" });
    let ctx = extract_ask_user_context(&block);
    assert!(ctx.questions.is_empty());
}

// ---------------------------------------------------------------------------
// parse_options_from_screen tests — based on real Claude v2.1.37 TUI captures
// ---------------------------------------------------------------------------

/// Helper: load a fixture file and split into screen lines.
fn fixture_lines(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

/// Bash permission dialog (from bash_permission_dialog.tui.txt)
#[test]
fn parse_options_bash_permission() {
    let lines = fixture_lines(include_str!("fixtures/bash_permission.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, and always allow access to tmp/ from this project", "No"]);
}

/// Edit permission dialog (from edit_permission_dialog.tui.txt)
#[test]
fn parse_options_edit_permission() {
    let lines = fixture_lines(include_str!("fixtures/edit_permission.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, allow all edits during this session (shift+tab)", "No"]);
}

/// Trust folder / Bash permission dialog (from trust_folder_dialog.tui.txt)
#[test]
fn parse_options_trust_folder() {
    let lines = fixture_lines(include_str!("fixtures/trust_folder.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, allow reading from Downloads/ from this project", "No"]);
}

/// Thinking dialog (from thinking_dialog_disabled_selected.tui.txt)
#[test]
fn parse_options_thinking_dialog() {
    let lines = fixture_lines(include_str!("fixtures/thinking_dialog.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(
        opts,
        vec![
            "Enabled ✔  Claude will think before responding",
            "Disabled   Claude will respond without extended thinking",
        ]
    );
}

/// Multi-question dialog Q1 (from multi_question_dialog_q1.tui.txt)
/// Options are split across a separator line, with description lines under each.
#[test]
fn parse_options_multi_question_dialog() {
    let lines = fixture_lines(include_str!("fixtures/multi_question_q1.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Rust", "Python", "Type something.", "Chat about this"]);
}

/// Non-breaking space after ❯ (Claude uses U+00A0 in practice)
#[test]
fn parse_options_nbsp_after_selector() {
    let lines =
        vec![" Do you want to proceed?".into(), " ❯\u{00A0}1. Yes".into(), "   2. No".into()];
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "No"]);
}

#[test]
fn parse_options_empty_screen() {
    let opts = parse_options_from_screen(&[]);
    assert!(opts.is_empty());
}

/// Theme picker with trailing checkmark on selected option
#[test]
fn parse_options_strips_trailing_checkmark() {
    let lines = vec![
        " Choose the text style".into(),
        " ❯ 1. Dark mode ✔".into(),
        "   2. Light mode".into(),
        "   3. Light mode (high contrast)".into(),
        " Enter to confirm · Esc to exit".into(),
    ];
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Dark mode", "Light mode", "Light mode (high contrast)"]);
}

#[test]
fn parse_options_no_match() {
    let lines = vec!["Working on your task...".into(), "Reading files".into()];
    let opts = parse_options_from_screen(&lines);
    assert!(opts.is_empty());
}
