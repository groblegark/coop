// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP + WebSocket transport for the mux proxy.

pub mod auth;
pub mod http;
pub mod ws;

use std::sync::Arc;

use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use tower_http::cors::CorsLayer;

use crate::state::MuxState;

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
        // WebSocket
        .route("/ws/{session_id}", get(ws::ws_handler))
        // Middleware
        .layer(middleware::from_fn_with_state(state.clone(), auth::auth_layer))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
