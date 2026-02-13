// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::MuxError;
use crate::state::MuxState;

/// Constant-time string comparison to prevent timing side-channel attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

/// Validate a Bearer token from HTTP headers.
pub fn validate_bearer(headers: &HeaderMap, expected: Option<&str>) -> Result<(), MuxError> {
    let expected = match expected {
        Some(tok) => tok,
        None => return Ok(()),
    };

    let header =
        headers.get("authorization").and_then(|v| v.to_str().ok()).ok_or(MuxError::Unauthorized)?;

    let token = header.strip_prefix("Bearer ").ok_or(MuxError::Unauthorized)?;
    if constant_time_eq(token, expected) {
        Ok(())
    } else {
        Err(MuxError::Unauthorized)
    }
}

/// Validate a token from a WebSocket query string (`?token=...`).
pub fn validate_ws_query(query: &str, expected: Option<&str>) -> Result<(), MuxError> {
    let expected = match expected {
        Some(tok) => tok,
        None => return Ok(()),
    };

    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            if constant_time_eq(value, expected) {
                return Ok(());
            }
        }
    }

    Err(MuxError::Unauthorized)
}

/// Axum middleware that enforces Bearer token authentication.
///
/// Exempt: `/api/v1/health` and WebSocket upgrades (`/ws/`).
pub async fn auth_layer(
    state: State<Arc<MuxState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();

    // Health, WS, and OAuth callback endpoints skip HTTP bearer auth.
    // WS auth is handled via query param in the WS handler.
    // OAuth callback is initiated by the browser redirect (no bearer token).
    if path == "/api/v1/health"
        || path == "/api/v1/credentials/callback"
        || path.starts_with("/ws/")
    {
        return next.run(req).await;
    }

    if let Err(code) = validate_bearer(req.headers(), state.config.auth_token.as_deref()) {
        let body = crate::error::ErrorResponse { error: code.to_error_body("unauthorized") };
        return (
            StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::UNAUTHORIZED),
            axum::Json(body),
        )
            .into_response();
    }

    next.run(req).await
}
