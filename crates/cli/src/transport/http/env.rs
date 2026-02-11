// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Environment variable and working directory HTTP handlers.
//!
//! - `GET /api/v1/env`       — list all child process env vars
//! - `GET /api/v1/env/:key`  — read a single env var from the child
//! - `PUT /api/v1/env/:key`  — store a pending env var (applied on next switch)
//! - `DELETE /api/v1/env/:key` — remove a pending env var
//! - `GET /api/v1/session/cwd` — read the child process working directory

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use crate::transport::state::Store;

// -- Response types -----------------------------------------------------------

#[derive(Serialize)]
pub struct EnvListResponse {
    pub vars: HashMap<String, String>,
    pub pending: HashMap<String, String>,
}

#[derive(Serialize)]
pub struct EnvGetResponse {
    pub key: String,
    pub value: Option<String>,
    pub source: &'static str,
}

#[derive(Deserialize)]
pub struct EnvPutRequest {
    pub value: String,
}

#[derive(Serialize)]
pub struct EnvPutResponse {
    pub key: String,
    pub updated: bool,
}

#[derive(Serialize)]
pub struct CwdResponse {
    pub cwd: String,
}

// -- Helpers ------------------------------------------------------------------

/// Read all environment variables from the child process via `/proc/{pid}/environ`.
///
/// The file contains null-separated `KEY=VALUE` pairs.  Returns an empty map
/// if the child is not running or the file is unreadable (e.g. macOS).
fn read_child_environ(pid: u32) -> HashMap<String, String> {
    let path = format!("/proc/{pid}/environ");
    let Ok(data) = std::fs::read(&path) else {
        return HashMap::new();
    };
    data.split(|&b| b == 0)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| {
            let s = std::str::from_utf8(chunk).ok()?;
            let (k, v) = s.split_once('=')?;
            Some((k.to_owned(), v.to_owned()))
        })
        .collect()
}

/// Read the child PID from the store, returning an error response if not running.
#[allow(clippy::result_large_err)]
fn get_child_pid(s: &Store) -> Result<u32, Response> {
    let pid = s.terminal.child_pid.load(std::sync::atomic::Ordering::Relaxed);
    if pid == 0 {
        Err(ErrorCode::Exited.to_http_response("child process not running").into_response())
    } else {
        Ok(pid)
    }
}

// -- Handlers -----------------------------------------------------------------

/// `GET /api/v1/env` — list all child process environment variables and pending overrides.
pub async fn list_env(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let pid = match get_child_pid(&s) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let vars = read_child_environ(pid);
    let pending = s.pending_env.read().await.clone();
    Json(EnvListResponse { vars, pending }).into_response()
}

/// `GET /api/v1/env/:key` — read a single environment variable.
///
/// Checks pending overrides first, then falls back to the child's live environ.
pub async fn get_env(
    State(s): State<Arc<Store>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    // Check pending first (these take precedence on next switch).
    let pending = s.pending_env.read().await;
    if let Some(val) = pending.get(&key) {
        return Json(EnvGetResponse {
            key,
            value: Some(val.clone()),
            source: "pending",
        })
        .into_response();
    }
    drop(pending);

    // Fall back to live child environ.
    let pid = match get_child_pid(&s) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    let vars = read_child_environ(pid);
    let value = vars.get(&key).cloned();
    Json(EnvGetResponse {
        key,
        value,
        source: "child",
    })
    .into_response()
}

/// `PUT /api/v1/env/:key` — store a pending environment variable override.
///
/// The value is applied on the next session switch (credential swap / restart).
pub async fn put_env(
    State(s): State<Arc<Store>>,
    Path(key): Path<String>,
    Json(req): Json<EnvPutRequest>,
) -> impl IntoResponse {
    s.pending_env.write().await.insert(key.clone(), req.value);
    Json(EnvPutResponse { key, updated: true })
}

/// `DELETE /api/v1/env/:key` — remove a pending environment variable override.
pub async fn delete_env(
    State(s): State<Arc<Store>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let removed = s.pending_env.write().await.remove(&key).is_some();
    Json(EnvPutResponse { key, updated: removed })
}

/// `GET /api/v1/session/cwd` — read the child process working directory.
///
/// On Linux, reads `/proc/{pid}/cwd` symlink.
pub async fn get_session_cwd(State(s): State<Arc<Store>>) -> impl IntoResponse {
    let pid = match get_child_pid(&s) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

    let link = format!("/proc/{pid}/cwd");
    match std::fs::read_link(&link) {
        Ok(path) => Json(CwdResponse {
            cwd: path.to_string_lossy().into_owned(),
        })
        .into_response(),
        Err(e) => ErrorCode::Internal
            .to_http_response(format!("cannot read cwd: {e}"))
            .into_response(),
    }
}
