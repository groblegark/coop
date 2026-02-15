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

use coopmux::config::MuxConfig;
use coopmux::credential::broker::CredentialBroker;
use coopmux::credential::{AccountConfig, CredentialConfig};

use coopmux::state::{MuxState, SessionEntry};
use coopmux::transport::build_router;

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
        prewarm_capacity: 64,
        prewarm_poll_ms: 15000,
        state_dir: Some(std::env::temp_dir().join(format!("coopmux-test-{}", std::process::id()))),
        #[cfg(debug_assertions)]
        hot: false,
    }
}

fn test_state() -> Arc<MuxState> {
    Arc::new(MuxState::new(test_config(), CancellationToken::new()))
}

fn test_state_with_broker(accounts: Vec<AccountConfig>) -> Arc<MuxState> {
    let cred_config = CredentialConfig { accounts };
    let (event_tx, _rx) = tokio::sync::broadcast::channel(64);
    let mux_config = test_config();
    let broker = CredentialBroker::new(cred_config, event_tx, Some(mux_config.state_dir()));
    let mut state = MuxState::new(mux_config, CancellationToken::new());
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
async fn credentials_set_and_status() -> anyhow::Result<()> {
    let accounts = vec![AccountConfig {
        name: "test-acct".into(),
        provider: "claude".into(),
        env_key: None,
        token_url: None,
        client_id: None,
        auth_url: None,
        device_auth_url: None,
        reauth: true,
    }];
    let state = test_state_with_broker(accounts);
    let server = test_server(Arc::clone(&state));

    // Set tokens.
    let set_resp = server
        .post("/api/v1/credentials/set")
        .json(&serde_json::json!({
            "account": "test-acct",
            "token": "sk-test-token",
            "expires_in": 3600
        }))
        .await;
    set_resp.assert_status_ok();

    let body: serde_json::Value = set_resp.json();
    assert_eq!(body["ok"], true);

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
async fn credentials_add_account_then_status() -> anyhow::Result<()> {
    // Start with an EMPTY broker (no pre-configured accounts).
    let state = test_state_with_broker(vec![]);
    let server = test_server(Arc::clone(&state));

    // Verify status is initially empty.
    let status_resp = server.get("/api/v1/credentials/status").await;
    status_resp.assert_status_ok();
    let list: Vec<serde_json::Value> = status_resp.json();
    assert_eq!(list.len(), 0);

    // Add account via API (same as web UI form).
    let add_resp = server
        .post("/api/v1/credentials/new")
        .json(&serde_json::json!({
            "name": "my-account",
            "provider": "claude"
        }))
        .await;
    add_resp.assert_status_ok();
    let body: serde_json::Value = add_resp.json();
    assert_eq!(body["added"], true);

    // Status should now contain the new account.
    let status_resp = server.get("/api/v1/credentials/status").await;
    status_resp.assert_status_ok();
    let list: Vec<serde_json::Value> = status_resp.json();
    assert_eq!(list.len(), 1, "expected 1 account, got: {list:?}");
    assert_eq!(list[0]["name"], "my-account");
    assert_eq!(list[0]["provider"], "claude");
    assert_eq!(list[0]["status"], "expired");
    Ok(())
}

/// Regression test: the refresh loop previously held an `RwLock` write guard
/// across a 60-second `tokio::time::sleep`, blocking all `status_list()` reads.
#[tokio::test]
async fn credentials_status_not_blocked_by_refresh_loop() -> anyhow::Result<()> {
    let state = test_state_with_broker(vec![]);
    let server = test_server(Arc::clone(&state));

    // Add account with no token — spawns a refresh loop that sleeps in the
    // "no refresh token" branch.
    let add_resp = server
        .post("/api/v1/credentials/new")
        .json(&serde_json::json!({
            "name": "no-token-acct",
            "provider": "claude"
        }))
        .await;
    add_resp.assert_status_ok();

    // Give the refresh loop a chance to start and enter its sleep.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Status must respond promptly — not blocked for 60s by the refresh loop.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        server.get("/api/v1/credentials/status"),
    )
    .await;
    assert!(result.is_ok(), "status endpoint blocked by refresh loop lock");

    let resp = result?;
    resp.assert_status_ok();
    let list: Vec<serde_json::Value> = resp.json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "no-token-acct");
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

#[tokio::test]
async fn launch_without_command_returns_400() -> anyhow::Result<()> {
    // No launch command configured.
    let state = test_state();
    let server = test_server(state);
    let resp = server.post("/api/v1/sessions/launch").await;
    resp.assert_status(axum::http::StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn launch_with_empty_body() -> anyhow::Result<()> {
    // Backward compatibility — empty body should work.
    let mut cfg = test_config();
    cfg.launch = Some("echo 'launched'".into());
    let state = Arc::new(MuxState::new(cfg, CancellationToken::new()));
    let server = test_server(state);

    let resp = server.post("/api/v1/sessions/launch").await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["launched"], true);
    Ok(())
}

#[tokio::test]
async fn launch_with_env_vars() -> anyhow::Result<()> {
    // Launch with user-supplied env vars in request body.
    let mut cfg = test_config();
    cfg.launch = Some("echo 'launched with env'".into());
    let state = Arc::new(MuxState::new(cfg, CancellationToken::new()));
    let server = test_server(state);

    let resp = server
        .post("/api/v1/sessions/launch")
        .json(&serde_json::json!({
            "env": {
                "GIT_REPO": "https://github.com/user/repo",
                "WORKING_DIR": "/workspace"
            }
        }))
        .await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["launched"], true);
    Ok(())
}

#[tokio::test]
async fn launch_with_empty_env_object() -> anyhow::Result<()> {
    // Empty env object should behave the same as no body.
    let mut cfg = test_config();
    cfg.launch = Some("echo 'launched'".into());
    let state = Arc::new(MuxState::new(cfg, CancellationToken::new()));
    let server = test_server(state);

    let resp = server.post("/api/v1/sessions/launch").json(&serde_json::json!({ "env": {} })).await;
    resp.assert_status_ok();

    let body: serde_json::Value = resp.json();
    assert_eq!(body["launched"], true);
    Ok(())
}
