// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod detect;
pub mod encoding;
pub mod hooks;
pub mod prompt;
pub mod resume;
pub mod screen_detect;
pub mod setup;
pub mod startup;
pub mod state;

use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::config::Config;

use super::hook_recv::HookReceiver;
use super::Detector;
use detect::{HookDetector, LogDetector, StdoutDetector};
use encoding::{ClaudeNudgeEncoder, ClaudeRespondEncoder};

/// Claude Code agent driver.
///
/// Provides encoding for nudge/respond actions and detection tiers
/// for monitoring Claude's agent state.
pub struct ClaudeDriver {
    pub nudge: ClaudeNudgeEncoder,
    pub respond: ClaudeRespondEncoder,
    pub detectors: Vec<Box<dyn Detector>>,
}

impl ClaudeDriver {
    /// Build a new driver from config and runtime paths.
    ///
    /// Constructs detectors based on available tiers:
    /// - Tier 1 (HookDetector): if `hook_pipe_path` is set
    /// - Tier 2 (LogDetector): if `session_log_path` is set
    /// - Tier 3 (StdoutDetector): if `stdout_rx` is provided
    pub fn new(
        config: &Config,
        hook_pipe_path: Option<&Path>,
        session_log_path: Option<PathBuf>,
        stdout_rx: Option<mpsc::Receiver<Bytes>>,
        log_start_offset: u64,
    ) -> anyhow::Result<Self> {
        let mut detectors: Vec<Box<dyn Detector>> = Vec::new();

        // Tier 1: Hook events (highest confidence)
        if let Some(pipe_path) = hook_pipe_path {
            let receiver = HookReceiver::new(pipe_path)?;
            detectors.push(Box::new(HookDetector { receiver }));
        }

        // Tier 2: Session log watching
        if let Some(log_path) = session_log_path {
            detectors.push(Box::new(LogDetector {
                log_path,
                start_offset: log_start_offset,
                poll_interval: config.log_poll(),
            }));
        }

        // Tier 3: Structured stdout JSONL
        if let Some(stdout_rx) = stdout_rx {
            detectors.push(Box::new(StdoutDetector { stdout_rx }));
        }

        // Sort by tier (lowest number = highest priority)
        detectors.sort_by_key(|d| d.tier());

        Ok(Self {
            nudge: ClaudeNudgeEncoder { keyboard_delay: config.keyboard_delay() },
            respond: ClaudeRespondEncoder {
                feedback_delay: config.feedback_delay(),
                input_delay: config.keyboard_delay(),
            },
            detectors,
        })
    }

    /// Consume the driver and return its detectors.
    pub fn into_detectors(self) -> Vec<Box<dyn Detector>> {
        self.detectors
    }
}
