// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, AgentType, ExitStatus, NudgeEncoder, RespondEncoder};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;

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
    /// The write lock gates *access* (single writer), while the nudge mutex
    /// gates *delivery* (atomic multi-step sequences).
    pub nudge_mutex: Arc<tokio::sync::Mutex<()>>,
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
    pub write_lock: Arc<WriteLock>,
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

/// Auto-expiry duration for the write lock.
const WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Owner identifier for the write lock.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LockOwner {
    Http,
    Ws(String),
}

/// Internal state for the write lock.
#[derive(Debug)]
struct LockInner {
    owner: Option<LockOwner>,
    acquired_at: Option<Instant>,
}

/// Single-writer concurrency primitive with 30-second auto-release.
///
/// HTTP POST endpoints acquire and release atomically (within a single request).
/// WebSocket clients acquire via `Lock { action: "acquire" }` and release via
/// `Lock { action: "release" }`. The lock auto-expires after 30 seconds.
#[derive(Debug)]
pub struct WriteLock {
    inner: Mutex<LockInner>,
}

/// Guard that releases the HTTP write lock when dropped.
pub struct WriteLockGuard<'a> {
    lock: &'a WriteLock,
}

impl Drop for WriteLockGuard<'_> {
    fn drop(&mut self) {
        let mut inner = self.lock.lock_inner();
        if inner.owner == Some(LockOwner::Http) {
            inner.owner = None;
            inner.acquired_at = None;
        }
    }
}

impl WriteLock {
    /// Create a new unlocked `WriteLock`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LockInner {
                owner: None,
                acquired_at: None,
            }),
        }
    }

    /// Acquire the inner mutex and expire stale locks.
    fn lock_inner(&self) -> parking_lot::MutexGuard<'_, LockInner> {
        let mut inner = self.inner.lock();
        if let Some(acquired_at) = inner.acquired_at {
            if acquired_at.elapsed() >= WRITE_LOCK_TIMEOUT {
                inner.owner = None;
                inner.acquired_at = None;
            }
        }
        inner
    }

    /// Acquire the write lock for an HTTP request. Returns a guard that
    /// auto-releases on drop. Returns `WriterBusy` if held by another owner.
    pub fn acquire_http(&self) -> Result<WriteLockGuard<'_>, ErrorCode> {
        let mut inner = self.lock_inner();
        if inner.owner.is_some() {
            return Err(ErrorCode::WriterBusy);
        }
        inner.owner = Some(LockOwner::Http);
        inner.acquired_at = Some(Instant::now());
        Ok(WriteLockGuard { lock: self })
    }

    /// Acquire the write lock for a WebSocket client. Returns `WriterBusy` if
    /// held by another owner.
    pub fn acquire_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = self.lock_inner();
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => return Ok(()),
            Some(_) => return Err(ErrorCode::WriterBusy),
            None => {}
        }
        inner.owner = Some(LockOwner::Ws(client_id.to_owned()));
        inner.acquired_at = Some(Instant::now());
        Ok(())
    }

    /// Release the write lock for a WebSocket client. Returns `WriterBusy`
    /// if the client is not the current owner.
    pub fn release_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = self.lock_inner();
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => {
                inner.owner = None;
                inner.acquired_at = None;
                Ok(())
            }
            Some(_) => Err(ErrorCode::WriterBusy),
            None => Ok(()),
        }
    }

    /// Check that the given WebSocket client currently holds the write lock.
    /// Returns `WriterBusy` if the client does not hold the lock.
    pub fn check_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let inner = self.lock_inner();
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => Ok(()),
            _ => Err(ErrorCode::WriterBusy),
        }
    }

    /// Force-release the lock if held by the given WebSocket client.
    /// Used during connection cleanup.
    pub fn force_release_ws(&self, client_id: &str) {
        let mut inner = self.lock_inner();
        if inner.owner == Some(LockOwner::Ws(client_id.to_owned())) {
            inner.owner = None;
            inner.acquired_at = None;
        }
    }

    /// Check if the lock is currently held (after expiry check).
    pub fn is_held(&self) -> bool {
        self.lock_inner().owner.is_some()
    }
}

impl Default for WriteLock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
