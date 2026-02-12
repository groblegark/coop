// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Multiplexer HTTP handlers.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::transport::state::Store;

/// `GET /api/v1/mux/pods` — list all pods with cached state.
pub async fn mux_list_pods(State(s): State<Arc<Store>>) -> impl IntoResponse {
    match &s.multiplexer {
        Some(mux) => {
            let cached = mux.cached_state().await;
            let pods: Vec<serde_json::Value> = cached
                .iter()
                .map(|(name, cache)| {
                    serde_json::json!({
                        "name": name,
                        "state": cache.agent_state,
                        "credential_status": cache.credential_status,
                        "screen_cols": cache.screen_cols,
                        "screen_rows": cache.screen_rows,
                    })
                })
                .collect();
            Json(serde_json::json!({ "pods": pods })).into_response()
        }
        None => Json(serde_json::json!({
            "pods": [],
            "message": "multiplexer not enabled"
        }))
        .into_response(),
    }
}

/// `GET /api/v1/mux/pods/:name/screen` — cached screen for a pod.
pub async fn mux_pod_screen(
    State(s): State<Arc<Store>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> impl IntoResponse {
    match &s.multiplexer {
        Some(mux) => {
            let cached = mux.cached_state().await;
            match cached.get(&name) {
                Some(cache) => Json(serde_json::json!({
                    "pod": name,
                    "lines": cache.screen_lines,
                    "cols": cache.screen_cols,
                    "rows": cache.screen_rows,
                }))
                .into_response(),
                None => Json(serde_json::json!({
                    "error": format!("pod '{}' not found", name)
                }))
                .into_response(),
            }
        }
        None => Json(serde_json::json!({ "error": "multiplexer not enabled" })).into_response(),
    }
}
