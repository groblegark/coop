// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod claude;
pub mod error_category;
pub mod grace;
pub mod hook_recv;
pub mod jsonl_stdout;
pub mod log_watch;
pub mod process;
pub mod screen_parse;
pub mod unknown;

pub use error_category::{classify_error_detail, ErrorCategory};

use grace::{GraceCheck, IdleGraceTimer};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
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
    PermissionPrompt { prompt: PromptContext },
    PlanPrompt { prompt: PromptContext },
    Question { prompt: PromptContext },
    Error { detail: String },
    AltScreen,
    Exited { status: ExitStatus },
    Unknown,
}

/// Contextual information about a prompt the agent is presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptContext {
    pub prompt_type: String,
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

/// Combines multiple [`Detector`] tiers with a grace timer to produce
/// a unified agent state stream.
///
/// Tier resolution rules:
/// - Lower tier number = higher confidence.
/// - States from equal-or-higher confidence tiers are accepted immediately.
/// - Idle transitions from lower confidence tiers go through the grace timer.
/// - Non-idle transitions are accepted from any tier.
/// - Duplicate states (prev == next) are suppressed.
pub struct CompositeDetector {
    pub tiers: Vec<Box<dyn Detector>>,
    pub grace_timer: IdleGraceTimer,
    pub grace_tick_interval: Duration,
}

impl AgentState {
    /// Return the wire-format string for this state (e.g. `"working"`,
    /// `"permission_prompt"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::WaitingForInput => "waiting_for_input",
            Self::PermissionPrompt { .. } => "permission_prompt",
            Self::PlanPrompt { .. } => "plan_prompt",
            Self::Question { .. } => "question",
            Self::Error { .. } => "error",
            Self::AltScreen => "alt_screen",
            Self::Exited { .. } => "exited",
            Self::Unknown => "unknown",
        }
    }

    /// Extract the prompt context from state variants that carry one.
    pub fn prompt(&self) -> Option<&PromptContext> {
        match self {
            Self::PermissionPrompt { prompt }
            | Self::PlanPrompt { prompt }
            | Self::Question { prompt } => Some(prompt),
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
    /// multiplexing their outputs with tier priority + dedup + grace timer.
    ///
    /// - `output_tx`: deduplicated state emissions sent to the session loop.
    /// - `activity_fn`: lock-free function returning total bytes written
    ///   (used by the grace timer to detect ongoing output).
    /// - `grace_deadline`: shared deadline for the HTTP/gRPC API to report
    ///   `idle_grace_remaining_secs`.
    pub async fn run(
        mut self,
        output_tx: mpsc::Sender<DetectedState>,
        activity_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
        grace_deadline: Arc<parking_lot::Mutex<Option<std::time::Instant>>>,
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
        let mut grace_proposed: Option<(u8, AgentState)> = None;
        let mut grace_check_interval = tokio::time::interval(self.grace_tick_interval);

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                tagged = tag_rx.recv() => {
                    let Some((tier, new_state)) = tagged else { break };

                    // Terminal states always accepted immediately.
                    if matches!(new_state, AgentState::Exited { .. }) {
                        self.grace_timer.cancel();
                        grace_proposed = None;
                        set_grace_deadline(&grace_deadline, None);
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
                        // UNLESS a generic PermissionPrompt would overwrite
                        // a more specific PlanPrompt or Question from the
                        // same tier (Claude fires both notification and
                        // pre_tool_use hooks for the same prompt moment).
                        if tier == current_tier
                            && prompt_supersedes(&current_state, &new_state)
                        {
                            continue;
                        }
                        self.grace_timer.cancel();
                        grace_proposed = None;
                        set_grace_deadline(&grace_deadline, None);
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier }).await;
                    } else if is_idle_state(&new_state) && !is_idle_state(&current_state) {
                        // Lower confidence tier reporting idle: start grace timer.
                        let activity = (activity_fn)();
                        self.grace_timer.trigger(activity);
                        let deadline = std::time::Instant::now() + self.grace_timer.duration;
                        set_grace_deadline(&grace_deadline, Some(deadline));
                        grace_proposed = Some((tier, new_state));
                        debug!(tier, "grace timer started for idle transition");
                    } else {
                        // Lower confidence tier reporting non-idle change.
                        self.grace_timer.cancel();
                        grace_proposed = None;
                        set_grace_deadline(&grace_deadline, None);
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier }).await;
                    }
                }
                _ = grace_check_interval.tick(), if self.grace_timer.is_pending() => {
                    let activity = (activity_fn)();
                    match self.grace_timer.check(activity) {
                        GraceCheck::Confirmed => {
                            self.grace_timer.cancel();
                            set_grace_deadline(&grace_deadline, None);
                            if let Some((tier, state)) = grace_proposed.take() {
                                if state != current_state {
                                    debug!(tier, "grace timer confirmed idle");
                                    current_state = state.clone();
                                    current_tier = tier;
                                    let _ = output_tx.send(DetectedState { state, tier }).await;
                                }
                            }
                        }
                        GraceCheck::Invalidated => {
                            debug!("grace timer invalidated (activity detected)");
                            self.grace_timer.cancel();
                            grace_proposed = None;
                            set_grace_deadline(&grace_deadline, None);
                        }
                        GraceCheck::Waiting | GraceCheck::NotPending => {}
                    }
                }
            }
        }
    }
}

/// Returns `true` for states that represent an idle / waiting agent.
fn is_idle_state(state: &AgentState) -> bool {
    matches!(state, AgentState::WaitingForInput)
}

/// Returns `true` when `current` is a specific prompt state that should not
/// be overwritten by the more generic `incoming` prompt from the same tier.
///
/// `PlanPrompt` and `Question` carry richer context than `PermissionPrompt`.
/// When the agent fires both a specific pre-tool-use event and a generic
/// permission notification for the same user-facing moment, the specific
/// state should stick.
fn prompt_supersedes(current: &AgentState, incoming: &AgentState) -> bool {
    matches!(incoming, AgentState::PermissionPrompt { .. })
        && matches!(
            current,
            AgentState::PlanPrompt { .. } | AgentState::Question { .. }
        )
}

fn set_grace_deadline(
    deadline: &parking_lot::Mutex<Option<std::time::Instant>>,
    value: Option<std::time::Instant>,
) {
    *deadline.lock() = value;
}

impl std::fmt::Debug for CompositeDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeDetector")
            .field("tiers", &self.tiers.len())
            .field("grace_timer", &self.grace_timer)
            .finish()
    }
}

#[cfg(test)]
#[path = "composite_tests.rs"]
mod composite_tests;
