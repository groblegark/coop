// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod detect;
pub mod encoding;
pub mod hooks;
pub mod setup;
pub mod state;

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;

use super::hook_recv::HookReceiver;
use super::Detector;
use detect::{HookDetector, StdoutDetector};
use encoding::{GeminiNudgeEncoder, GeminiRespondEncoder};

/// Configuration for building a [`GeminiDriver`].
pub struct GeminiDriverConfig {
    /// Path for the hook named pipe (Tier 1).
    pub hook_pipe_path: Option<PathBuf>,
    /// Channel for raw stdout JSONL bytes (Tier 3).
    /// Used when Gemini runs with `--output-format stream-json`.
    pub stdout_rx: Option<mpsc::Receiver<Bytes>>,
    /// Delay between plan rejection keystroke and feedback text.
    pub feedback_delay: Duration,
}

/// Gemini CLI agent driver.
///
/// Provides encoding for nudge/respond actions and detection tiers
/// for monitoring Gemini's agent state.
pub struct GeminiDriver {
    pub nudge: GeminiNudgeEncoder,
    pub respond: GeminiRespondEncoder,
    pub detectors: Vec<Box<dyn Detector>>,
    /// Stored for `env_vars()`; the pipe path must stay available.
    hook_pipe_path: Option<PathBuf>,
}

impl GeminiDriver {
    /// Build a new driver from the given configuration.
    ///
    /// Constructs detectors based on available tiers:
    /// - Tier 1 (HookDetector): if `hook_pipe_path` is set
    /// - Tier 3 (StdoutDetector): if `stdout_rx` is provided
    pub fn new(config: GeminiDriverConfig) -> anyhow::Result<Self> {
        let mut detectors: Vec<Box<dyn Detector>> = Vec::new();
        let hook_pipe_path = config.hook_pipe_path.clone();

        // Tier 1: Hook events (highest confidence)
        if let Some(pipe_path) = config.hook_pipe_path {
            let receiver = HookReceiver::new(&pipe_path)?;
            detectors.push(Box::new(HookDetector { receiver }));
        }

        // Tier 3: Structured stdout JSONL
        if let Some(stdout_rx) = config.stdout_rx {
            detectors.push(Box::new(StdoutDetector { stdout_rx }));
        }

        // Sort by tier (lowest number = highest priority)
        detectors.sort_by_key(|d| d.tier());

        Ok(Self {
            nudge: GeminiNudgeEncoder,
            respond: GeminiRespondEncoder {
                feedback_delay: config.feedback_delay,
            },
            detectors,
            hook_pipe_path,
        })
    }

    /// Return environment variables needed by the Gemini child process.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        match &self.hook_pipe_path {
            Some(path) => hooks::hook_env_vars(path),
            None => vec![],
        }
    }
}
