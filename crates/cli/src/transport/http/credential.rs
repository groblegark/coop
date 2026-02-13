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

/// Request body for `POST /api/v1/credentials/reauth`.
#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    /// Account name to re-authenticate. If omitted, re-auths the first revoked account.
    #[serde(default)]
    pub account: Option<String>,
}

/// `POST /api/v1/credentials/reauth` — initiate device code re-authentication.
pub async fn credentials_reauth(
    State(s): State<Arc<Store>>,
    Json(req): Json<ReauthRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credentials else {
        return ErrorCode::Internal
            .to_http_response("credential broker not configured")
            .into_response();
    };

    // Determine which account to re-auth.
    let account_name = match req.account {
        Some(name) => name,
        None => {
            let statuses = broker.status().await;
            match statuses.iter().find(|a| a.status == crate::credential::AccountStatus::Revoked) {
                Some(a) => a.name.clone(),
                None => {
                    return ErrorCode::NotFound
                        .to_http_response("no revoked accounts found")
                        .into_response();
                }
            }
        }
    };

    match broker.initiate_reauth(&account_name).await {
        Ok((auth_url, user_code)) => Json(serde_json::json!({
            "account": account_name,
            "auth_url": auth_url,
            "user_code": user_code,
        }))
        .into_response(),
        Err(e) => ErrorCode::Internal.to_http_response(e).into_response(),
    }
}

/// Request body for `POST /api/v1/credentials/login-reauth`.
#[derive(Debug, Deserialize)]
pub struct LoginReauthRequest {
    /// Account name to re-authenticate. If omitted, re-auths the first revoked account.
    #[serde(default)]
    pub account: Option<String>,
}

/// Request body for `POST /api/v1/credentials/login-reauth/complete`.
#[derive(Debug, Deserialize)]
pub struct LoginReauthCompleteRequest {
    /// Account name.
    pub account: String,
    /// Authorization code from the browser callback.
    pub code: String,
    /// Redirect URI used in the authorization request.
    pub redirect_uri: String,
    /// OAuth client ID used in the authorization request.
    pub client_id: String,
}

/// `POST /api/v1/credentials/login-reauth` — initiate authorization code reauth.
///
/// Returns the authorization URL the human must open in a browser. After
/// authorizing, the human receives a code which must be submitted via
/// `POST /api/v1/credentials/login-reauth/complete`.
pub async fn credentials_login_reauth(
    State(s): State<Arc<Store>>,
    Json(req): Json<LoginReauthRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credentials else {
        return ErrorCode::Internal
            .to_http_response("credential broker not configured")
            .into_response();
    };

    let account_name = match req.account {
        Some(name) => name,
        None => {
            let statuses = broker.status().await;
            match statuses
                .iter()
                .find(|a| a.status == crate::credential::AccountStatus::Revoked)
            {
                Some(a) => a.name.clone(),
                None => {
                    return ErrorCode::NotFound
                        .to_http_response("no revoked accounts found")
                        .into_response();
                }
            }
        }
    };

    match broker.initiate_login_reauth(&account_name).await {
        Ok(session) => Json(serde_json::json!(session)).into_response(),
        Err(e) => ErrorCode::Internal.to_http_response(e).into_response(),
    }
}

/// `POST /api/v1/credentials/login-reauth/complete` — exchange auth code for tokens.
pub async fn credentials_login_reauth_complete(
    State(s): State<Arc<Store>>,
    Json(req): Json<LoginReauthCompleteRequest>,
) -> impl IntoResponse {
    let Some(ref broker) = s.credentials else {
        return ErrorCode::Internal
            .to_http_response("credential broker not configured")
            .into_response();
    };

    match broker
        .complete_login_reauth(&req.account, &req.code, &req.redirect_uri, &req.client_id)
        .await
    {
        Ok(()) => Json(serde_json::json!({
            "account": req.account,
            "status": "healthy",
        }))
        .into_response(),
        Err(e) => ErrorCode::Internal.to_http_response(e).into_response(),
    }
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

    if broker.seed(&req.account, req.access_token, req.refresh_token, req.expires_in).await {
        Json(serde_json::json!({ "seeded": true, "account": req.account })).into_response()
    } else {
        ErrorCode::NotFound
            .to_http_response(format!("unknown account: {}", req.account))
            .into_response()
    }
}
