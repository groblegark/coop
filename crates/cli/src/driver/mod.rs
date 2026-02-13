// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod claude;
pub mod composite;
pub mod error_category;
pub mod gemini;
pub mod hook_detect;
pub mod hook_recv;
pub mod jsonl_stdout;
pub mod log_watch;
pub mod nudge;
pub mod process;
pub mod screen_parse;
pub mod stdout_detect;
pub mod unknown;

pub use composite::{CompositeDetector, DetectedState};
pub use error_category::{classify_error_detail, ErrorCategory};

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::event::{RawHookEvent, RawMessageEvent};
use crate::usage::UsageState;

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
    Idle,
    Prompt { prompt: PromptContext },
    Error { detail: String },
    Parked { reason: String, resume_at_epoch_ms: u64 },
    Switching,
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
    Setup,
}

impl PromptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Plan => "plan",
            Self::Question => "question",
            Self::Setup => "setup",
        }
    }
}

/// Contextual information about a prompt the agent is presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptContext {
    /// Prompt type: permission, plan, question, setup.
    #[serde(rename = "type")]
    pub kind: PromptKind,
    /// Prompt subtype for further classification within a kind.
    ///
    /// Known subtypes by kind:
    /// - **permission**: `"trust"` (workspace trust), `"tool"` (tool permission)
    /// - **setup**: `"theme_picker"`, `"terminal_setup"`, `"security_notes"`,
    ///   `"login_success"`, `"login_method"`, `"oauth_login"`
    /// - **plan**, **question**: (none currently)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    pub tool: Option<String>,
    pub input: Option<String>,
    /// Numbered option labels parsed from the terminal screen (permission/plan prompts).
    #[serde(default)]
    pub options: Vec<String>,
    /// True when `options` contains fallback labels (parser couldn't find real options).
    #[serde(default)]
    pub options_fallback: bool,
    /// All questions in a multi-question dialog.
    #[serde(default)]
    pub questions: Vec<QuestionContext>,
    /// 0-indexed active question; == questions.len() means confirm phase.
    #[serde(default)]
    pub question_current: usize,
    /// True when all async enrichment (e.g. option parsing) is complete.
    /// Permission/Plan prompts start `false` until enrichment finishes;
    /// Question and Setup prompts are immediately `true`.
    #[serde(default)]
    pub ready: bool,
}

impl PromptContext {
    /// Create a new `PromptContext` with the given kind and default fields.
    pub fn new(kind: PromptKind) -> Self {
        Self {
            kind,
            subtype: None,
            tool: None,
            input: None,
            options: vec![],
            options_fallback: false,
            questions: vec![],
            question_current: 0,
            ready: false,
        }
    }

    pub fn with_subtype(mut self, s: impl Into<String>) -> Self {
        self.subtype = Some(s.into());
        self
    }

    pub fn with_tool(mut self, t: impl Into<String>) -> Self {
        self.tool = Some(t.into());
        self
    }

    pub fn with_input(mut self, i: impl Into<String>) -> Self {
        self.input = Some(i.into());
        self
    }

    pub fn with_options(mut self, o: Vec<String>) -> Self {
        self.options = o;
        self
    }

    pub fn with_options_fallback(mut self) -> Self {
        self.options_fallback = true;
        self
    }

    pub fn with_questions(mut self, q: Vec<QuestionContext>) -> Self {
        self.questions = q;
        self
    }

    pub fn with_ready(mut self) -> Self {
        self.ready = true;
        self
    }
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

/// Emission from a detector: `(state, cause, tier_override)`.
///
/// The optional `tier_override` lets a detector emit at a different tier
/// than its default for high-confidence signals (e.g. a Tier 2 log detector
/// emitting at Tier 1 when it sees a definitive interrupt entry).
pub type DetectorEmission = (AgentState, String, Option<u8>);

/// A state detection source that monitors structured data and emits
/// [`AgentState`] transitions.
///
/// Object-safe for use as `Box<dyn Detector>`.
pub trait Detector: Send + 'static {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<DetectorEmission>,
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
    fn encode_permission(&self, option: u32) -> Vec<NudgeStep>;
    fn encode_plan(&self, option: u32, feedback: Option<&str>) -> Vec<NudgeStep>;
    fn encode_question(&self, answers: &[QuestionAnswer], total_questions: usize)
        -> Vec<NudgeStep>;
    fn encode_setup(&self, option: u32) -> Vec<NudgeStep>;
}

/// Lifecycle events for hook integrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    TurnStart,
    TurnEnd,
    ToolAfter { tool: String },
    ToolBefore { tool: String, tool_input: Option<serde_json::Value> },
    Notification { notification_type: String },
}

/// Driver-provided function that parses numbered option labels from rendered
/// screen lines. Used by the session's prompt enrichment loop.
pub type OptionParser = Arc<dyn Fn(&[String]) -> Vec<String> + Send + Sync>;

/// Channels for detectors to broadcast observations to the transport layer.
#[derive(Default)]
pub struct DetectorSinks {
    /// Shared last assistant message text (written by log/stdout detectors).
    pub last_message: Option<Arc<RwLock<Option<String>>>>,
    /// Raw hook event JSON from hook detectors.
    pub raw_hook_tx: Option<broadcast::Sender<RawHookEvent>>,
    /// Raw agent message JSON from log/stdout detectors.
    pub raw_message_tx: Option<broadcast::Sender<RawMessageEvent>>,
    /// Structured stdout JSONL from the agent process (Tier 3).
    pub stdout_rx: Option<mpsc::Receiver<Bytes>>,
    /// Usage tracking state (token counts, cost).
    pub usage: Option<Arc<UsageState>>,
}

impl DetectorSinks {
    pub fn with_last_message(mut self, lm: Arc<RwLock<Option<String>>>) -> Self {
        self.last_message = Some(lm);
        self
    }

    pub fn with_hook_tx(mut self, tx: broadcast::Sender<RawHookEvent>) -> Self {
        self.raw_hook_tx = Some(tx);
        self
    }

    pub fn with_message_tx(mut self, tx: broadcast::Sender<RawMessageEvent>) -> Self {
        self.raw_message_tx = Some(tx);
        self
    }

    pub fn with_stdout_rx(mut self, rx: mpsc::Receiver<Bytes>) -> Self {
        self.stdout_rx = Some(rx);
        self
    }

    pub fn with_usage(mut self, u: Arc<UsageState>) -> Self {
        self.usage = Some(u);
        self
    }
}

/// Compute a scaled nudge delay based on message length.
///
/// For short messages (≤256 bytes), returns the base delay.
/// For longer messages, adds `per_byte` for each byte beyond 256.
pub fn compute_nudge_delay(base: Duration, per_byte: Duration, len: usize) -> Duration {
    let extra_bytes = len.saturating_sub(256);
    base + per_byte * extra_bytes as u32
}

/// If this prompt is a disruption (safe to auto-dismiss), returns the
/// option number to select. Returns `None` for elicitations.
pub fn disruption_option(prompt: &PromptContext) -> Option<u32> {
    match prompt.kind {
        PromptKind::Setup => match prompt.subtype.as_deref() {
            Some("security_notes") => Some(1),
            Some("login_success") => Some(1),
            Some("terminal_setup") => Some(1),
            Some("theme_picker") => Some(1),
            Some("settings_error") => Some(2), // "Continue without these settings"
            _ => None, // oauth_login, login_method, startup_* = elicitations
        },
        PromptKind::Permission => match prompt.subtype.as_deref() {
            Some("trust") => Some(1), // "Yes, I trust this folder"
            _ => None,
        },
        _ => None,
    }
}

impl AgentState {
    /// Return the wire-format string for this state (e.g. `"working"`,
    /// `"prompt"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Prompt { .. } => "prompt",
            Self::Error { .. } => "error",
            Self::Parked { .. } => "parked",
            Self::Switching => "switching",
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
    /// Starting(0) < Idle(1) < Error(2) < Working(3) < Prompt(4)
    /// ```
    ///
    /// `Unknown` is treated the same as `Starting` (lowest).
    /// `Exited` is handled separately (always accepted) and never compared.
    pub fn state_priority(&self) -> u8 {
        match self {
            Self::Starting | Self::Unknown => 0,
            Self::Idle => 1,
            Self::Error { .. } | Self::Parked { .. } => 2,
            Self::Working => 3,
            Self::Prompt { .. } => 4,
            Self::Switching | Self::Exited { .. } => 5,
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

/// Shared session setup produced by agent-specific setup modules.
///
/// Wraps the artifacts (FIFO path, log path, env vars, CLI args) that
/// `run::prepare()` needs to spawn the backend and build the driver.
/// Agent-specific setup functions return this type.
pub struct SessionSetup {
    /// Agent-specific session ID (UUID).
    pub session_id: String,
    /// Tier 1 FIFO path. `None` in pristine mode.
    pub hook_pipe_path: Option<PathBuf>,
    /// Tier 2 session log path. `None` for agents without log detection.
    pub session_log_path: Option<PathBuf>,
    /// Session artifact directory (settings, MCP config, FIFO).
    pub session_dir: PathBuf,
    /// Environment variables for the child process.
    pub env_vars: Vec<(String, String)>,
    /// Extra CLI arguments to append to the command.
    pub extra_args: Vec<String>,
}

/// Create and return a coop session directory for the given session ID.
///
/// Session artifacts (FIFO pipe, settings file) live at
/// `$XDG_STATE_HOME/coop/sessions/<session-id>/` (defaulting to
/// `~/.local/state/coop/sessions/<session-id>/`) so they survive for
/// debugging and session recovery.
pub fn coop_session_dir(session_id: &str) -> anyhow::Result<PathBuf> {
    let state_home = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.local/state")
    });
    let dir = PathBuf::from(state_home).join("coop").join("sessions").join(session_id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Return environment variables for hook communication with the agent child process.
pub fn hook_env_vars(pipe_path: &Path, coop_url: &str) -> Vec<(String, String)> {
    vec![
        ("COOP_HOOK_PIPE".to_string(), pipe_path.display().to_string()),
        ("COOP_URL".to_string(), coop_url.to_string()),
    ]
}

/// Bundle of driver outputs consumed by `run::prepare()` and `execute_switch()`.
pub struct DriverContext {
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    pub detectors: Vec<Box<dyn Detector>>,
    pub option_parser: Option<OptionParser>,
}

/// Build a Claude-specific driver (Tier 1 hooks + Tier 2 log watcher).
pub fn build_claude_driver(
    config: &crate::config::Config,
    setup: Option<&SessionSetup>,
    log_start_offset: u64,
    sinks: DetectorSinks,
) -> anyhow::Result<DriverContext> {
    let hook_pipe = setup.and_then(|s| s.hook_pipe_path.as_deref());
    let log_path = setup.and_then(|s| s.session_log_path.clone());
    let driver = claude::ClaudeDriver::new(config, hook_pipe, log_path, log_start_offset, sinks)?;
    Ok(DriverContext {
        nudge_encoder: Some(Arc::new(driver.nudge)),
        respond_encoder: Some(Arc::new(driver.respond)),
        detectors: driver.detectors,
        option_parser: Some(Arc::new(claude::screen::parse_options_from_screen)),
    })
}

/// Build a Gemini-specific driver (Tier 1 hooks + Tier 4 process monitor).
pub fn build_gemini_driver(
    config: &crate::config::Config,
    setup: Option<&SessionSetup>,
    child_pid_fn: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
    sinks: DetectorSinks,
) -> anyhow::Result<DriverContext> {
    let hook_path = setup.and_then(|s| s.hook_pipe_path.as_deref());
    let driver = gemini::GeminiDriver::new(config, hook_path, sinks)?;
    let mut detectors = driver.detectors;
    // Tier 4: ProcessMonitor fallback for basic Working/Exited detection
    detectors.push(Box::new(
        process::ProcessMonitor::new(child_pid_fn, ring_total_written_fn)
            .with_poll_interval(config.process_poll()),
    ));
    detectors.sort_by_key(|d| d.tier());
    Ok(DriverContext {
        nudge_encoder: Some(Arc::new(driver.nudge)),
        respond_encoder: Some(Arc::new(driver.respond)),
        detectors,
        option_parser: Some(Arc::new(gemini::screen::parse_options_from_screen)),
    })
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod driver_tests;
