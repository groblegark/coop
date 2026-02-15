// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP handlers for the mux proxy.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::MuxError;
use crate::state::{epoch_ms, MuxEvent, MuxState, SessionEntry};
use crate::upstream::client::UpstreamClient;

// -- Request/Response types ---------------------------------------------------

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub session_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub url: String,
    #[serde(default)]
    pub auth_token: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub id: String,
    pub registered: bool,
}

#[derive(Debug, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub url: String,
    pub metadata: serde_json::Value,
    pub registered_at_ms: u64,
    pub health_failures: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_state: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DeregisterResponse {
    pub id: String,
    pub removed: bool,
}

#[derive(Debug, Serialize)]
pub struct LaunchConfigResponse {
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct LaunchResponse {
    pub launched: bool,
}

// -- Handlers -----------------------------------------------------------------

/// `GET /api/v1/health`
pub async fn health(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    let sessions = s.sessions.read().await;
    Json(HealthResponse { status: "running".to_owned(), session_count: sessions.len() })
}

/// `POST /api/v1/sessions` — register a coop session.
pub async fn register_session(
    State(s): State<Arc<MuxState>>,
    Json(req): Json<RegisterRequest>,
) -> impl IntoResponse {
    let url = req.url.trim_end_matches('/').to_owned();

    // Validate upstream is reachable.
    let client = UpstreamClient::new(url.clone(), req.auth_token.clone());
    if let Err(e) = client.health().await {
        tracing::warn!(url = %url, err = %e, "upstream health check failed during registration");
        return MuxError::UpstreamError
            .to_http_response(format!("upstream unreachable: {e}"))
            .into_response();
    }

    let id = req.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let metadata = req.metadata.unwrap_or(serde_json::Value::Null);
    let cancel = CancellationToken::new();

    // Clone values needed for the SessionOnline event and credential push before moving into entry.
    let event_url = url.clone();
    let event_metadata = metadata.clone();
    let cred_url = url.clone();
    let cred_token = req.auth_token.clone();

    let entry = Arc::new(SessionEntry {
        id: id.clone(),
        url,
        auth_token: req.auth_token,
        metadata,
        registered_at: std::time::Instant::now(),
        cached_screen: tokio::sync::RwLock::new(None),
        cached_status: tokio::sync::RwLock::new(None),
        health_failures: std::sync::atomic::AtomicU32::new(0),
        cancel: cancel.clone(),
        ws_bridge: tokio::sync::RwLock::new(None),
    });

    let is_new = {
        let mut sessions = s.sessions.write().await;
        if sessions.contains_key(&id) {
            // Heartbeat re-registration: keep the existing entry so that
            // pollers/feeds (which hold Arc clones of the old entry) continue
            // writing to the same cached_screen/cached_status that screen_batch
            // reads.  Replacing the entry would orphan their writes.
            false
        } else {
            // Evict any stale session(s) pointing to the same URL (e.g. after a
            // pod restart generated a new session UUID for the same coop instance).
            let stale: Vec<String> = sessions
                .iter()
                .filter(|(k, v)| *k != &id && v.url == entry.url)
                .map(|(k, _)| k.clone())
                .collect();
            for stale_id in &stale {
                if let Some(old) = sessions.remove(stale_id) {
                    old.cancel.cancel();
                    let _ = s
                        .feed
                        .event_tx
                        .send(MuxEvent::SessionOffline { session: stale_id.clone() });
                    tracing::info!(
                        old_session = %stale_id,
                        new_session = %id,
                        url = %entry.url,
                        "evicted stale session with same URL"
                    );
                }
            }
            sessions.insert(id.clone(), Arc::clone(&entry));
            true
        }
    };
    if is_new {
        // Start background screen/status pollers for this session.
        crate::upstream::poller::spawn_screen_poller(entry, &s.config, cancel);

        // Notify connected dashboard clients about the new session.
        let _ = s.feed.event_tx.send(MuxEvent::SessionOnline {
            session: id.clone(),
            url: event_url,
            metadata: event_metadata,
        });

        // Push all healthy account profiles to the new session.
        if let Some(ref broker) = s.credential_broker {
            let broker = Arc::clone(broker);
            let session_url = cred_url;
            let session_token = cred_token;
            tokio::spawn(async move {
                let status_list = broker.status_list().await;
                for acct in &status_list {
                    if acct.status != crate::credential::AccountStatus::Healthy {
                        continue;
                    }
                    let Some(credentials) = broker.get_credentials(&acct.name).await else {
                        continue;
                    };
                    let client = UpstreamClient::new(session_url.clone(), session_token.clone());
                    let profile_body = serde_json::json!({
                        "profiles": [{
                            "name": acct.name,
                            "credentials": credentials,
                        }]
                    });
                    if let Err(e) =
                        client.post_json("/api/v1/session/profiles", &profile_body).await
                    {
                        tracing::debug!(account = %acct.name, err = %e, "failed to push profile to new session");
                    }
                }
            });
        }

        tracing::info!(session_id = %id, "session registered");
    } else {
        tracing::debug!(session_id = %id, "session re-registered (heartbeat)");
    }

    Json(RegisterResponse { id, registered: true }).into_response()
}

/// `DELETE /api/v1/sessions/{id}` — deregister a session.
pub async fn deregister_session(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if s.remove_session(&id).await.is_some() {
        tracing::info!(session_id = %id, "session deregistered");
        Json(DeregisterResponse { id, removed: true }).into_response()
    } else {
        MuxError::SessionNotFound.to_http_response("session not found").into_response()
    }
}

/// `GET /api/v1/sessions` — list all registered sessions.
pub async fn list_sessions(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    let sessions = s.sessions.read().await;
    let mut list = Vec::with_capacity(sessions.len());
    for entry in sessions.values() {
        let cached_state = entry.cached_status.read().await.as_ref().map(|st| st.state.clone());
        let registered_at_ms =
            epoch_ms().saturating_sub(entry.registered_at.elapsed().as_millis() as u64);
        list.push(SessionInfo {
            id: entry.id.clone(),
            url: entry.url.clone(),
            metadata: entry.metadata.clone(),
            registered_at_ms,
            health_failures: entry.health_failures.load(std::sync::atomic::Ordering::Relaxed),
            cached_state,
        });
    }
    Json(list)
}

/// `GET /api/v1/sessions/{id}/screen` — cached screen snapshot.
pub async fn session_screen(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let sessions = s.sessions.read().await;
    let entry = match sessions.get(&id) {
        Some(e) => Arc::clone(e),
        None => {
            return MuxError::SessionNotFound.to_http_response("session not found").into_response()
        }
    };
    drop(sessions);

    let cached = entry.cached_screen.read().await;
    match cached.as_ref() {
        Some(screen) => Json(screen.clone()).into_response(),
        None => MuxError::UpstreamError.to_http_response("screen not yet cached").into_response(),
    }
}

/// `GET /api/v1/sessions/{id}/status` — cached status.
pub async fn session_status(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let sessions = s.sessions.read().await;
    let entry = match sessions.get(&id) {
        Some(e) => Arc::clone(e),
        None => {
            return MuxError::SessionNotFound.to_http_response("session not found").into_response()
        }
    };
    drop(sessions);

    let cached = entry.cached_status.read().await;
    match cached.as_ref() {
        Some(status) => Json(status.clone()).into_response(),
        None => MuxError::UpstreamError.to_http_response("status not yet cached").into_response(),
    }
}

/// `GET /api/v1/sessions/{id}/agent` — proxy to upstream agent endpoint.
pub async fn session_agent(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let sessions = s.sessions.read().await;
    let entry = match sessions.get(&id) {
        Some(e) => Arc::clone(e),
        None => {
            return MuxError::SessionNotFound.to_http_response("session not found").into_response()
        }
    };
    drop(sessions);

    let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
    match client.get_agent().await {
        Ok(value) => Json(value).into_response(),
        Err(e) => {
            MuxError::UpstreamError.to_http_response(format!("upstream error: {e}")).into_response()
        }
    }
}

/// `POST /api/v1/sessions/{id}/input` — proxy input to upstream.
pub async fn session_input(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    proxy_post(&s, &id, "/api/v1/input", body).await
}

/// `POST /api/v1/sessions/{id}/input/raw` — proxy raw input to upstream.
pub async fn session_input_raw(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    proxy_post(&s, &id, "/api/v1/input/raw", body).await
}

/// `POST /api/v1/sessions/{id}/input/keys` — proxy keys to upstream.
pub async fn session_input_keys(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    proxy_post(&s, &id, "/api/v1/input/keys", body).await
}

/// `POST /api/v1/sessions/{id}/upload` — proxy file upload to upstream.
pub async fn session_upload(
    State(s): State<Arc<MuxState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    proxy_post(&s, &id, "/api/v1/upload", body).await
}

/// `GET /api/v1/config/launch` — whether launch is available.
pub async fn launch_config(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    Json(LaunchConfigResponse { available: s.config.launch.is_some() })
}

/// `POST /api/v1/sessions/launch` — spawn a new session via the configured launch command.
pub async fn launch_session(State(s): State<Arc<MuxState>>) -> impl IntoResponse {
    let launch = match &s.config.launch {
        Some(cmd) => cmd.clone(),
        None => {
            return MuxError::BadRequest
                .to_http_response("launch command not configured")
                .into_response()
        }
    };

    let mux_url = format!("http://{}:{}", s.config.host, s.config.port);

    let mut cmd = tokio::process::Command::new("sh");
    cmd.args(["-c", &launch]);
    cmd.env("COOP_MUX_URL", &mux_url);
    if let Some(token) = &s.config.auth_token {
        cmd.env("COOP_MUX_TOKEN", token);
    }
    // Inject healthy account credentials so the agent CLI has them at startup.
    if let Some(ref broker) = s.credential_broker {
        let status_list = broker.status_list().await;
        for acct in &status_list {
            if acct.status != crate::credential::AccountStatus::Healthy {
                continue;
            }
            if let Some(credentials) = broker.get_credentials(&acct.name).await {
                for (key, value) in &credentials {
                    cmd.env(key, value);
                }
            }
        }
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());
    // Detach into a new process group so launched sessions survive mux restart.
    cmd.process_group(0);

    match cmd.spawn() {
        Ok(_child) => Json(LaunchResponse { launched: true }).into_response(),
        Err(e) => {
            tracing::error!(err = %e, "failed to spawn launch command");
            MuxError::Internal.to_http_response(format!("failed to spawn: {e}")).into_response()
        }
    }
}

/// Generic POST proxy to upstream coop.
async fn proxy_post(
    state: &MuxState,
    session_id: &str,
    path: &str,
    body: serde_json::Value,
) -> axum::response::Response {
    let sessions = state.sessions.read().await;
    let entry = match sessions.get(session_id) {
        Some(e) => Arc::clone(e),
        None => {
            return MuxError::SessionNotFound.to_http_response("session not found").into_response()
        }
    };
    drop(sessions);

    let client = UpstreamClient::new(entry.url.clone(), entry.auth_token.clone());
    match client.post_json(path, &body).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => {
            MuxError::UpstreamError.to_http_response(format!("upstream error: {e}")).into_response()
        }
    }
}
