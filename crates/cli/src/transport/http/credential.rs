// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Credential broker HTTP handlers (Epic 16).

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::credential::AccountStatusInfo;
use crate::error::ErrorCode;
use crate::transport::state::Store;

/// `GET /api/v1/credentials/status` — credential health for all accounts.
pub async fn credentials_status(State(s): State<Arc<Store>>) -> impl IntoResponse {
    match &s.credentials {
        Some(broker) => {
            let accounts = broker.status().await;
            Json(serde_json::json!({ "accounts": accounts })).into_response()
        }
        None => {
            let empty: Vec<AccountStatusInfo> = Vec::new();
            Json(serde_json::json!({
                "accounts": empty,
                "message": "credential broker not configured"
            }))
            .into_response()
        }
    }
}

/// Request body for `POST /api/v1/credentials/seed`.
#[derive(Debug, Deserialize)]
pub struct SeedRequest {
    /// Account name to seed.
    pub account: String,
    /// OAuth access token.
    pub access_token: String,
    /// OAuth refresh token (required for non-static accounts).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Token lifetime in seconds.
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// `POST /api/v1/credentials/seed` — inject initial credentials for an account.
pub async fn credentials_seed(
    State(s): State<Arc<Store>>,
    Json(req): Json<SeedRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credentials else {
        return ErrorCode::Internal
            .to_http_response("credential broker not configured")
            .into_response();
    };

    if broker
        .seed(&req.account, req.access_token, req.refresh_token, req.expires_in)
        .await
    {
        Json(serde_json::json!({ "seeded": true, "account": req.account })).into_response()
    } else {
        ErrorCode::NotFound
            .to_http_response(format!("unknown account: {}", req.account))
            .into_response()
    }
}
