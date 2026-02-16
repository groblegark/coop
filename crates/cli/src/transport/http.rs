// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP request/response types and axum handler implementations.

mod agent;
mod events;
mod hooks;
mod record;
mod screen;
mod switch;
mod transcript;
mod upload;
mod usage;

pub use agent::*;
pub use events::*;
pub use hooks::*;
pub use record::*;
pub use screen::*;
pub use switch::*;
pub use transcript::*;
pub use upload::*;
pub use usage::*;

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::transport::state::Store;

// -- Lifecycle ----------------------------------------------------------------

/// `POST /api/v1/session/restart` — kill and respawn the agent process (202 Accepted).
pub async fn restart_session(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let req = crate::switch::SwitchRequest { credentials: None, force: true, profile: None };
    match s.switch.switch_tx.try_send(req) {
        Ok(()) => axum::http::StatusCode::ACCEPTED.into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            crate::error::ErrorCode::SwitchInProgress
                .to_http_response("a switch is already in progress")
                .into_response()
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => crate::error::ErrorCode::Internal
            .to_http_response("switch channel closed")
            .into_response(),
    }
}

/// `POST /api/v1/shutdown` — initiate graceful coop shutdown.
pub async fn shutdown(State(s): State<Arc<Store>>) -> impl IntoResponse {
    s.lifecycle.shutdown.cancel();
    Json(serde_json::json!({ "accepted": true }))
}

#[cfg(test)]
mod screen_tests;

#[cfg(test)]
mod agent_tests;

#[cfg(test)]
mod hooks_tests;

#[cfg(test)]
mod transcript_tests;

#[cfg(test)]
mod profile_tests;

#[cfg(test)]
mod switch_tests;

#[cfg(test)]
mod usage_tests;
