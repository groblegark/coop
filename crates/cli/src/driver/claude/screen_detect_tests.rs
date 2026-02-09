// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::driver::AgentState;
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

#[test]
fn detects_idle_prompt() {
    let snap = snapshot(&[
        "Claude Code v2.1.37",
        "",
        "\u{276f} Try \"fix lint errors\"",
        "",
    ]);
    assert_eq!(
        classify_claude_screen(&snap),
        Some(AgentState::WaitingForInput)
    );
}

#[test]
fn no_match_on_empty_screen() {
    let snap = snapshot(&["", "", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_match_on_working_output() {
    let snap = snapshot(&["Reading file src/main.rs...", "Analyzing code...", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_match_on_startup_prompt() {
    let snap = snapshot(&["Do you trust the files in this folder?", "(y/n)", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn detects_bare_prompt() {
    let snap = snapshot(&["\u{276f} ", ""]);
    assert_eq!(
        classify_claude_screen(&snap),
        Some(AgentState::WaitingForInput)
    );
}

#[test]
fn no_match_on_workspace_trust_dialog() {
    let snap = snapshot(&[
        " Accessing workspace:",
        " /Users/kestred/Developer/foo",
        "",
        " \u{276f} 1. Yes, I trust this folder",
        "   2. No, exit",
        "",
        " Enter to confirm \u{00b7} Esc to cancel",
    ]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_match_on_theme_picker() {
    let snap = snapshot(&[
        " Choose the text style that looks best with your terminal",
        "",
        " \u{276f} 1. Dark mode \u{2714}",
        "   2. Light mode",
        "",
        " Enter to confirm",
    ]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_match_on_tool_permission_dialog() {
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
fn detects_prompt_with_status_text_below() {
    let snap = snapshot(&[
        "Claude Code v2.1.37",
        "",
        "\u{276f}\u{00a0}Try \"create a util logging.py that...\"",
        "────────────────────────────────",
        "  ctrl+t to hide tasks",
    ]);
    assert_eq!(
        classify_claude_screen(&snap),
        Some(AgentState::WaitingForInput)
    );
}
