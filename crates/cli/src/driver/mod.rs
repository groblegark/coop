// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod claude;
pub mod error_category;
pub mod gemini;
pub mod hook_recv;
pub mod jsonl_stdout;
pub mod log_watch;
pub mod process;
pub mod screen_parse;
pub mod unknown;

pub use error_category::{classify_error_detail, ErrorCategory};

use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// Known agent types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    Claude,
    Codex,
    Gemini,
    Unknown,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => f.write_str("claude"),
            Self::Codex => f.write_str("codex"),
            Self::Gemini => f.write_str("gemini"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// Classified state of the agent process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentState {
    Starting,
    Working,
    WaitingForInput,
    Prompt { prompt: PromptContext },
    Error { detail: String },
    Exited { status: ExitStatus },
    Unknown,
}

/// Distinguishes the type of prompt the agent is presenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Permission,
    Plan,
    Question,
}

impl PromptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Plan => "plan",
            Self::Question => "question",
        }
    }
}

/// Contextual information about a prompt the agent is presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptContext {
    pub kind: PromptKind,
    pub tool: Option<String>,
    pub input_preview: Option<String>,
    pub screen_lines: Vec<String>,
    /// All questions in a multi-question dialog.
    #[serde(default)]
    pub questions: Vec<QuestionContext>,
    /// 0-indexed active question; == questions.len() means confirm phase.
    #[serde(default)]
    pub question_current: usize,
}

/// A single question within a multi-question dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionContext {
    pub question: String,
    pub options: Vec<String>,
}

/// An answer to a single question within a dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub option: Option<u32>,
    pub text: Option<String>,
}

/// Exit status of the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// A single step in a nudge sequence written to the PTY.
#[derive(Debug, Clone)]
pub struct NudgeStep {
    pub bytes: Vec<u8>,
    pub delay_after: Option<Duration>,
}

/// A state detection source that monitors structured data and emits
/// [`AgentState`] transitions.
///
/// Object-safe for use as `Box<dyn Detector>`.
pub trait Detector: Send + 'static {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    fn tier(&self) -> u8;
}

/// Encodes a plain-text nudge message into PTY byte sequences.
pub trait NudgeEncoder: Send + Sync {
    fn encode(&self, message: &str) -> Vec<NudgeStep>;
}

/// Encodes structured prompt responses into PTY byte sequences.
pub trait RespondEncoder: Send + Sync {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep>;
    fn encode_plan(&self, accept: bool, feedback: Option<&str>) -> Vec<NudgeStep>;
    fn encode_question(&self, answers: &[QuestionAnswer], total_questions: usize)
        -> Vec<NudgeStep>;
}

/// A state emission from the composite detector, including the tier that
/// produced it.
#[derive(Debug, Clone)]
pub struct DetectedState {
    pub state: AgentState,
    pub tier: u8,
}

/// Combines multiple [`Detector`] tiers to produce a unified agent state
/// stream.
///
/// Tier resolution rules:
/// - Lower tier number = higher confidence.
/// - States from equal-or-higher confidence tiers are accepted immediately.
/// - Lower confidence tiers may only *escalate* state priority; downgrades
///   are silently rejected.
/// - Duplicate states (prev == next) are suppressed.
pub struct CompositeDetector {
    pub tiers: Vec<Box<dyn Detector>>,
}

impl AgentState {
    /// Return the wire-format string for this state (e.g. `"working"`,
    /// `"prompt"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::WaitingForInput => "waiting_for_input",
            Self::Prompt { .. } => "prompt",
            Self::Error { .. } => "error",
            Self::Exited { .. } => "exited",
            Self::Unknown => "unknown",
        }
    }

    /// Relative priority for tier-based state resolution.
    ///
    /// Lower-confidence tiers may only *escalate* state (move to a higher
    /// priority); they are never allowed to downgrade it.  Same-or-higher
    /// confidence tiers may transition in any direction.
    ///
    /// ```text
    /// Starting(0) < WaitingForInput(1) < Error(2) < Working(3) < Prompt(4)
    /// ```
    ///
    /// `Unknown` is treated the same as `Starting` (lowest).
    /// `Exited` is handled separately (always accepted) and never compared.
    pub fn state_priority(&self) -> u8 {
        match self {
            Self::Starting | Self::Unknown => 0,
            Self::WaitingForInput => 1,
            Self::Error { .. } => 2,
            Self::Working => 3,
            Self::Prompt { .. } => 4,
            Self::Exited { .. } => 5,
        }
    }

    /// Extract the prompt context from state variants that carry one.
    pub fn prompt(&self) -> Option<&PromptContext> {
        match self {
            Self::Prompt { prompt } => Some(prompt),
            _ => None,
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl CompositeDetector {
    /// Run the composite detector, spawning all tier detectors and
    /// multiplexing their outputs with tier priority + dedup.
    ///
    /// - `output_tx`: deduplicated state emissions sent to the session loop.
    pub async fn run(
        mut self,
        output_tx: mpsc::Sender<DetectedState>,
        shutdown: CancellationToken,
    ) {
        // Internal channel where each detector sends (tier, state).
        let (tag_tx, mut tag_rx) = mpsc::channel::<(u8, AgentState)>(64);

        // Spawn each detector with a forwarding task that tags with tier.
        for detector in self.tiers.drain(..) {
            let tier = detector.tier();
            let inner_tx = tag_tx.clone();
            let sd = shutdown.clone();
            let (det_tx, mut det_rx) = mpsc::channel::<AgentState>(16);

            tokio::spawn(detector.run(det_tx, sd));
            tokio::spawn(async move {
                while let Some(state) = det_rx.recv().await {
                    if inner_tx.send((tier, state)).await.is_err() {
                        break;
                    }
                }
            });
        }
        drop(tag_tx); // only forwarding tasks hold senders

        let mut current_state = AgentState::Starting;
        let mut current_tier: u8 = u8::MAX;

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                tagged = tag_rx.recv() => {
                    let Some((tier, new_state)) = tagged else { break };

                    // Terminal states always accepted immediately.
                    if matches!(new_state, AgentState::Exited { .. }) {
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier }).await;
                        continue;
                    }

                    // Dedup: same state from any tier → update tier tracking only.
                    if new_state == current_state {
                        if tier < current_tier {
                            current_tier = tier;
                        }
                        continue;
                    }

                    // State changed.
                    if tier <= current_tier {
                        // Same or higher confidence → accept immediately,
                        // UNLESS a generic Permission prompt would overwrite
                        // a more specific Plan or Question prompt from the
                        // same tier (Claude fires both notification and
                        // pre_tool_use hooks for the same prompt moment).
                        if tier == current_tier
                            && prompt_supersedes(&current_state, &new_state)
                        {
                            continue;
                        }
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier }).await;
                    } else if new_state.state_priority() > current_state.state_priority() {
                        // Lower confidence tier escalating state → accept.
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier }).await;
                    } else {
                        // Lower confidence tier attempting to downgrade or
                        // maintain state priority → reject silently.
                        debug!(
                            tier,
                            new = new_state.as_str(),
                            current = current_state.as_str(),
                            "rejected state downgrade from lower confidence tier"
                        );
                    }
                }
            }
        }
    }
}

/// Returns `true` when `current` is a specific prompt state that should not
/// be overwritten by the more generic `incoming` prompt from the same tier.
///
/// Plan and Question prompts carry richer context than Permission prompts.
/// When the agent fires both a specific pre-tool-use event and a generic
/// permission notification for the same user-facing moment, the specific
/// state should stick.
fn prompt_supersedes(current: &AgentState, incoming: &AgentState) -> bool {
    match (current, incoming) {
        (AgentState::Prompt { prompt: cur }, AgentState::Prompt { prompt: inc }) => {
            inc.kind == PromptKind::Permission
                && matches!(cur.kind, PromptKind::Plan | PromptKind::Question)
        }
        _ => false,
    }
}

impl std::fmt::Debug for CompositeDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeDetector").field("tiers", &self.tiers.len()).finish()
    }
}

#[cfg(test)]
#[path = "composite_tests.rs"]
mod composite_tests;
