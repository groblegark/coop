// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! State transition helpers extracted from the session select-loop.
//!
//! Each function is independently testable — it takes the shared [`Store`]
//! plus the minimal set of arguments it needs.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::sync::mpsc;
use tracing::debug;

use crate::config::Config;
use crate::driver::{
    classify_error_detail, AgentState, DetectedState, ErrorCategory, ExitStatus, OptionParser,
    PromptKind,
};
use crate::event::{OutputEvent, TransitionEvent};
use crate::profile::RotateOutcome;
use crate::transport::Store;

use super::groom;
use super::run::SessionState;

/// Action returned by [`process_detected_state`] telling the select-loop what to do.
pub enum DetectAction {
    /// Continue looping.
    Continue,
    /// Break out of the select-loop.
    Break,
}

/// Feed raw backend output into the ring buffer, screen, and broadcast channel.
pub async fn feed_output(store: &Store, bytes: &Bytes) {
    // Write to ring buffer and stamp offset while holding the lock.
    let offset;
    {
        let mut ring = store.terminal.ring.write().await;
        ring.write(bytes);
        offset = ring.total_written() - bytes.len() as u64;
        store
            .terminal
            .ring_total_written
            .store(ring.total_written(), std::sync::atomic::Ordering::Relaxed);
    }
    // Feed screen
    {
        let mut screen = store.terminal.screen.write().await;
        screen.feed(bytes);
    }
    // Broadcast raw output with stamped offset
    let _ = store.channels.output_tx.send(OutputEvent::Raw { data: bytes.clone(), offset });
}

/// Process a detected state change from the composite detector.
///
/// Updates agent_state, handles rate-limiting, broadcasts the transition,
/// spawns prompt enrichment/auto-dismiss, and tracks idle time.
///
/// Returns a [`DetectAction`] telling the select-loop whether to continue or break.
pub async fn process_detected_state(
    store: &Arc<Store>,
    detected: DetectedState,
    session: &mut SessionState,
    option_parser: &Option<OptionParser>,
    config: &Config,
) -> DetectAction {
    session.state_seq += 1;
    let mut current = store.driver.agent_state.write().await;
    let prev = current.clone();
    *current = detected.state.clone();
    drop(current);
    session.last_state = detected.state.clone();

    // Mark ready on first transition away from Starting.
    if matches!(prev, AgentState::Starting) && !matches!(detected.state, AgentState::Starting) {
        store.ready.store(true, std::sync::atomic::Ordering::Release);
    }

    // Store error detail + category when entering Error state.
    if let AgentState::Error { ref detail } = detected.state {
        let category = classify_error_detail(detail);
        *store.driver.error.write().await =
            Some(crate::transport::state::ErrorInfo { detail: detail.clone(), category });

        // Auto-rotate on rate limit when profiles are registered.
        if category == ErrorCategory::RateLimited {
            handle_rate_limit(Arc::clone(store), session).await;
        }
    } else {
        *store.driver.error.write().await = None;
    }

    // Store metadata for the HTTP/gRPC API.
    store.driver.state_seq.store(session.state_seq, std::sync::atomic::Ordering::Release);
    *store.driver.detection.write().await = crate::transport::state::DetectionInfo {
        tier: detected.tier,
        cause: detected.cause.clone(),
    };

    let last_message = store.driver.last_message.read().await.clone();
    let _ = store.channels.state_tx.send(TransitionEvent {
        prev,
        next: detected.state.clone(),
        seq: session.state_seq,
        cause: detected.cause,
        last_message,
    });

    // Spawn deferred option enrichment for Permission/Plan prompts.
    if let AgentState::Prompt { ref prompt } = detected.state {
        if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
            if let Some(ref parser) = option_parser {
                groom::spawn_enrichment(store, session.state_seq, parser);
            }
        }
    }

    // Auto-dismiss disruption prompts in groom=auto mode.
    if let AgentState::Prompt { ref prompt } = detected.state {
        groom::spawn_auto_dismiss(store, prompt, config, session.state_seq);
    }

    // Track idle time for idle_timeout.
    if matches!(detected.state, AgentState::Idle) && session.idle_timeout > Duration::ZERO {
        if session.idle_since.is_none() {
            session.idle_since = Some(tokio::time::Instant::now());
        }
    } else {
        session.idle_since = None;
    }

    // Switch check: agent reached idle during pending switch → SIGHUP now.
    if session.pending_switch.is_some() && matches!(detected.state, AgentState::Idle) {
        debug!("switch: agent reached idle, sending SIGHUP");
        let cause = if session.pending_switch.as_ref().is_some_and(|r| r.credentials.is_some()) {
            "switch"
        } else {
            "restart"
        };
        broadcast_restarting(store, session, cause).await;
        sighup_child_group(store);
    }

    // Drain check: agent reached idle during drain → kill now.
    if session.drain_deadline.is_some() && matches!(detected.state, AgentState::Idle) {
        debug!("drain: agent reached idle, sending SIGHUP");
        sighup_child_group(store);
        return DetectAction::Break;
    }

    DetectAction::Continue
}

/// Handle a rate-limit error by attempting profile rotation or parking.
async fn handle_rate_limit(store: Arc<Store>, session: &mut SessionState) {
    match store.profile.try_auto_rotate().await {
        RotateOutcome::Switch(req) => {
            let _ = store.switch.switch_tx.try_send(req);
        }
        RotateOutcome::Exhausted { retry_after } => {
            let resume_at = now_epoch_ms() + retry_after.as_millis() as u64;
            let parked = AgentState::Parked {
                reason: "all_profiles_rate_limited".into(),
                resume_at_epoch_ms: resume_at,
            };
            session.state_seq += 1;
            let mut current = store.driver.agent_state.write().await;
            let prev = current.clone();
            *current = parked.clone();
            drop(current);
            session.last_state = parked.clone();
            store.driver.state_seq.store(session.state_seq, std::sync::atomic::Ordering::Release);
            let last_message = store.driver.last_message.read().await.clone();
            let _ = store.channels.state_tx.send(TransitionEvent {
                prev,
                next: parked,
                seq: session.state_seq,
                cause: "all_profiles_rate_limited".to_owned(),
                last_message,
            });
            store.profile.schedule_retry(retry_after, store.clone());
        }
        RotateOutcome::Skipped => {}
    }
}

/// Broadcast an `AgentState::Restarting` transition and update tracking state.
pub async fn broadcast_restarting(store: &Store, session: &mut SessionState, cause: &str) {
    session.state_seq += 1;
    let mut current = store.driver.agent_state.write().await;
    let prev = current.clone();
    *current = AgentState::Restarting;
    drop(current);
    session.last_state = AgentState::Restarting;
    let last_message = store.driver.last_message.read().await.clone();
    let _ = store.channels.state_tx.send(TransitionEvent {
        prev,
        next: AgentState::Restarting,
        seq: session.state_seq,
        cause: cause.to_owned(),
        last_message,
    });
}

/// Drain any pending output so final bytes are captured.
pub async fn drain_pending_output(store: &Store, rx: &mut mpsc::Receiver<Bytes>) {
    while let Ok(bytes) = rx.try_recv() {
        feed_output(store, &bytes).await;
    }
}

/// Store exit status and broadcast the `Exited` state transition.
pub async fn broadcast_exit(store: &Store, status: ExitStatus, state_seq: &mut u64) {
    // ORDERING: exit_status must be written before agent_state so that
    // any reader who observes AgentState::Exited is guaranteed to find
    // exit_status populated.
    {
        let mut exit = store.terminal.exit_status.write().await;
        *exit = Some(status);
    }
    let mut current = store.driver.agent_state.write().await;
    let prev = current.clone();
    *current = AgentState::Exited { status };
    drop(current);
    *state_seq += 1;
    let last_message = store.driver.last_message.read().await.clone();
    let _ = store.channels.state_tx.send(TransitionEvent {
        prev,
        next: AgentState::Exited { status },
        seq: *state_seq,
        cause: String::new(),
        last_message,
    });
}

/// Send SIGHUP to the child process group.
pub fn sighup_child_group(store: &Store) {
    let pid = store.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
    if pid != 0 {
        let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGHUP);
    }
}

/// Return the current UTC time as milliseconds since the Unix epoch.
pub fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
