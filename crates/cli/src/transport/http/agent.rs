// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent state and interaction HTTP handlers.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::driver::PromptContext;
use crate::transport::handler::{
    error_message, handle_nudge, handle_respond, TransportQuestionAnswer,
};
use crate::transport::state::Store;

// -- Types --------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub agent: String,
    pub state: String,
    pub since_seq: u64,
    pub screen_seq: u64,
    pub detection_tier: String,
    pub detection_cause: String,
    pub prompt: Option<PromptContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondRequest {
    pub accept: Option<bool>,
    pub text: Option<String>,
    #[serde(default)]
    pub answers: Vec<TransportQuestionAnswer>,
    pub option: Option<i32>,
}

// -- Handlers -----------------------------------------------------------------

/// `GET /api/v1/agent`
pub async fn agent(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let state = s.driver.agent_state.read().await;
    let screen = s.terminal.screen.read().await;

    let detection = s.driver.detection.read().await;

    Json(AgentResponse {
        agent: s.config.agent.to_string(),
        state: state.as_str().to_owned(),
        since_seq: s.driver.state_seq.load(Ordering::Acquire),
        screen_seq: screen.seq(),
        detection_tier: detection.tier_str(),
        detection_cause: detection.cause.clone(),
        prompt: state.prompt().cloned(),
        error_detail: s.driver.error.read().await.as_ref().map(|e| e.detail.clone()),
        error_category: s
            .driver
            .error
            .read()
            .await
            .as_ref()
            .map(|e| e.category.as_str().to_owned()),
        last_message: s.driver.last_message.read().await.clone(),
    })
    .into_response()
}

/// `POST /api/v1/agent/nudge`
pub async fn agent_nudge(
    State(s): State<Arc<Store>>,
    Json(req): Json<NudgeRequest>,
) -> impl IntoResponse {
    match handle_nudge(&s, &req.message).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(code) => code.to_http_response(error_message(code)).into_response(),
    }
}

/// `POST /api/v1/agent/respond`
pub async fn agent_respond(
    State(s): State<Arc<Store>>,
    Json(req): Json<RespondRequest>,
) -> impl IntoResponse {
    match handle_respond(&s, req.accept, req.option, req.text.as_deref(), &req.answers).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(code) => code.to_http_response(error_message(code)).into_response(),
    }
}
