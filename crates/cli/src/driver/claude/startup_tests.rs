// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::{detect_startup_prompt, StartupPrompt};

#[test]
fn detect_workspace_trust_prompt() {
    let lines = vec![
        "".to_owned(),
        "  Welcome to Claude Code!".to_owned(),
        "".to_owned(),
        "  Do you trust the files in this folder?".to_owned(),
        "  (y/n)".to_owned(),
    ];
    assert_eq!(detect_startup_prompt(&lines), Some(StartupPrompt::WorkspaceTrust));
}

#[test]
fn detect_trust_this_workspace() {
    let lines = vec!["Do you trust this workspace? [y/N]".to_owned()];
    assert_eq!(detect_startup_prompt(&lines), Some(StartupPrompt::WorkspaceTrust));
}

#[test]
fn detect_bypass_permissions_prompt() {
    let lines = vec![
        "".to_owned(),
        "  --dangerously-skip-permissions detected".to_owned(),
        "  Allow tool use without prompting? (y/n)".to_owned(),
    ];
    assert_eq!(detect_startup_prompt(&lines), Some(StartupPrompt::BypassPermissions));
}

#[test]
fn detect_login_required() {
    let lines =
        vec!["Claude Code".to_owned(), "".to_owned(), "Please sign in to continue.".to_owned()];
    assert_eq!(detect_startup_prompt(&lines), Some(StartupPrompt::LoginRequired));
}

#[test]
fn no_prompt_on_empty_screen() {
    let lines: Vec<String> = vec![];
    assert_eq!(detect_startup_prompt(&lines), None);
}

#[test]
fn no_prompt_on_normal_output() {
    let lines = vec![
        "$ claude --model opus".to_owned(),
        "I'll help you with that task.".to_owned(),
        "Let me start by reading the file.".to_owned(),
    ];
    assert_eq!(detect_startup_prompt(&lines), None);
}
