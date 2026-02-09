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
    let snap = snapshot(&["Claude Code v2.1.37", "", "\u{276f} Try \"fix lint errors\"", ""]);
    assert_eq!(classify_claude_screen(&snap), Some(AgentState::WaitingForInput));
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
    assert_eq!(classify_claude_screen(&snap), Some(AgentState::WaitingForInput));
}

#[test]
fn no_idle_on_workspace_trust_dialog() {
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
fn no_idle_on_theme_picker() {
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
fn no_idle_on_tool_permission_dialog() {
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
fn no_idle_on_bypass_permissions_dialog() {
    let snap = snapshot(&[
        " WARNING: Claude Code running in Bypass Permissions mode",
        "",
        " In Bypass Permissions mode, Claude Code will not ask for your",
        " approval before running potentially dangerous commands.",
        "",
        " \u{276f} 1. No, exit",
        "   2. Yes, I accept",
        "",
        " Enter to confirm \u{00b7} Esc to cancel",
    ]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn idle_with_bypass_permissions_status_bar() {
    // After accepting bypass permissions, the status bar shows the indicator.
    // The idle prompt should still be detected.
    let snap = snapshot(&[
        "\u{276f} Try \"refactor <filepath>\"",
        "────────────────────────────────",
        "  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle)",
    ]);
    assert_eq!(classify_claude_screen(&snap), Some(AgentState::WaitingForInput));
}

#[test]
fn idle_with_dismissed_bypass_dialog_still_visible() {
    // Full terminal after bypass dialog accepted: warning text persists at top,
    // welcome screen + idle prompt in the middle, status bar at bottom.
    let snap = snapshot(&[
        "────────────────────────────────────────",
        " WARNING: Claude Code running in Bypass Permissions mode",
        "",
        " In Bypass Permissions mode, Claude Code will not ask for your",
        " approval before running potentially dangerous commands.",
        "",
        " https://code.claude.com/docs/en/security",
        "",
        "   1. No, exit",
        " \u{276f} 2. Yes, I accept",
        "",
        " Enter to confirm \u{00b7} Esc to cancel",
        "",
        "\u{256d}\u{2500}\u{2500}\u{2500} Claude Code v2.1.37 \u{2500}\u{2500}\u{2500}",
        "\u{2502}     Welcome back Matt!",
        "\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
        "  Welcome to Opus 4.6",
        "────────────────────────────────────────",
        "\u{276f} Try \"how does <filepath> work?\"",
        "────────────────────────────────────────",
        "  \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle)",
        "",
        "",
        "",
    ]);
    assert_eq!(classify_claude_screen(&snap), Some(AgentState::WaitingForInput));
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
    assert_eq!(classify_claude_screen(&snap), Some(AgentState::WaitingForInput));
}
