// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Broker HTTP handlers (Epic 16b/16c).

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;

use crate::broker::registry::RegisterRequest;
use crate::error::ErrorCode;
use crate::transport::state::Store;

/// `GET /api/v1/broker/pods` — list registered pods with health status.
pub async fn broker_pods(State(s): State<Arc<Store>>) -> impl IntoResponse {
    match &s.broker_registry {
        Some(registry) => {
            let pods = registry.list().await;
            Json(serde_json::json!({ "pods": pods })).into_response()
        }
        None => ErrorCode::NotFound
            .to_http_response("broker not enabled")
            .into_response(),
    }
}

/// `POST /api/v1/broker/register` — register an agent pod.
pub async fn broker_register(
    State(s): State<Arc<Store>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let Some(ref registry) = s.broker_registry else {
        return ErrorCode::NotFound
            .to_http_response("broker not enabled")
            .into_response();
    };

    let name = req.name.clone();
    let is_new = registry.register(req).await;
    Json(serde_json::json!({
        "registered": true,
        "pod": name,
        "new": is_new,
    }))
    .into_response()
}

/// `DELETE /api/v1/broker/deregister` — remove a pod.
pub async fn broker_deregister(
    State(s): State<Arc<Store>>,
    Json(req): Json<DeregisterRequest>,
) -> impl IntoResponse {
    let Some(ref registry) = s.broker_registry else {
        return ErrorCode::NotFound
            .to_http_response("broker not enabled")
            .into_response();
    };

    let removed = registry.deregister(&req.name).await;
    Json(serde_json::json!({
        "removed": removed,
        "pod": req.name,
    }))
    .into_response()
}

#[derive(Debug, serde::Deserialize)]
pub struct DeregisterRequest {
    pub name: String,
}
