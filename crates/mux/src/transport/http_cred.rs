// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP handlers for credential management endpoints.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::credential::AccountConfig;
use crate::error::MuxError;
use crate::state::MuxState;

/// Helper to get broker or return 400.
fn get_broker(
    s: &MuxState,
) -> Result<&Arc<crate::credential::broker::CredentialBroker>, Box<axum::response::Response>> {
    s.credential_broker.as_ref().ok_or_else(|| {
        Box::new(
            MuxError::BadRequest
                .to_http_response("credential broker not configured")
                .into_response(),
        )
    })
}

/// `GET /api/v1/credentials/status` — list all accounts with status.
pub async fn credentials_status(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };
    let list = broker.status_list().await;
    Json(list).into_response()
}

/// Request body for `POST /api/v1/credentials/set`.
#[derive(Debug, Deserialize)]
pub struct SetRequest {
    pub account: String,
    pub token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
}

/// `POST /api/v1/credentials/set` — set tokens for an existing account.
pub async fn credentials_set(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<SetRequest>,
) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };
    match broker.set_token(&req.account, req.token, req.refresh_token, req.expires_in).await {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/credentials/reauth`.
#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    #[serde(default)]
    pub account: Option<String>,
}

/// `POST /api/v1/credentials/reauth` — trigger OAuth authorization code flow for an account.
pub async fn credentials_reauth(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<ReauthRequest>,
) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };

    let account = match req.account {
        Some(name) => name,
        None => match broker.first_account_name().await {
            Some(name) => name,
            None => {
                return MuxError::BadRequest
                    .to_http_response("no accounts configured")
                    .into_response()
            }
        },
    };

    match broker.initiate_reauth(&account).await {
        Ok(resp) => Json(serde_json::json!({
            "account": resp.account,
            "auth_url": resp.auth_url,
            "user_code": resp.user_code,
            "state": resp.state,
        }))
        .into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/credentials/new`.
#[derive(Debug, Deserialize)]
pub struct NewAccountRequest {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub env_key: Option<String>,
    #[serde(default)]
    pub token_url: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub auth_url: Option<String>,
    /// OAuth device authorization endpoint (RFC 8628).
    #[serde(default)]
    pub device_auth_url: Option<String>,
    /// Optional token to set immediately.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// Whether this account supports OAuth reauth/refresh (default: true).
    #[serde(default = "crate::credential::default_true")]
    pub reauth: bool,
}

/// `POST /api/v1/credentials/new` — create a new credential account.
pub async fn credentials_new(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<NewAccountRequest>,
) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };

    let config = AccountConfig {
        name: req.name.clone(),
        provider: req.provider,
        env_key: req.env_key,
        token_url: req.token_url,
        client_id: req.client_id,
        auth_url: req.auth_url,
        device_auth_url: req.device_auth_url,
        reauth: req.reauth,
    };

    match broker.add_account(config, req.token, req.refresh_token, req.expires_in).await {
        Ok(()) => Json(serde_json::json!({ "added": true, "account": req.name })).into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/credentials/distribute`.
#[derive(Debug, Deserialize)]
pub struct DistributeRequest {
    pub account: String,
    #[serde(default)]
    pub switch: bool,
}

/// `POST /api/v1/credentials/distribute` — manually push an account's credentials to all sessions.
pub async fn credentials_distribute(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<DistributeRequest>,
) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };

    let credentials = match broker.get_credentials(&req.account).await {
        Some(creds) => creds,
        None => {
            return MuxError::BadRequest
                .to_http_response(format!("no credentials available for account: {}", req.account))
                .into_response()
        }
    };

    crate::credential::distributor::distribute_to_sessions(
        &s,
        &req.account,
        &credentials,
        req.switch,
    )
    .await;
    Json(serde_json::json!({ "distributed": true, "account": req.account })).into_response()
}

/// Request body for `POST /api/v1/credentials/exchange`.
#[derive(Debug, Deserialize)]
pub struct ExchangeRequest {
    pub state: String,
    pub code: String,
}

/// `POST /api/v1/credentials/exchange` — exchange an authorization code (pasted by the user) for tokens.
pub async fn credentials_exchange(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<ExchangeRequest>,
) -> impl IntoResponse {
    let broker = match get_broker(&s) {
        Ok(b) => b,
        Err(resp) => return *resp,
    };

    match broker.complete_reauth(&req.state, &req.code).await {
        Ok(()) => Json(serde_json::json!({ "completed": true })).into_response(),
        Err(e) => MuxError::BadRequest.to_http_response(e.to_string()).into_response(),
    }
}
