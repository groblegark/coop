// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! API contract types and server implementation for HTTP and WebSocket transports.

pub mod auth;
pub mod grpc;
pub mod http;
pub mod state;
pub mod ws;

pub use state::AppState;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

use crate::driver::{AgentState, NudgeStep, PromptKind, QuestionAnswer, RespondEncoder};
use crate::error::ErrorCode;
use crate::event::InputEvent;

// ---------------------------------------------------------------------------
// Helpers (shared between HTTP and gRPC)
// ---------------------------------------------------------------------------

/// Translate a named key to its terminal escape sequence (case-insensitive).
pub fn encode_key(name: &str) -> Option<Vec<u8>> {
    let lower = name.to_lowercase();
    let bytes: &[u8] = match lower.as_str() {
        "enter" | "return" => b"\r",
        "tab" => b"\t",
        "escape" | "esc" => b"\x1b",
        "backspace" => b"\x7f",
        "delete" | "del" => b"\x1b[3~",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" | "page_up" => b"\x1b[5~",
        "pagedown" | "page_down" => b"\x1b[6~",
        "insert" => b"\x1b[2~",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        "space" => b" ",
        _ => {
            // Generic Ctrl-<letter> handler
            if let Some(ch_str) = lower.strip_prefix("ctrl-") {
                let ch = ch_str.chars().next()?;
                if ch.is_ascii_lowercase() {
                    let ctrl = (ch.to_ascii_uppercase() as u8).wrapping_sub(b'@');
                    return Some(vec![ctrl]);
                }
            }
            return None;
        }
    };
    Some(bytes.to_vec())
}

/// Send encoder steps to the PTY, respecting inter-step delays.
pub async fn deliver_steps(
    input_tx: &tokio::sync::mpsc::Sender<InputEvent>,
    steps: Vec<NudgeStep>,
) -> Result<(), ErrorCode> {
    for step in steps {
        input_tx
            .send(InputEvent::Write(bytes::Bytes::from(step.bytes)))
            .await
            .map_err(|_| ErrorCode::Internal)?;
        if let Some(delay) = step.delay_after {
            tokio::time::sleep(delay).await;
        }
    }
    Ok(())
}

/// Match the current agent state to the appropriate encoder call.
///
/// Returns `(steps, answers_delivered)` where `answers_delivered` is the
/// number of question answers that were encoded (for question_current tracking).
pub fn encode_response(
    agent: &AgentState,
    encoder: &dyn RespondEncoder,
    accept: Option<bool>,
    text: Option<&str>,
    answers: &[QuestionAnswer],
) -> Result<(Vec<NudgeStep>, usize), ErrorCode> {
    match agent {
        AgentState::Prompt { prompt } => match prompt.kind {
            PromptKind::Permission => Ok((encoder.encode_permission(accept.unwrap_or(false)), 0)),
            PromptKind::Plan => Ok((encoder.encode_plan(accept.unwrap_or(false), text), 0)),
            PromptKind::Question => {
                if answers.is_empty() {
                    return Ok((vec![], 0));
                }
                let total_questions = prompt.questions.len();
                let count = answers.len();
                Ok((encoder.encode_question(answers, total_questions), count))
            }
        },
        _ => Err(ErrorCode::NoPrompt),
    }
}

/// Advance `question_current` on the current `Question` state after answers
/// have been delivered to the PTY.
pub async fn update_question_current(state: &AppState, answers_delivered: usize) {
    let mut agent = state.driver.agent_state.write().await;
    if let AgentState::Prompt { ref mut prompt } = *agent {
        if prompt.kind != PromptKind::Question {
            return;
        }
        let prev_aq = prompt.question_current;
        prompt.question_current = prev_aq
            .saturating_add(answers_delivered)
            .min(prompt.questions.len());
        if prompt.question_current != prev_aq {
            let next = agent.clone();
            drop(agent);
            let seq = state
                .driver
                .state_seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Broadcast updated state so clients see question_current progress.
            let _ = state
                .channels
                .state_tx
                .send(crate::event::StateChangeEvent {
                    prev: next.clone(),
                    next,
                    seq,
                });
        }
    }
}

/// Read from the ring buffer starting at `offset`, combine wrapping slices,
/// and return the raw bytes.
pub fn read_ring_combined(ring: &crate::ring::RingBuffer, offset: u64) -> Vec<u8> {
    let (a, b) = ring.read_from(offset).unwrap_or((&[], &[]));
    [a, b].concat()
}

// ---------------------------------------------------------------------------
// Error response types
// ---------------------------------------------------------------------------

/// Top-level error response envelope shared across HTTP and WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

/// Error body containing a machine-readable code and human-readable message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ErrorCode {
    /// Convert this error code into a transport [`ErrorBody`].
    pub fn to_error_body(&self, message: impl Into<String>) -> ErrorBody {
        ErrorBody {
            code: self.as_str().to_owned(),
            message: message.into(),
        }
    }

    /// Convert this error code into an axum JSON error response.
    pub fn to_http_response(
        &self,
        message: impl Into<String>,
    ) -> (StatusCode, Json<ErrorResponse>) {
        let status =
            StatusCode::from_u16(self.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = ErrorResponse {
            error: self.to_error_body(message),
        };
        (status, Json(body))
    }
}

// ---------------------------------------------------------------------------
// Helpers (shared between HTTP, WebSocket, and gRPC)
// ---------------------------------------------------------------------------

/// Convert named key sequences to raw bytes for PTY input.
///
/// Delegates to [`encode_key`] for each key; returns an error with the
/// unrecognised key name if any key is unknown.
pub fn keys_to_bytes(keys: &[String]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for key in keys {
        match encode_key(key) {
            Some(bytes) => out.extend_from_slice(&bytes),
            None => return Err(key.clone()),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the axum `Router` with all HTTP and WebSocket routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(http::health))
        .route("/api/v1/ready", get(http::ready))
        .route("/api/v1/screen", get(http::screen))
        .route("/api/v1/screen/text", get(http::screen_text))
        .route("/api/v1/output", get(http::output))
        .route("/api/v1/status", get(http::status))
        .route("/api/v1/input", post(http::input))
        .route("/api/v1/input/keys", post(http::input_keys))
        .route("/api/v1/resize", post(http::resize))
        .route("/api/v1/signal", post(http::signal))
        .route("/api/v1/agent/state", get(http::agent_state))
        .route("/api/v1/agent/nudge", post(http::agent_nudge))
        .route("/api/v1/agent/respond", post(http::agent_respond))
        .route("/api/v1/hooks/stop", post(http::hooks_stop))
        .route("/api/v1/hooks/stop/resolve", post(http::resolve_stop))
        .route(
            "/api/v1/config/stop",
            get(http::get_stop_config).put(http::put_stop_config),
        )
        .route("/ws", get(ws::ws_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_layer,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Build a minimal health-only router (for `--port-health`).
pub fn build_health_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(http::health))
        .route("/api/v1/ready", get(http::ready))
        .route("/api/v1/agent/state", get(http::agent_state))
        .with_state(state)
}
