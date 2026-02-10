// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session switch HTTP handler.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::error::ErrorCode;
use crate::switch::SwitchRequest;
use crate::transport::state::Store;

// -- Handlers -----------------------------------------------------------------

/// `POST /api/v1/session/switch` — schedule a credential switch (202 Accepted).
pub async fn switch_session(
    State(s): State<Arc<Store>>,
    Json(req): Json<SwitchRequest>,
) -> impl IntoResponse {
    match s.switch.switch_tx.try_send(req) {
        Ok(()) => axum::http::StatusCode::ACCEPTED.into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => ErrorCode::SwitchInProgress
            .to_http_response("a switch is already in progress")
            .into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            ErrorCode::Internal.to_http_response("switch channel closed").into_response()
        }
    }
}
