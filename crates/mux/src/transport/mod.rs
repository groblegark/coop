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
const MUX_HTML: &str = include_str!("../../../web/dist/mux.html");

/// Path to on-disk mux HTML (debug builds only, for `--hot` live reload).
#[cfg(debug_assertions)]
const MUX_HTML_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../web/dist/mux.html");

/// Build the axum `Router`, optionally serving HTML from disk for live reload.
#[cfg(debug_assertions)]
pub fn build_router_hot(state: Arc<MuxState>, hot: bool) -> Router {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    if hot {
        build_router_inner(
            state,
            get(|| async {
                match tokio::fs::read_to_string(MUX_HTML_PATH).await {
                    Ok(html) => Html(html).into_response(),
                    Err(e) => {
                        (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to read mux.html: {e}"))
                            .into_response()
                    }
                }
            }),
        )
    } else {
        build_router_inner(state, get(|| async { Html(MUX_HTML) }))
    }
}

/// Build the axum `Router` with all mux routes.
pub fn build_router(state: Arc<MuxState>) -> Router {
    build_router_inner(state, get(|| async { Html(MUX_HTML) }))
}

fn build_router_inner(
    state: Arc<MuxState>,
    mux_route: axum::routing::MethodRouter<Arc<MuxState>>,
) -> Router {
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
        .route("/api/v1/sessions/{id}/upload", post(http::session_upload))
        // Launch
        .route("/api/v1/sessions/launch", post(http::launch_session))
        .route("/api/v1/config/launch", get(http::launch_config))
        // WebSocket (per-session bridge)
        .route("/ws/{session_id}", get(ws::ws_handler))
        // Mux aggregation
        .route("/ws/mux", get(ws_mux::ws_mux_handler))
        .route("/mux", mux_route)
        // Credential management
        .route("/api/v1/credentials/status", get(http_cred::credentials_status))
        .route("/api/v1/credentials/new", post(http_cred::credentials_new))
        .route("/api/v1/credentials/set", post(http_cred::credentials_set))
        .route("/api/v1/credentials/reauth", post(http_cred::credentials_reauth))
        .route("/api/v1/credentials/exchange", post(http_cred::credentials_exchange))
        .route("/api/v1/credentials/distribute", post(http_cred::credentials_distribute))
        // Credential pool
        .route("/api/v1/credentials/pool", get(http_cred::credentials_pool))
        .route("/api/v1/credentials/pool/rebalance", post(http_cred::credentials_pool_rebalance))
        // Middleware
        .layer(middleware::from_fn_with_state(state.clone(), auth::auth_layer))
        .layer(CorsLayer::permissive())
        .with_state(state)
}
