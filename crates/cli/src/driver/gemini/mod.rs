// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod detect;
pub mod encoding;
pub mod hooks;
pub mod setup;
pub mod state;

use std::path::Path;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::config::Config;

use super::hook_recv::HookReceiver;
use super::Detector;
use detect::{HookDetector, StdoutDetector};
use encoding::{GeminiNudgeEncoder, GeminiRespondEncoder};

/// Gemini CLI agent driver.
///
/// Provides encoding for nudge/respond actions and detection tiers
/// for monitoring Gemini's agent state.
pub struct GeminiDriver {
    pub nudge: GeminiNudgeEncoder,
    pub respond: GeminiRespondEncoder,
    pub detectors: Vec<Box<dyn Detector>>,
}

impl GeminiDriver {
    /// Build a new driver from config and runtime paths.
    ///
    /// Constructs detectors based on available tiers:
    /// - Tier 1 (HookDetector): if `hook_pipe_path` is set
    /// - Tier 3 (StdoutDetector): if `stdout_rx` is provided
    pub fn new(
        config: &Config,
        hook_pipe_path: Option<&Path>,
        stdout_rx: Option<mpsc::Receiver<Bytes>>,
    ) -> anyhow::Result<Self> {
        let mut detectors: Vec<Box<dyn Detector>> = Vec::new();

        // Tier 1: Hook events (highest confidence)
        if let Some(pipe_path) = hook_pipe_path {
            let receiver = HookReceiver::new(pipe_path)?;
            detectors.push(Box::new(HookDetector { receiver }));
        }

        // Tier 3: Structured stdout JSONL
        if let Some(stdout_rx) = stdout_rx {
            detectors.push(Box::new(StdoutDetector { stdout_rx }));
        }

        // Sort by tier (lowest number = highest priority)
        detectors.sort_by_key(|d| d.tier());

        Ok(Self {
            nudge: GeminiNudgeEncoder,
            respond: GeminiRespondEncoder {
                feedback_delay: config.feedback_delay(),
            },
            detectors,
        })
    }
}
