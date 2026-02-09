// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::driver::{AgentState, PromptKind};
use crate::screen::{CursorPosition, ScreenSnapshot};

use super::classify_claude_screen;

fn snapshot(lines: &[&str]) -> ScreenSnapshot {
    ScreenSnapshot {
        lines: lines.iter().map(|s| s.to_string()).collect(),
        cols: 80,
        rows: 24,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    }
}

/// Extract just the state from the classify result for simple assertions.
fn state(snap: &ScreenSnapshot) -> Option<AgentState> {
    classify_claude_screen(snap).map(|(s, _)| s)
}

/// Extract the cause string from the classify result.
fn cause(snap: &ScreenSnapshot) -> Option<String> {
    classify_claude_screen(snap).map(|(_, c)| c)
}

#[test]
fn detects_idle_prompt() {
    let snap = snapshot(&["Claude Code v2.1.37", "", "\u{276f} Try \"fix lint errors\"", ""]);
    assert_eq!(state(&snap), Some(AgentState::WaitingForInput));
    assert_eq!(cause(&snap).as_deref(), Some("screen:idle"));
}

#[test]
fn no_idle_on_empty_screen() {
    let snap = snapshot(&["", "", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_idle_on_working_output() {
    let snap = snapshot(&["Reading file src/main.rs...", "Analyzing code...", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_idle_on_startup_prompt() {
    let snap = snapshot(&["Do you trust the files in this folder?", "(y/n)", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn detects_bare_prompt() {
    let snap = snapshot(&["\u{276f} ", ""]);
    assert_eq!(state(&snap), Some(AgentState::WaitingForInput));
}

#[test]
fn workspace_trust_dialog_emits_permission() {
    let snap = snapshot(&[
        " Accessing workspace:",
        " /Users/kestred/Developer/foo",
        "",
        " \u{276f} 1. Yes, I trust this folder",
        "   2. No, exit",
        "",
        " Enter to confirm \u{00b7} Esc to cancel",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Permission);
    assert_eq!(prompt.subtype.as_deref(), Some("trust"));
    assert!(!prompt.options.is_empty(), "should parse options");
    assert_eq!(c, "screen:permission");
}

#[test]
fn theme_picker_emits_setup() {
    let snap = snapshot(&[
        " Choose the text style that looks best with your terminal",
        "",
        " \u{276f} 1. Dark mode \u{2714}",
        "   2. Light mode",
        "",
        " Enter to confirm",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("theme_picker"));
    assert!(!prompt.options.is_empty(), "should parse options");
    assert_eq!(c, "screen:setup");
}

#[test]
fn tool_permission_dialog_still_suppressed() {
    let snap = snapshot(&[
        " Bash command",
        "",
        "   coop send '{\"status\":\"done\",\"message\":\"Said goodbye as requested.\"}'",
        "   Signal done to coop stop hook",
        "",
        " Do you want to proceed?",
        " \u{276f} 1. Yes",
        "   2. Yes, and don't ask again for coop send commands in /Users/kestred/Developer/coop",
        "   3. No",
        "",
        " Esc to cancel \u{00b7} Tab to amend \u{00b7} ctrl+e to explain",
    ]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn security_notes_emits_setup() {
    let snap = snapshot(&[
        " Security notes:",
        "",
        " Claude can make mistakes. Review tool use requests carefully.",
        "",
        " Press Enter to continue...",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("security_notes"));
    assert_eq!(c, "screen:setup");
}

#[test]
fn login_success_emits_setup() {
    let snap = snapshot(&[
        " Login successful. Press Enter to continue...",
        "",
        " Logged in as user@example.com",
    ]);
    let (s, _) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("login_success"));
}

#[test]
fn detects_prompt_with_status_text_below() {
    let snap = snapshot(&[
        "Claude Code v2.1.37",
        "",
        "\u{276f}\u{00a0}Try \"create a util logging.py that...\"",
        "────────────────────────────────",
        "  ctrl+t to hide tasks",
    ]);
    assert_eq!(state(&snap), Some(AgentState::WaitingForInput));
}
