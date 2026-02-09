// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared handler functions for HTTP, WebSocket, and gRPC transports.
//!
//! Each transport is a thin wire-format adapter: parse → call shared fn → serialize.
//! Business logic lives here to prevent behavioral divergence.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, QuestionAnswer};
use crate::error::ErrorCode;
use crate::event::InputEvent;
use crate::event::PtySignal;
use crate::transport::state::AppState;
use crate::transport::{deliver_steps, encode_response, keys_to_bytes, update_question_current};

// ---------------------------------------------------------------------------
// Shared result types
// ---------------------------------------------------------------------------

/// Health check result.
pub struct HealthInfo {
    pub status: &'static str,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub agent: String,
    pub terminal_cols: u16,
    pub terminal_rows: u16,
    pub ws_clients: i32,
    pub ready: bool,
}

/// Session status result.
pub struct SessionStatus {
    pub state: &'static str,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub exit_code: Option<i32>,
    pub screen_seq: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub ws_clients: i32,
}

/// Nudge delivery result.
#[derive(Debug)]
pub struct NudgeOutcome {
    pub delivered: bool,
    pub state_before: Option<String>,
    pub reason: Option<String>,
}

/// Respond delivery result.
#[derive(Debug)]
pub struct RespondOutcome {
    pub delivered: bool,
    pub prompt_type: Option<String>,
    pub reason: Option<String>,
}

/// Transport-agnostic question answer (shared across HTTP, WS, gRPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportQuestionAnswer {
    pub option: Option<i32>,
    pub text: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

/// Convert transport question answers to domain [`QuestionAnswer`] values.
pub fn to_domain_answers(answers: &[TransportQuestionAnswer]) -> Vec<QuestionAnswer> {
    answers
        .iter()
        .map(|a| QuestionAnswer { option: a.option.map(|o| o as u32), text: a.text.clone() })
        .collect()
}

// ---------------------------------------------------------------------------
// Handler functions
// ---------------------------------------------------------------------------

/// Determine session state string from agent state and child PID.
pub fn session_state_str(agent: &AgentState, child_pid: u32) -> &'static str {
    match agent {
        AgentState::Exited { .. } => "exited",
        _ => {
            if child_pid == 0 {
                "starting"
            } else {
                "running"
            }
        }
    }
}

/// Compute health info.
pub async fn compute_health(state: &AppState) -> HealthInfo {
    let snap = state.terminal.screen.read().await.snapshot();
    let pid = state.terminal.child_pid.load(Ordering::Relaxed);
    let uptime = state.config.started_at.elapsed().as_secs() as i64;
    let ready = state.ready.load(Ordering::Acquire);

    HealthInfo {
        status: "running",
        pid: if pid == 0 { None } else { Some(pid as i32) },
        uptime_secs: uptime,
        agent: state.config.agent.to_string(),
        terminal_cols: snap.cols,
        terminal_rows: snap.rows,
        ws_clients: state.lifecycle.ws_client_count.load(Ordering::Relaxed),
        ready,
    }
}

/// Compute session status.
pub async fn compute_status(state: &AppState) -> SessionStatus {
    let agent = state.driver.agent_state.read().await;
    let ring = state.terminal.ring.read().await;
    let screen = state.terminal.screen.read().await;
    let pid = state.terminal.child_pid.load(Ordering::Relaxed);
    let exit = state.terminal.exit_status.read().await;
    let bw = state.lifecycle.bytes_written.load(Ordering::Relaxed);

    SessionStatus {
        state: session_state_str(&agent, pid),
        pid: if pid == 0 { None } else { Some(pid as i32) },
        uptime_secs: state.config.started_at.elapsed().as_secs() as i64,
        exit_code: exit.as_ref().and_then(|e| e.code),
        screen_seq: screen.seq(),
        bytes_read: ring.total_written(),
        bytes_written: bw,
        ws_clients: state.lifecycle.ws_client_count.load(Ordering::Relaxed),
    }
}

/// Send a nudge to the agent.
///
/// Returns `Err` only for genuine errors (not ready, no driver).
/// Agent-busy is a soft failure returned as `Ok(NudgeOutcome { delivered: false })`.
pub async fn handle_nudge(state: &AppState, message: &str) -> Result<NudgeOutcome, ErrorCode> {
    if !state.ready.load(Ordering::Acquire) {
        return Err(ErrorCode::NotReady);
    }

    let encoder = match &state.config.nudge_encoder {
        Some(enc) => Arc::clone(enc),
        None => return Err(ErrorCode::NoDriver),
    };

    let _delivery = state.nudge_mutex.lock().await;

    let agent = state.driver.agent_state.read().await;
    let state_before = agent.as_str().to_owned();

    match &*agent {
        AgentState::WaitingForInput => {}
        _ => {
            return Ok(NudgeOutcome {
                delivered: false,
                state_before: Some(state_before.clone()),
                reason: Some(format!("agent is {state_before}")),
            });
        }
    }
    drop(agent);

    let steps = encoder.encode(message);
    let _ = deliver_steps(&state.channels.input_tx, steps).await;

    Ok(NudgeOutcome { delivered: true, state_before: Some(state_before), reason: None })
}

/// Respond to an active prompt.
///
/// Returns `Err` only for genuine errors (not ready, no driver).
/// No-prompt is a soft failure returned as `Ok(RespondOutcome { delivered: false })`.
pub async fn handle_respond(
    state: &AppState,
    accept: Option<bool>,
    option: Option<i32>,
    text: Option<&str>,
    answers: &[TransportQuestionAnswer],
) -> Result<RespondOutcome, ErrorCode> {
    if !state.ready.load(Ordering::Acquire) {
        return Err(ErrorCode::NotReady);
    }

    let encoder = match &state.config.respond_encoder {
        Some(enc) => Arc::clone(enc),
        None => return Err(ErrorCode::NoDriver),
    };

    let domain_answers = to_domain_answers(answers);
    let resolved_option = option.map(|o| o as u32);

    let _delivery = state.nudge_mutex.lock().await;

    let agent = state.driver.agent_state.read().await;
    let prompt_type = agent.prompt().map(|p| p.kind.as_str().to_owned());

    let (steps, answers_delivered) = match encode_response(
        &agent,
        encoder.as_ref(),
        accept,
        resolved_option,
        text,
        &domain_answers,
    ) {
        Ok(r) => r,
        Err(_code) => {
            return Ok(RespondOutcome {
                delivered: false,
                prompt_type: None,
                reason: Some("no prompt active".to_owned()),
            });
        }
    };

    drop(agent);
    let _ = deliver_steps(&state.channels.input_tx, steps).await;

    if answers_delivered > 0 {
        update_question_current(state, answers_delivered).await;
    }

    Ok(RespondOutcome { delivered: true, prompt_type, reason: None })
}

/// Write text to the PTY, optionally followed by a carriage return.
pub async fn handle_input(state: &AppState, text: String, enter: bool) -> i32 {
    let mut data = text.into_bytes();
    if enter {
        data.push(b'\r');
    }
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    len
}

/// Write raw bytes to the PTY.
pub async fn handle_input_raw(state: &AppState, data: Vec<u8>) -> i32 {
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    len
}

/// Send named key sequences to the PTY.
///
/// Returns the byte count on success, or the unrecognised key name on failure.
pub async fn handle_keys(state: &AppState, keys: &[String]) -> Result<i32, String> {
    let data = keys_to_bytes(keys)?;
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    Ok(len)
}

/// Resize the PTY.
pub async fn handle_resize(state: &AppState, cols: u16, rows: u16) -> Result<(), ErrorCode> {
    if cols == 0 || rows == 0 {
        return Err(ErrorCode::BadRequest);
    }
    let _ = state.channels.input_tx.send(InputEvent::Resize { cols, rows }).await;
    Ok(())
}

/// Send a signal to the child process.
///
/// Returns `Ok(())` on success, or the unknown signal name on failure.
pub async fn handle_signal(state: &AppState, signal: &str) -> Result<(), String> {
    let sig = PtySignal::from_name(signal).ok_or_else(|| signal.to_owned())?;
    let _ = state.channels.input_tx.send(InputEvent::Signal(sig)).await;
    Ok(())
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
