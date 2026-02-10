// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Profile registration and listing HTTP handlers.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::profile::{ProfileConfig, ProfileEntry, ProfileInfo};
use crate::transport::state::Store;

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
