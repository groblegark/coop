// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session switch and profile HTTP handlers.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::profile::{ProfileConfig, ProfileEntry, ProfileInfo};
use crate::switch::SwitchRequest;
use crate::transport::handler::resolve_switch_profile;
use crate::transport::state::Store;

// -- Switch -------------------------------------------------------------------

/// `POST /api/v1/session/switch` — schedule a credential switch (202 Accepted).
pub async fn switch_session(
    State(s): State<Arc<Store>>,
    Json(mut req): Json<SwitchRequest>,
) -> impl IntoResponse {
    if let Err(code) = resolve_switch_profile(&s, &mut req).await {
        return code.to_http_response("unknown profile").into_response();
    }
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

// -- Profiles -----------------------------------------------------------------

/// Request body for `POST /api/v1/session/profiles`.
#[derive(Debug, Deserialize)]
pub struct RegisterProfilesRequest {
    pub profiles: Vec<ProfileEntry>,
    #[serde(default)]
    pub config: Option<ProfileConfig>,
}

/// Response for `GET /api/v1/session/profiles`.
#[derive(Debug, Serialize)]
pub struct ProfileListResponse {
    pub profiles: Vec<ProfileInfo>,
    pub config: ProfileConfig,
    pub active_profile: Option<String>,
}

/// `POST /api/v1/session/profiles` — register credential profiles.
pub async fn register_profiles(
    State(s): State<Arc<Store>>,
    Json(req): Json<RegisterProfilesRequest>,
) -> impl IntoResponse {
    let count = req.profiles.len();
    s.profile.register(req.profiles, req.config).await;
    Json(serde_json::json!({ "registered": count }))
}

/// `GET /api/v1/session/profiles` — list all profiles with status.
pub async fn list_profiles(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let profiles = s.profile.list().await;
    let config = s.profile.config().await;
    let active_profile = s.profile.active_name().await;
    Json(ProfileListResponse { profiles, config, active_profile })
}
