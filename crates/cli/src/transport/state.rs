// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::GroomLevel;
use crate::driver::{
    AgentState, AgentType, ErrorCategory, ExitStatus, NudgeEncoder, RespondEncoder,
};
use crate::event::{
    InputEvent, OutputEvent, PromptOutcome, RawHookEvent, RawMessageEvent, TransitionEvent,
};
use crate::event_log::EventLog;
use crate::profile::ProfileState;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::start::StartState;
use crate::stop::StopState;
use crate::switch::SwitchState;
use crate::transcript::TranscriptState;
use crate::usage::UsageState;

/// Shared application state passed to all handlers via axum `State` extractor.
///
/// Organized into focused sub-structs by concern:
/// - `terminal`: screen, ring buffer, child process
/// - `driver`: agent detection state
/// - `channels`: channel endpoints for session ↔ transport communication
/// - `config`: static session settings
/// - `lifecycle`: runtime lifecycle primitives
pub struct Store {
    pub terminal: Arc<TerminalState>,
    pub driver: Arc<DriverState>,
    pub channels: TransportChannels,
    pub config: SessionSettings,
    pub lifecycle: LifecycleState,

    /// Agent session ID (UUID). Updated on switch.
    pub session_id: RwLock<String>,
    /// Whether the agent has transitioned out of `Starting` and is ready.
    pub ready: Arc<AtomicBool>,
    /// Stop hook gating state. Always present (defaults to mode=allow).
    pub stop: Arc<StopState>,
    /// Session switch state. Always present.
    pub switch: Arc<SwitchState>,
    /// Start hook state. Always present (defaults to empty config).
    pub start: Arc<StartState>,
    /// Transcript snapshot state. Always present.
    pub transcript: Arc<TranscriptState>,
    /// Per-session API usage tracking. Always present.
    pub usage: Arc<UsageState>,
    /// Named credential profiles for rotation. Always present (defaults to empty).
    pub profile: Arc<ProfileState>,
    /// Pending environment variable overrides.  Written by `PUT /api/v1/env/:key`,
    /// merged into the child's environment on the next session switch.
    pub pending_env: RwLock<HashMap<String, String>>,
    /// Serializes structured input delivery (nudge, respond) and enforces
    /// a minimum inter-delivery gap to prevent garbled terminal input.
    pub input_gate: Arc<InputGate>,
    /// Notified by the session loop whenever any `InputEvent` is processed.
    /// Used by the enter-retry monitor to cancel itself if other input
    /// activity occurs on the PTY (e.g. raw keys, resize, signal, new delivery).
    pub input_activity: Arc<tokio::sync::Notify>,
    /// File-backed event log for state/hook event catchup on WS reconnect.
    pub event_log: Arc<EventLog>,
}

/// Terminal I/O: screen, ring buffer, child process.
pub struct TerminalState {
    pub screen: RwLock<Screen>,
    pub ring: RwLock<RingBuffer>,
    /// Lock-free mirror of `ring.total_written()` updated by the session loop.
    ///
    /// Arc-wrapped so the session loop can hand a cheap clone to the
    /// [`CompositeDetector`] activity callback without holding a reference
    /// to the entire `TerminalState`.
    pub ring_total_written: Arc<AtomicU64>,
    pub child_pid: AtomicU32,
    /// ORDERING: must be written before `DriverState.agent_state` is set to
    /// `Exited` so readers who see the exited state always find this populated.
    pub exit_status: RwLock<Option<ExitStatus>>,
}

impl TerminalState {
    /// Build a closure that captures `self` and returns a screen snapshot.
    ///
    /// Used by detectors that need periodic screen access without holding a
    /// reference to the full `TerminalState`.
    pub fn snapshot_fn(
        self: &Arc<Self>,
    ) -> Arc<dyn Fn() -> crate::screen::ScreenSnapshot + Send + Sync> {
        let terminal = Arc::clone(self);
        Arc::new(move || {
            terminal.screen.try_read().map(|s| s.snapshot()).unwrap_or_else(|_| {
                crate::screen::ScreenSnapshot {
                    lines: vec![],
                    cols: 0,
                    rows: 0,
                    alt_screen: false,
                    cursor: crate::screen::CursorPosition { row: 0, col: 0 },
                    sequence: 0,
                }
            })
        })
    }

    /// Build a closure that returns the child PID (or `None` if not yet spawned).
    pub fn child_pid_fn(self: &Arc<Self>) -> Arc<dyn Fn() -> Option<u32> + Send + Sync> {
        let terminal = Arc::clone(self);
        Arc::new(move || {
            let v = terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        })
    }

    /// Build a closure that returns the current `ring_total_written` value.
    pub fn ring_total_written_fn(self: &Arc<Self>) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let rtw = Arc::clone(&self.ring_total_written);
        Arc::new(move || rtw.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Reset terminal state for a new session iteration (switch).
    pub async fn reset(&self, cols: u16, rows: u16, ring_size: usize) {
        *self.screen.write().await = Screen::new(cols, rows);
        {
            let mut ring = self.ring.write().await;
            *ring = RingBuffer::new(ring_size);
            self.ring_total_written.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        self.child_pid.store(0, std::sync::atomic::Ordering::Release);
        *self.exit_status.write().await = None;
    }
}

impl DriverState {
    /// Reset driver state for a new session iteration (switch).
    pub async fn reset(&self) {
        *self.agent_state.write().await = AgentState::Starting;
        self.state_seq.store(0, std::sync::atomic::Ordering::Release);
        *self.detection.write().await = DetectionInfo { tier: u8::MAX, cause: String::new() };
        *self.error.write().await = None;
        *self.last_message.write().await = None;
    }
}

/// Classified error detail and category, stored atomically under a single lock.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub detail: String,
    pub category: ErrorCategory,
}

/// Detection tier and cause, stored atomically under a single lock.
#[derive(Debug, Clone)]
pub struct DetectionInfo {
    pub tier: u8,
    pub cause: String,
}

impl DetectionInfo {
    /// Format the detection tier as a display string.
    pub fn tier_str(&self) -> String {
        if self.tier == u8::MAX {
            "none".to_owned()
        } else {
            self.tier.to_string()
        }
    }
}

/// Driver detection state.
pub struct DriverState {
    pub agent_state: RwLock<AgentState>,
    pub state_seq: AtomicU64,
    /// Detection tier and freeform cause string from the detector that produced
    /// the current state. Combined into a single lock to prevent readers from
    /// seeing a stale cause with a new tier (or vice versa).
    pub detection: RwLock<DetectionInfo>,
    /// Error detail + category when agent is in `Error` state, `None` otherwise.
    /// Combined into a single lock to prevent readers from seeing a torn state
    /// (e.g. detail=Some with category=None).
    pub error: RwLock<Option<ErrorInfo>>,
    /// Last assistant message text (concatenated text blocks from the most recent
    /// assistant JSONL entry). Written directly by log/stdout detectors.
    pub last_message: Arc<RwLock<Option<String>>>,
}

/// Channel endpoints for consumer ↔ session communication.
pub struct TransportChannels {
    pub input_tx: mpsc::Sender<InputEvent>,
    pub output_tx: broadcast::Sender<OutputEvent>,
    pub state_tx: broadcast::Sender<TransitionEvent>,
    pub prompt_tx: broadcast::Sender<PromptOutcome>,
    pub hook_tx: broadcast::Sender<RawHookEvent>,
    pub message_tx: broadcast::Sender<RawMessageEvent>,
}

/// Static session configuration (immutable after construction).
pub struct SessionSettings {
    pub started_at: Instant,
    pub agent: AgentType,
    pub auth_token: Option<String>,
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    /// Timeout for the enter-retry safety net after nudge delivery.
    /// `Duration::ZERO` disables the retry.
    pub nudge_timeout: Duration,
    /// How aggressively coop auto-responds to agent prompts.
    pub groom: GroomLevel,
}

/// Runtime lifecycle primitives.
pub struct LifecycleState {
    pub shutdown: CancellationToken,
    pub ws_client_count: AtomicI32,
    pub bytes_written: AtomicU64,
}

/// Serializes structured input delivery sequences (nudge, respond) and
/// enforces a minimum inter-delivery gap (debounce) to prevent garbled
/// terminal input when deliveries arrive in rapid succession.
///
/// Terminal-based agents can drop or mis-interpret keystrokes when input
/// arrives faster than the TUI can process.  The gate ensures at least
/// `debounce` time elapses between the end of one delivery and the start
/// of the next.
pub struct InputGate {
    lock: tokio::sync::Mutex<DeliveryGateInner>,
    debounce: Duration,
}

struct DeliveryGateInner {
    last_delivery: Option<Instant>,
    /// Cancel token for any pending enter-retry from the previous delivery.
    retry_cancel: Option<CancellationToken>,
}

impl InputGate {
    pub fn new(debounce: Duration) -> Self {
        Self {
            lock: tokio::sync::Mutex::new(DeliveryGateInner {
                last_delivery: None,
                retry_cancel: None,
            }),
            debounce,
        }
    }

    /// Acquire exclusive delivery access, sleeping if the minimum
    /// inter-delivery gap has not yet elapsed.
    ///
    /// Cancels any pending enter-retry from the previous delivery.
    pub async fn acquire(&self) -> DeliveryGuard<'_> {
        let guard = self.lock.lock().await;
        // Cancel pending retry from previous delivery
        if let Some(ref token) = guard.retry_cancel {
            token.cancel();
        }
        if let Some(last) = guard.last_delivery {
            let elapsed = last.elapsed();
            if elapsed < self.debounce {
                tokio::time::sleep(self.debounce - elapsed).await;
            }
        }
        DeliveryGuard { inner: guard }
    }
}

/// RAII guard returned by [`InputGate::acquire`].
///
/// Records the delivery completion timestamp on drop so that subsequent
/// acquisitions can enforce the debounce interval.
pub struct DeliveryGuard<'a> {
    inner: tokio::sync::MutexGuard<'a, DeliveryGateInner>,
}

impl DeliveryGuard<'_> {
    /// Store a cancellation token for the enter-retry monitor spawned
    /// after this delivery.  The next `acquire()` call will cancel it.
    pub fn set_retry_cancel(&mut self, token: CancellationToken) {
        self.inner.retry_cancel = Some(token);
    }
}

impl Drop for DeliveryGuard<'_> {
    fn drop(&mut self) {
        self.inner.last_delivery = Some(Instant::now());
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agent", &self.config.agent)
            .field("auth_token", &self.config.auth_token.is_some())
            .finish()
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
