// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP + WebSocket transport for the mux proxy.

pub mod auth;
pub mod http;
pub mod http_cred;
pub mod ws;
pub mod ws_mux;

use std::sync::Arc;

use axum::middleware;
use axum::response::Html;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::CorsLayer;

use crate::state::MuxState;

/// Embedded mux dashboard HTML.
const MUX_HTML: &str = include_str!("../web/mux.html");

/// Build the axum `Router` with all mux routes.
pub fn build_router(state: Arc<MuxState>) -> Router {
    Router::new()
        // Health (no auth)
        .route("/api/v1/health", get(http::health))
        // Session management
        .route("/api/v1/sessions", post(http::register_session).get(http::list_sessions))
        .route("/api/v1/sessions/{id}", delete(http::deregister_session))
        // Cached data
        .route("/api/v1/sessions/{id}/screen", get(http::session_screen))
        .route("/api/v1/sessions/{id}/status", get(http::session_status))
        // Proxy endpoints
        .route("/api/v1/sessions/{id}/agent", get(http::session_agent))
        .route("/api/v1/sessions/{id}/input", post(http::session_input))
        .route("/api/v1/sessions/{id}/input/raw", post(http::session_input_raw))
        .route("/api/v1/sessions/{id}/input/keys", post(http::session_input_keys))
        // WebSocket (per-session bridge)
        .route("/ws/{session_id}", get(ws::ws_handler))
        // Mux aggregation
        .route("/ws/mux", get(ws_mux::ws_mux_handler))
        .route("/mux", get(|| async { Html(MUX_HTML) }))
        // Credential management (returns 400 when broker not configured)
        .route("/api/v1/credentials/status", get(http_cred::credentials_status))
        .route("/api/v1/credentials/seed", post(http_cred::credentials_seed))
        .route("/api/v1/credentials/reauth", post(http_cred::credentials_reauth))
        // Middleware
        .layer(middleware::from_fn_with_state(state.clone(), auth::auth_layer))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
