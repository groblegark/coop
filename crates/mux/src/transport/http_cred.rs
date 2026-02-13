// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP handlers for credential management endpoints.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::error::MuxError;
use crate::state::MuxState;

/// `GET /api/v1/credentials/status` — list all accounts with status.
pub async fn credentials_status(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    let Some(ref broker) = s.credential_broker else {
        return MuxError::BadRequest
            .to_http_response("credential broker not configured")
            .into_response();
    };
    let list = broker.status_list().await;
    Json(list).into_response()
}

/// Request body for `POST /api/v1/credentials/seed`.
#[derive(Debug, Deserialize)]
pub struct SeedRequest {
    pub account: String,
    pub token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// `POST /api/v1/credentials/seed` — inject initial tokens for an account.
pub async fn credentials_seed(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<SeedRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credential_broker else {
        return MuxError::BadRequest
            .to_http_response("credential broker not configured")
            .into_response();
    };
    match broker.seed(&req.account, req.token, req.refresh_token, req.expires_in).await {
        Ok(()) => Json(serde_json::json!({ "seeded": true })).into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/credentials/reauth`.
#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    #[serde(default)]
    pub account: Option<String>,
}

/// `POST /api/v1/credentials/reauth` — trigger device code flow for an account.
pub async fn credentials_reauth(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<ReauthRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credential_broker else {
        return MuxError::BadRequest
            .to_http_response("credential broker not configured")
            .into_response();
    };

    let account = req.account.unwrap_or_else(|| {
        broker.config().accounts.first().map(|a| a.name.clone()).unwrap_or_default()
    });

    match broker.initiate_reauth(&account).await {
        Ok(resp) => Json(serde_json::json!({
            "account": account,
            "user_code": resp.user_code,
            "auth_url": resp.verification_uri,
            "expires_in": resp.expires_in,
        }))
        .into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}
