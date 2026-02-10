// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Transcript snapshot HTTP handlers.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::error::ErrorCode;
use crate::transport::state::Store;

// -- Types --------------------------------------------------------------------

/// Query parameters for the transcript catchup endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct CatchupQuery {
    #[serde(default)]
    pub since_transcript: u32,
    #[serde(default)]
    pub since_line: u64,
}

// -- Handlers -----------------------------------------------------------------

/// `GET /api/v1/transcripts` — list all transcript snapshots.
pub async fn list_transcripts(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let list = s.transcript.list().await;
    Json(serde_json::json!({ "transcripts": list }))
}

/// `GET /api/v1/transcripts/catchup` — catch up from a cursor.
pub async fn catchup_transcripts(
    State(s): State<Arc<Store>>,
    Query(q): Query<CatchupQuery>,
) -> impl IntoResponse {
    match s.transcript.catchup(q.since_transcript, q.since_line).await {
        Ok(resp) => Json(serde_json::to_value(resp).unwrap_or_default()).into_response(),
        Err(e) => {
            ErrorCode::Internal.to_http_response(format!("catchup failed: {e}")).into_response()
        }
    }
}

/// `GET /api/v1/transcripts/{number}` — get a single transcript's content.
pub async fn get_transcript(
    State(s): State<Arc<Store>>,
    axum::extract::Path(number): axum::extract::Path<u32>,
) -> impl IntoResponse {
    match s.transcript.get_content(number).await {
        Ok(content) => {
            Json(serde_json::json!({ "number": number, "content": content })).into_response()
        }
        Err(_) => ErrorCode::BadRequest
            .to_http_response(format!("transcript {number} not found"))
            .into_response(),
    }
}
