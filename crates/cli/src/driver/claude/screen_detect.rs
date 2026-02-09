// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::driver::{AgentState, Detector};
use crate::screen::ScreenSnapshot;

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
        state_tx: mpsc::Sender<AgentState>,
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
                let new_state = classify_claude_screen(&snapshot);

                if let Some(ref state) = new_state {
                    if last_state.as_ref() != Some(state) {
                        let _ = state_tx.send(state.clone()).await;
                        last_state = new_state;
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

/// Classify Claude's screen as idle when the prompt indicator is visible
/// and no startup prompt is blocking.
fn classify_claude_screen(snapshot: &ScreenSnapshot) -> Option<AgentState> {
    // Skip if a known startup prompt is present — those are handled
    // separately and should not appear as WaitingForInput.
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
            return Some(AgentState::WaitingForInput);
        }
    }

    None
}

#[cfg(test)]
#[path = "screen_detect_tests.rs"]
mod tests;
