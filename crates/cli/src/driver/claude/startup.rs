// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Startup prompt detection for Claude Code.
//!
//! Claude may present text-based prompts during startup (workspace trust,
//! permission bypass, login/onboarding) that block the agent before reaching
//! the idle `WaitingForInput` state. The screen detector uses these patterns
//! to suppress false idle signals when a startup prompt is visible.

/// Known startup prompts that Claude Code may present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPrompt {
    /// "Do you trust the files in this folder?"
    WorkspaceTrust,
    /// "Allow tool use without prompting?" / --dangerously-skip-permissions
    BypassPermissions,
    /// "Please sign in" / login / onboarding flow
    LoginRequired,
}

/// Classify a screen snapshot as a startup prompt.
///
/// Scans the last non-empty lines of the screen for known prompt patterns.
pub fn detect_startup_prompt(screen_lines: &[String]) -> Option<StartupPrompt> {
    // Work backwards through lines to find the last non-empty content.
    let trimmed: Vec<&str> =
        screen_lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    if trimmed.is_empty() {
        return None;
    }

    // Check the last few lines for known patterns.
    let tail = if trimmed.len() >= 5 { &trimmed[trimmed.len() - 5..] } else { &trimmed };
    let combined = tail.join(" ");
    let lower = combined.to_lowercase();

    // Workspace trust prompt
    if lower.contains("trust the files")
        || lower.contains("do you trust")
        || lower.contains("trust this folder")
        || lower.contains("trust this workspace")
    {
        return Some(StartupPrompt::WorkspaceTrust);
    }

    // Permission bypass prompt
    if lower.contains("skip permissions")
        || lower.contains("dangerously-skip-permissions")
        || lower.contains("allow tool use without prompting")
        || lower.contains("bypass permissions")
    {
        return Some(StartupPrompt::BypassPermissions);
    }

    // Login / onboarding prompt
    if lower.contains("please sign in")
        || lower.contains("please log in")
        || lower.contains("login required")
        || lower.contains("sign in to continue")
        || lower.contains("authenticate")
    {
        return Some(StartupPrompt::LoginRequired);
    }

    None
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
