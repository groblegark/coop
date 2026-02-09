// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};
use crate::screen::ScreenSnapshot;

use super::prompt::parse_options_from_screen;
use super::startup::detect_startup_prompt;

/// Tier 5 detector: classifies Claude's rendered terminal screen.
///
/// Detects the idle prompt (`❯`) on the last non-empty line, anti-matching
/// known startup prompts that should be handled separately.
///
/// Polls frequently during the startup window to quickly detect the initial
/// idle prompt, then backs off to a slower cadence.
pub struct ClaudeScreenDetector {
    snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    startup_poll: Duration,
    steady_poll: Duration,
    startup_window: Duration,
}

impl ClaudeScreenDetector {
    pub fn new(
        config: &Config,
        snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    ) -> Self {
        Self {
            snapshot_fn,
            startup_poll: config.screen_startup_poll(),
            steady_poll: config.screen_steady_poll(),
            startup_window: config.screen_startup_window(),
        }
    }
}

impl Detector for ClaudeScreenDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let start = tokio::time::Instant::now();
            let mut interval = tokio::time::interval(self.startup_poll);
            let mut in_startup = true;
            let mut last_state: Option<AgentState> = None;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }

                // Switch to slower cadence after the startup window.
                if in_startup && start.elapsed() >= self.startup_window {
                    in_startup = false;
                    interval = tokio::time::interval(self.steady_poll);
                    // Consume the immediate first tick.
                    interval.tick().await;
                }

                let snapshot = (self.snapshot_fn)();
                let classified = classify_claude_screen(&snapshot);

                if let Some((ref state, ref cause)) = classified {
                    if last_state.as_ref() != Some(state) {
                        let _ = state_tx.send((state.clone(), cause.clone())).await;
                        last_state = Some(state.clone());
                    }
                } else if last_state.is_some() {
                    last_state = None;
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        5
    }
}

/// Classification of an interactive dialog screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    /// Tool permission dialog — suppressed (handled by Tier 1 hooks).
    ToolPermission,
    /// Workspace trust — emitted as `Prompt(Permission)` with subtype `"trust"`.
    Permission,
    /// Onboarding/setup dialog — emitted as `Prompt(Setup)` with a subtype string.
    Setup(&'static str),
}

/// Classify Claude's screen, returning the state and a cause string.
///
/// Emits `Prompt(Setup)` for onboarding dialogs, `Prompt(Permission)` for
/// workspace trust, and `WaitingForInput` for the idle prompt. Tool
/// permission dialogs and startup text prompts are suppressed (`None`).
fn classify_claude_screen(snapshot: &ScreenSnapshot) -> Option<(AgentState, String)> {
    // Classify interactive dialogs first — they take priority over the
    // simple startup text prompts which match more broadly.
    match classify_interactive_dialog(&snapshot.lines) {
        Some(DialogKind::ToolPermission) => return None,
        Some(DialogKind::Permission) => {
            let options = parse_options_from_screen(&snapshot.lines);
            return Some((
                AgentState::Prompt {
                    prompt: PromptContext {
                        kind: PromptKind::Permission,
                        subtype: Some("trust".to_owned()),
                        tool: None,
                        input: None,
                        auth_url: None,
                        options,
                        options_fallback: false,
                        questions: vec![],
                        question_current: 0,
                    },
                },
                "screen:permission".to_owned(),
            ));
        }
        Some(DialogKind::Setup(subtype)) => {
            let options = parse_options_from_screen(&snapshot.lines);
            let auth_url =
                if subtype == "oauth_login" { extract_auth_url(&snapshot.lines) } else { None };
            return Some((
                AgentState::Prompt {
                    prompt: PromptContext {
                        kind: PromptKind::Setup,
                        subtype: Some(subtype.to_owned()),
                        tool: None,
                        input: None,
                        auth_url,
                        options,
                        options_fallback: false,
                        questions: vec![],
                        question_current: 0,
                    },
                },
                "screen:setup".to_owned(),
            ));
        }
        None => {}
    }

    // Skip if a known startup text prompt is present — those are handled
    // separately by the session auto-responder and should not appear as
    // WaitingForInput. Checked after dialog classification because the
    // startup detector matches broadly (e.g. "trust this folder" appears in
    // both the simple y/n prompt and the interactive Accessing workspace dialog).
    if detect_startup_prompt(&snapshot.lines).is_some() {
        return None;
    }

    // Look for Claude's idle prompt indicator anywhere in the visible lines.
    // Claude Code renders `❯` (U+276F) at the start of its input line.
    // Status text like "ctrl+t to hide tasks" may appear below the prompt,
    // so we scan all non-empty lines rather than only the last.
    for line in snapshot.lines.iter().rev() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.starts_with('\u{276f}') {
            return Some((AgentState::WaitingForInput, "screen:idle".to_owned()));
        }
    }

    None
}

/// Signal phrases for a dialog screen, paired with its classification.
/// Each screen defines 2-3 signal phrases; a match requires 2+ signals.
/// Signals are `(phrase, case_insensitive)`.
type DialogScreen = (DialogKind, &'static [(&'static str, bool)]);

const DIALOG_SCREENS: &[DialogScreen] = &[
    // Security notes
    (
        DialogKind::Setup("security_notes"),
        &[
            ("Security notes:", false),
            ("Claude can make mistakes", false),
            ("Press Enter to continue", false),
        ],
    ),
    // Login success
    (
        DialogKind::Setup("login_success"),
        &[("Login successful", false), ("Logged in as", false), ("Press Enter to continue", false)],
    ),
    // OAuth login
    (
        DialogKind::Setup("oauth_login"),
        &[("Paste code here if prompted", false), ("oauth/authorize", false)],
    ),
    // Login method picker
    (
        DialogKind::Setup("login_method"),
        &[
            ("Select login method:", false),
            ("Claude account with subscription", false),
            ("Anthropic Console account", false),
        ],
    ),
    // Workspace trust
    (
        DialogKind::Permission,
        &[
            ("Accessing workspace:", false),
            ("Yes, I trust this folder", false),
            ("enter to confirm", true),
        ],
    ),
    // Terminal setup
    (
        DialogKind::Setup("terminal_setup"),
        &[
            ("Use Claude Code's terminal setup?", false),
            ("Yes, use recommended settings", false),
            ("enter to confirm", true),
        ],
    ),
    // Theme picker
    (
        DialogKind::Setup("theme_picker"),
        &[("Choose the text style", false), ("Dark mode", false), ("enter to confirm", true)],
    ),
    // Tool permission
    (
        DialogKind::ToolPermission,
        &[
            ("Do you want to proceed?", false),
            ("Yes, and don't ask again", false),
            ("Esc to cancel", false),
        ],
    ),
];

/// Minimum number of signals that must match to identify a dialog screen.
const DIALOG_SIGNAL_THRESHOLD: usize = 2;

/// Classify the screen as an interactive dialog, returning the dialog kind
/// if recognized, or `None` if no dialog is detected.
///
/// Each known dialog screen defines 2-3 signal phrases; a match requires
/// at least [`DIALOG_SIGNAL_THRESHOLD`] signals present on screen.
fn classify_interactive_dialog(lines: &[String]) -> Option<DialogKind> {
    for (kind, signals) in DIALOG_SCREENS {
        let mut hits = 0;
        for &(phrase, ci) in *signals {
            let found = lines.iter().any(|line| {
                let trimmed = line.trim();
                if ci {
                    trimmed.to_lowercase().contains(phrase)
                } else {
                    trimmed.contains(phrase)
                }
            });
            if found {
                hits += 1;
                if hits >= DIALOG_SIGNAL_THRESHOLD {
                    return Some(*kind);
                }
            }
        }
    }
    None
}

/// Extract an OAuth authorization URL from screen lines.
///
/// The URL starts with `https://claude.ai/oauth/authorize?` and may wrap
/// across multiple terminal lines.  Continuation lines start at column 0
/// (no leading whitespace) while surrounding UI text is indented.
fn extract_auth_url(lines: &[String]) -> Option<String> {
    let prefix = "https://claude.ai/oauth/authorize?";
    let start_idx = lines.iter().position(|line| line.trim_start().starts_with(prefix))?;

    let mut url = lines[start_idx].trim().to_string();

    // Concatenate hard-wrapped continuation lines.
    for line in &lines[start_idx + 1..] {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with(' ') {
            break;
        }
        url.push_str(trimmed);
    }

    Some(url)
}

#[cfg(test)]
#[path = "screen_detect_tests.rs"]
mod tests;
