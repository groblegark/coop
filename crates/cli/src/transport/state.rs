// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{
    AgentState, AgentType, ErrorCategory, ExitStatus, NudgeEncoder, RespondEncoder,
};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::stop::StopState;

/// Shared application state passed to all handlers via axum `State` extractor.
///
/// Organized into focused sub-structs by concern:
/// - `terminal`: screen, ring buffer, child process
/// - `driver`: agent detection state
/// - `channels`: channel endpoints for session ↔ transport communication
/// - `config`: static session settings
/// - `lifecycle`: runtime lifecycle primitives
pub struct AppState {
    pub terminal: Arc<TerminalState>,
    pub driver: Arc<DriverState>,
    pub channels: TransportChannels,
    pub config: SessionSettings,
    pub lifecycle: LifecycleState,

    /// Whether the agent has transitioned out of `Starting` and is ready.
    pub ready: Arc<AtomicBool>,
    /// Serializes multi-step nudge/respond delivery sequences.
    /// The mutex covers state check + delivery to prevent double-nudge races.
    pub nudge_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Stop hook gating state. Always present (defaults to mode=allow).
    pub stop: Arc<StopState>,
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

/// Driver detection state.
pub struct DriverState {
    pub agent_state: RwLock<AgentState>,
    pub state_seq: AtomicU64,
    pub detection_tier: AtomicU8,
    /// Arc-wrapped so the session loop can pass a cheap clone to
    /// [`CompositeDetector::run`] without holding a reference to the
    /// entire `DriverState`.
    pub idle_grace_deadline: Arc<Mutex<Option<Instant>>>,
    /// Error detail string when agent is in `Error` state, `None` otherwise.
    pub error_detail: RwLock<Option<String>>,
    /// Classified error category when agent is in `Error` state, `None` otherwise.
    pub error_category: RwLock<Option<ErrorCategory>>,
}

impl DriverState {
    /// Format the current detection tier as a display string.
    pub fn detection_tier_str(&self) -> String {
        let tier = self
            .detection_tier
            .load(std::sync::atomic::Ordering::Relaxed);
        if tier == u8::MAX {
            "none".to_owned()
        } else {
            tier.to_string()
        }
    }

    /// Compute the remaining seconds on the idle grace timer, if any.
    pub fn idle_grace_remaining_secs(&self) -> Option<f32> {
        let deadline = self.idle_grace_deadline.lock();
        deadline.map(|dl| {
            let now = Instant::now();
            if now < dl {
                (dl - now).as_secs_f32()
            } else {
                0.0
            }
        })
    }
}

/// Channel endpoints for consumer ↔ session communication.
pub struct TransportChannels {
    pub input_tx: mpsc::Sender<InputEvent>,
    pub output_tx: broadcast::Sender<OutputEvent>,
    pub state_tx: broadcast::Sender<StateChangeEvent>,
}

/// Static session configuration (immutable after construction).
pub struct SessionSettings {
    pub started_at: Instant,
    pub agent: AgentType,
    pub auth_token: Option<String>,
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    pub idle_grace_duration: Duration,
}

/// Runtime lifecycle primitives.
pub struct LifecycleState {
    pub shutdown: CancellationToken,
    pub ws_client_count: AtomicI32,
    pub bytes_written: AtomicU64,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agent", &self.config.agent)
            .field("auth_token", &self.config.auth_token.is_some())
            .finish()
    }
}
