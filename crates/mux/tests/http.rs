// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for the mux HTTP API.
//!
//! Uses `axum_test::TestServer` — no real TCP needed.

use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use std::time::Instant;

use axum_test::TestServer;
use tokio_util::sync::CancellationToken;

use coop_mux::config::MuxConfig;
use coop_mux::credential::broker::CredentialBroker;
use coop_mux::credential::{AccountConfig, CredentialConfig};

use coop_mux::state::{MuxState, SessionEntry};
use coop_mux::transport::build_router;

fn test_config() -> MuxConfig {
    MuxConfig {
        host: "127.0.0.1".into(),
        port: 0,
        auth_token: None,
        screen_poll_ms: 500,
        status_poll_ms: 2000,
        health_check_ms: 10000,
        max_health_failures: 3,
        launch: None,
        credential_config: None,
        #[cfg(debug_assertions)]
        hot: false,
    }
}

fn test_state() -> Arc<MuxState> {
    Arc::new(MuxState::new(test_config(), CancellationToken::new()))
}

fn test_state_with_broker(accounts: Vec<AccountConfig>) -> Arc<MuxState> {
    let config = CredentialConfig { accounts };
    let (event_tx, _rx) = tokio::sync::broadcast::channel(64);
    let broker = CredentialBroker::new(config, event_tx);
    let mut state = MuxState::new(test_config(), CancellationToken::new());
    state.credential_broker = Some(broker);
    Arc::new(state)
}

fn test_server(state: Arc<MuxState>) -> TestServer {
    let router = build_router(state);
    TestServer::new(router).expect("failed to create test server")
}

/// Insert a fake session entry directly (bypasses upstream health check).
async fn insert_session(state: &MuxState, id: &str, url: &str) {
    let entry = Arc::new(SessionEntry {
        id: id.to_owned(),
        url: url.to_owned(),
        auth_token: None,
        metadata: serde_json::Value::Null,
        registered_at: Instant::now(),
        cached_screen: tokio::sync::RwLock::new(None),
        cached_status: tokio::sync::RwLock::new(None),
        health_failures: AtomicU32::new(0),
        cancel: CancellationToken::new(),
        ws_bridge: tokio::sync::RwLock::new(None),
    });
    state.sessions.write().await.insert(id.to_owned(), entry);
}

#[tokio::test]
async fn health_returns_session_count() -> anyhow::Result<()> {
    let state = test_state();
    insert_session(&state, "s1", "http://fake:1001").await;
    insert_session(&state, "s2", "http://fake:1002").await;

    let server = test_server(state);
    let resp = server.get("/api/v1/health").await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "running");
    assert_eq!(body["session_count"], 2);
    Ok(())
}

#[tokio::test]
async fn list_sessions_returns_registered() -> anyhow::Result<()> {
    let state = test_state();
    insert_session(&state, "abc", "http://fake:2001").await;
    insert_session(&state, "def", "http://fake:2002").await;

    let server = test_server(state);
    let resp = server.get("/api/v1/sessions").await;
    resp.assert_status_ok();

    let list: Vec<serde_json::Value> = resp.json();
    assert_eq!(list.len(), 2);

    let ids: Vec<&str> = list.iter().filter_map(|s| s["id"].as_str()).collect();
    assert!(ids.contains(&"abc"));
    assert!(ids.contains(&"def"));
    Ok(())
}

#[tokio::test]
async fn deregister_session_removes_it() -> anyhow::Result<()> {
    let state = test_state();
    insert_session(&state, "to-remove", "http://fake:3001").await;

    let server = test_server(Arc::clone(&state));
    let resp = server.delete("/api/v1/sessions/to-remove").await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["removed"], true);

    // Verify it's gone from the list.
    let sessions = state.sessions.read().await;
    assert!(!sessions.contains_key("to-remove"));
    Ok(())
}

#[tokio::test]
async fn deregister_nonexistent_returns_404() -> anyhow::Result<()> {
    let state = test_state();
    let server = test_server(state);
    let resp = server.delete("/api/v1/sessions/nope").await;
    resp.assert_status(axum::http::StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn dashboard_serves_html() -> anyhow::Result<()> {
    let state = test_state();
    let server = test_server(state);
    let resp = server.get("/mux").await;
    resp.assert_status_ok();

    let body = resp.text();
    assert!(body.contains("<html") || body.contains("<!DOCTYPE"));
    Ok(())
}

#[tokio::test]
async fn credentials_status_without_broker_returns_400() -> anyhow::Result<()> {
    let state = test_state();
    let server = test_server(state);
    let resp = server.get("/api/v1/credentials/status").await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn credentials_seed_and_status() -> anyhow::Result<()> {
    let accounts = vec![AccountConfig {
        name: "test-acct".into(),
        provider: "claude".into(),
        env_key: None,
        token_url: None,
        client_id: None,
        auth_url: None,
    }];
    let state = test_state_with_broker(accounts);
    let server = test_server(Arc::clone(&state));

    // Seed tokens.
    let seed_resp = server
        .post("/api/v1/credentials/seed")
        .json(&serde_json::json!({
            "account": "test-acct",
            "token": "sk-test-token",
            "expires_in": 3600
        }))
        .await;
    seed_resp.assert_status_ok();

    let body: serde_json::Value = seed_resp.json();
    assert_eq!(body["seeded"], true);

    // Check status.
    let status_resp = server.get("/api/v1/credentials/status").await;
    status_resp.assert_status_ok();

    let list: Vec<serde_json::Value> = status_resp.json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "test-acct");
    assert_eq!(list[0]["status"], "healthy");
    Ok(())
}

#[tokio::test]
async fn credentials_reauth_without_broker_returns_400() -> anyhow::Result<()> {
    let state = test_state();
    let server = test_server(state);
    let resp = server.post("/api/v1/credentials/reauth").json(&serde_json::json!({})).await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
    Ok(())
}
