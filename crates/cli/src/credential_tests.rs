// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use axum::routing::post;
use axum::Router;
use tokio::net::TcpListener;

use super::*;

fn test_config(name: &str, token_url: &str) -> CredentialConfig {
    CredentialConfig {
        accounts: vec![AccountConfig {
            name: name.to_owned(),
            provider: "claude".to_owned(),
            token_url: Some(token_url.to_owned()),
            client_id: Some("test-client".to_owned()),
            r#static: false,
            auto_reauth: true,
            device_auth_url: None,
            authorize_url: None,
            redirect_uri: None,
            refresh_margin_secs: 5,
        }],
        persist_path: None,
    }
}

fn static_config(name: &str) -> CredentialConfig {
    CredentialConfig {
        accounts: vec![AccountConfig {
            name: name.to_owned(),
            provider: "anthropic".to_owned(),
            token_url: None,
            client_id: None,
            device_auth_url: None,
            authorize_url: None,
            redirect_uri: None,
            r#static: true,
            auto_reauth: false,
            refresh_margin_secs: 0,
        }],
        persist_path: None,
    }
}

#[tokio::test]
async fn new_creates_accounts_from_config() {
    let config = CredentialConfig {
        accounts: vec![
            AccountConfig {
                name: "personal".to_owned(),
                provider: "claude".to_owned(),
                token_url: Some("http://localhost/token".to_owned()),
                client_id: Some("client-1".to_owned()),
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: false,
                auto_reauth: true,
                refresh_margin_secs: 900,
            },
            AccountConfig {
                name: "api-key".to_owned(),
                provider: "anthropic".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: true,
                auto_reauth: false,
                refresh_margin_secs: 0,
            },
        ],
        persist_path: None,
    };

    let (broker, _rx) = CredentialBroker::new(&config);
    let status = broker.status().await;

    assert_eq!(status.len(), 2);

    let personal = status.iter().find(|s| s.name == "personal");
    assert!(personal.is_some());
    assert_eq!(personal.map(|p| &p.status), Some(&AccountStatus::Expired));

    let api = status.iter().find(|s| s.name == "api-key");
    assert!(api.is_some());
    assert_eq!(api.map(|a| &a.status), Some(&AccountStatus::Static));
}

#[tokio::test]
async fn seed_sets_credentials_and_healthy_status() {
    let config = test_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let seeded =
        broker.seed("test", "access-123".into(), Some("refresh-456".into()), Some(3600)).await;
    assert!(seeded);

    let status = broker.status().await;
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].status, AccountStatus::Healthy);
    assert!(status[0].expires_in_secs.is_some());

    let creds = broker.credentials_for("test").await;
    assert!(creds.is_some());
    let creds = creds.expect("creds should exist");
    assert_eq!(creds.get("ANTHROPIC_API_KEY"), Some(&"access-123".to_owned()));
}

#[tokio::test]
async fn seed_unknown_account_returns_false() {
    let config = test_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);
    assert!(!broker.seed("unknown", "tok".into(), None, None).await);
}

#[tokio::test]
async fn static_account_returns_static_status() {
    let config = static_config("api-key");
    let (broker, _rx) = CredentialBroker::new(&config);

    broker.seed("api-key", "sk-ant-xxx".into(), None, None).await;

    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Static);

    let creds = broker.credentials_for("api-key").await;
    assert!(creds.is_some());
    assert_eq!(creds.expect("creds").get("ANTHROPIC_API_KEY"), Some(&"sk-ant-xxx".to_owned()));
}

#[tokio::test]
async fn credentials_for_maps_provider_to_env_key() {
    let config = CredentialConfig {
        accounts: vec![
            AccountConfig {
                name: "openai".to_owned(),
                provider: "openai".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: true,
                auto_reauth: false,
                refresh_margin_secs: 0,
            },
            AccountConfig {
                name: "google".to_owned(),
                provider: "gemini".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: true,
                auto_reauth: false,
                refresh_margin_secs: 0,
            },
        ],
        persist_path: None,
    };

    let (broker, _rx) = CredentialBroker::new(&config);
    broker.seed("openai", "sk-openai".into(), None, None).await;
    broker.seed("google", "goog-key".into(), None, None).await;

    let oai = broker.credentials_for("openai").await.expect("openai creds");
    assert!(oai.contains_key("OPENAI_API_KEY"));

    let goog = broker.credentials_for("google").await.expect("google creds");
    assert!(goog.contains_key("GOOGLE_API_KEY"));
}

#[tokio::test]
async fn all_credentials_excludes_revoked() {
    let config = CredentialConfig {
        accounts: vec![
            AccountConfig {
                name: "good".to_owned(),
                provider: "claude".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: true,
                auto_reauth: false,
                refresh_margin_secs: 0,
            },
            AccountConfig {
                name: "bad".to_owned(),
                provider: "claude".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                authorize_url: None,
                redirect_uri: None,
                r#static: false,
                auto_reauth: true,
                refresh_margin_secs: 0,
            },
        ],
        persist_path: None,
    };

    let (broker, _rx) = CredentialBroker::new(&config);
    broker.seed("good", "good-tok".into(), None, None).await;
    broker.seed("bad", "bad-tok".into(), None, None).await;

    // Manually mark "bad" as revoked.
    {
        let mut accounts = broker.accounts.write().await;
        accounts.get_mut("bad").expect("bad account").status = AccountStatus::Revoked;
    }

    let all = broker.all_credentials().await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "good");
}

/// Helper: start a mock OAuth token server that returns configurable responses.
async fn mock_token_server(responses: Vec<(u16, String)>) -> (SocketAddr, Arc<AtomicU32>) {
    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = Arc::clone(&call_count);
    let responses = Arc::new(responses);

    let app = Router::new().route(
        "/token",
        post(move |_body: String| {
            let count = Arc::clone(&call_count_clone);
            let resps = Arc::clone(&responses);
            async move {
                let idx = count.fetch_add(1, Ordering::Relaxed) as usize;
                let (status, body) = if idx < resps.len() {
                    resps[idx].clone()
                } else {
                    // Default: repeat last response.
                    resps.last().cloned().unwrap_or((500, "{}".to_owned()))
                };
                (
                    axum::http::StatusCode::from_u16(status)
                        .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
                    body,
                )
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });

    (addr, call_count)
}

#[tokio::test]
async fn do_refresh_success() {
    let success_body = serde_json::json!({
        "access_token": "new-access",
        "refresh_token": "new-refresh",
        "expires_in": 3600
    })
    .to_string();

    let (addr, call_count) = mock_token_server(vec![(200, success_body)]).await;
    let token_url = format!("http://{addr}/token");

    let config = test_config("test", &token_url);
    let (broker, mut rx) = CredentialBroker::new(&config);
    broker.seed("test", "old-access".into(), Some("old-refresh".into()), Some(10)).await;

    // Drain the Refreshed event emitted by seed().
    let _ = rx.try_recv();

    // Trigger refresh directly.
    broker.refresh_with_retries("test").await;

    assert_eq!(call_count.load(Ordering::Relaxed), 1);

    // Verify state updated.
    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Healthy);

    let creds = broker.credentials_for("test").await.expect("creds");
    assert_eq!(creds.get("ANTHROPIC_API_KEY"), Some(&"new-access".to_owned()));

    // Verify event was broadcast.
    let event = rx.try_recv();
    assert!(event.is_ok());
    match event.expect("event") {
        CredentialEvent::Refreshed { account, credentials } => {
            assert_eq!(account, "test");
            assert_eq!(credentials.get("ANTHROPIC_API_KEY"), Some(&"new-access".to_owned()));
        }
        other => panic!("expected Refreshed, got {other:?}"),
    }
}

#[tokio::test]
async fn do_refresh_invalid_grant_marks_revoked() {
    let error_body = serde_json::json!({
        "error": "invalid_grant",
        "error_description": "Refresh token not found or invalid"
    })
    .to_string();

    let (addr, _count) = mock_token_server(vec![(400, error_body)]).await;
    let token_url = format!("http://{addr}/token");

    let config = test_config("test", &token_url);
    let (broker, mut rx) = CredentialBroker::new(&config);
    broker.seed("test", "old".into(), Some("dead-refresh".into()), Some(10)).await;

    // Drain the Refreshed event emitted by seed().
    let _ = rx.try_recv();

    broker.refresh_with_retries("test").await;

    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Revoked);

    // Should get RefreshFailed event followed by ReauthRequired (auto-reauth).
    let event = rx.try_recv();
    assert!(event.is_ok());
    match event.expect("event") {
        CredentialEvent::RefreshFailed { account, .. } => {
            assert_eq!(account, "test");
        }
        other => panic!("expected RefreshFailed, got {other:?}"),
    }

    // Auto-reauth triggers ReauthRequired with the login-reauth URL.
    let event2 = rx.try_recv();
    assert!(event2.is_ok());
    match event2.expect("event") {
        CredentialEvent::ReauthRequired { account, auth_url, .. } => {
            assert_eq!(account, "test");
            assert!(
                auth_url.contains("oauth/authorize"),
                "auth_url should be a login-reauth URL: {auth_url}"
            );
        }
        other => panic!("expected ReauthRequired, got {other:?}"),
    }
}

#[tokio::test]
async fn do_refresh_transient_retries_then_succeeds() {
    let error_body = serde_json::json!({
        "error": "server_error",
        "error_description": "temporary"
    })
    .to_string();
    let success_body = serde_json::json!({
        "access_token": "recovered",
        "expires_in": 3600
    })
    .to_string();

    let (addr, call_count) =
        mock_token_server(vec![(500, error_body.clone()), (500, error_body), (200, success_body)])
            .await;
    let token_url = format!("http://{addr}/token");

    let config = test_config("test", &token_url);
    let (broker, _rx) = CredentialBroker::new(&config);
    broker.seed("test", "old".into(), Some("refresh".into()), Some(10)).await;

    broker.refresh_with_retries("test").await;

    // Should have called 3 times (2 failures + 1 success).
    assert_eq!(call_count.load(Ordering::Relaxed), 3);

    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Healthy);
}

#[tokio::test]
async fn empty_config_produces_empty_broker() {
    let config = CredentialConfig::default();
    let (broker, _rx) = CredentialBroker::new(&config);
    assert!(broker.status().await.is_empty());
    assert!(broker.all_credentials().await.is_empty());
}

// ---------------------------------------------------------------------------
// Claude credential extraction tests
// ---------------------------------------------------------------------------

#[test]
fn parse_flat_credentials() {
    let json = r#"{
        "accessToken": "sk-ant-oat01-test-access",
        "refreshToken": "sk-ant-ort01-test-refresh",
        "expiresAt": 9999999999999
    }"#;
    let creds = parse_claude_credentials(json).expect("should parse");
    assert_eq!(creds.access_token, "sk-ant-oat01-test-access");
    assert_eq!(creds.refresh_token.as_deref(), Some("sk-ant-ort01-test-refresh"));
    assert!(creds.expires_in_secs > 0, "far-future expiry should have positive TTL");
}

#[test]
fn parse_nested_credentials() {
    let json = r#"{
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-nested",
            "refreshToken": "sk-ant-ort01-nested",
            "expiresAt": 9999999999999,
            "subscriptionType": "max"
        }
    }"#;
    let creds = parse_claude_credentials(json).expect("should parse");
    assert_eq!(creds.access_token, "sk-ant-oat01-nested");
    assert_eq!(creds.refresh_token.as_deref(), Some("sk-ant-ort01-nested"));
}

#[test]
fn parse_expired_credentials() {
    let json = r#"{"accessToken": "sk-expired", "expiresAt": 1000}"#;
    let creds = parse_claude_credentials(json).expect("should parse");
    assert_eq!(creds.access_token, "sk-expired");
    assert_eq!(creds.expires_in_secs, 0);
}

#[test]
fn parse_no_refresh_token() {
    let json = r#"{"accessToken": "sk-no-refresh"}"#;
    let creds = parse_claude_credentials(json).expect("should parse");
    assert_eq!(creds.access_token, "sk-no-refresh");
    assert!(creds.refresh_token.is_none());
    assert_eq!(creds.expires_in_secs, 0);
}

#[test]
fn parse_empty_access_token_fails() {
    let json = r#"{"accessToken": ""}"#;
    assert!(parse_claude_credentials(json).is_err());
}

#[test]
fn parse_missing_access_token_fails() {
    let json = r#"{"refreshToken": "has-refresh-but-no-access"}"#;
    assert!(parse_claude_credentials(json).is_err());
}

#[test]
fn parse_invalid_json_fails() {
    assert!(parse_claude_credentials("not json").is_err());
}

#[test]
fn extract_from_nonexistent_dir_fails() {
    let dir = std::path::Path::new("/tmp/coop-test-nonexistent-dir-xyz");
    assert!(extract_claude_credentials(Some(dir)).is_err());
}

#[test]
fn extract_from_temp_dir() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cred_path = dir.path().join(".credentials.json");
    std::fs::write(
        &cred_path,
        r#"{"accessToken":"sk-tmp-test","refreshToken":"sk-tmp-refresh","expiresAt":9999999999999}"#,
    )?;

    let creds = extract_claude_credentials(Some(dir.path()))?;
    assert_eq!(creds.access_token, "sk-tmp-test");
    assert_eq!(creds.refresh_token.as_deref(), Some("sk-tmp-refresh"));
    assert!(creds.expires_in_secs > 0);
    Ok(())
}

#[tokio::test]
async fn seed_from_claude_config_integrates() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cred_path = dir.path().join(".credentials.json");
    std::fs::write(
        &cred_path,
        r#"{"accessToken":"sk-from-config","refreshToken":"sk-refresh-config","expiresAt":9999999999999}"#,
    )?;

    let config = test_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let creds = broker.seed_from_claude_config("test", Some(dir.path())).await?;
    assert_eq!(creds.access_token, "sk-from-config");

    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Healthy);

    let broker_creds = broker.credentials_for("test").await.expect("creds");
    assert_eq!(broker_creds.get("ANTHROPIC_API_KEY"), Some(&"sk-from-config".to_owned()));
    Ok(())
}

#[tokio::test]
async fn seed_from_claude_config_unknown_account_fails() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let cred_path = dir.path().join(".credentials.json");
    std::fs::write(&cred_path, r#"{"accessToken":"sk-test"}"#)?;

    let config = test_config("real-account", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let result = broker.seed_from_claude_config("nonexistent", Some(dir.path())).await;
    assert!(result.is_err());
    Ok(())
}

// ---------------------------------------------------------------------------
// Login-reauth (authorization code flow) tests
// ---------------------------------------------------------------------------

fn login_reauth_config(name: &str, token_url: &str) -> CredentialConfig {
    CredentialConfig {
        accounts: vec![AccountConfig {
            name: name.to_owned(),
            provider: "claude".to_owned(),
            token_url: Some(token_url.to_owned()),
            client_id: Some("9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_owned()),
            device_auth_url: None,
            authorize_url: Some("https://claude.ai/oauth/authorize".to_owned()),
            redirect_uri: Some("https://platform.claude.com/oauth/code/callback".to_owned()),
            r#static: false,
            auto_reauth: true,
            refresh_margin_secs: 5,
        }],
        persist_path: None,
    }
}

#[tokio::test]
async fn initiate_login_reauth_returns_auth_url() {
    let config = login_reauth_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let session = broker.initiate_login_reauth("test").await.expect("should succeed");
    assert_eq!(session.account, "test");
    assert!(session.auth_url.starts_with("https://claude.ai/oauth/authorize?"));
    assert!(session.auth_url.contains("code=true"));
    assert!(session.auth_url.contains("response_type=code"));
    assert!(session.auth_url.contains("client_id=9d1c250a"));
    assert!(session.auth_url.contains("redirect_uri="));
    assert!(session.auth_url.contains("scope=user%3Aprofile"));
    assert!(session.auth_url.contains("code_challenge="));
    assert!(session.auth_url.contains("code_challenge_method=S256"));
    assert!(!session.state.is_empty());
    assert!(!session.code_verifier.is_empty());
    assert_eq!(session.redirect_uri, "https://platform.claude.com/oauth/code/callback");
    assert_eq!(session.client_id, "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
}

#[tokio::test]
async fn initiate_login_reauth_uses_defaults_without_config() {
    // Config without explicit authorize_url/redirect_uri falls back to defaults.
    let config = test_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let session = broker.initiate_login_reauth("test").await.expect("should succeed");
    assert!(session.auth_url.starts_with("https://claude.ai/oauth/authorize?"));
    assert_eq!(session.redirect_uri, "https://platform.claude.com/oauth/code/callback");
}

#[tokio::test]
async fn initiate_login_reauth_unknown_account_fails() {
    let config = test_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let result = broker.initiate_login_reauth("nonexistent").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn initiate_login_reauth_deduplicates() {
    let config = login_reauth_config("test", "http://localhost/token");
    let (broker, _rx) = CredentialBroker::new(&config);

    let session1 = broker.initiate_login_reauth("test").await.expect("first should succeed");
    let session2 = broker.initiate_login_reauth("test").await.expect("second should return existing");

    // Second call returns the same session (same state, same auth_url).
    assert_eq!(session1.state, session2.state);
    assert_eq!(session1.auth_url, session2.auth_url);
}

#[tokio::test]
async fn complete_login_reauth_exchanges_code() {
    let success_body = serde_json::json!({
        "access_token": "sk-ant-from-code-exchange",
        "refresh_token": "sk-ant-refresh-from-code",
        "expires_in": 7200
    })
    .to_string();

    let (addr, call_count) = mock_token_server(vec![(200, success_body)]).await;
    let token_url = format!("http://{addr}/token");

    let config = login_reauth_config("test", &token_url);
    let (broker, _rx) = CredentialBroker::new(&config);

    // Initiate first so the PKCE code_verifier is stored in pending_reauth.
    broker.initiate_login_reauth("test").await.expect("initiate");

    broker
        .complete_login_reauth(
            "test",
            "auth-code-123",
            "https://platform.claude.com/oauth/code/callback",
            "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
        )
        .await
        .expect("should succeed");

    assert_eq!(call_count.load(Ordering::Relaxed), 1);

    let status = broker.status().await;
    assert_eq!(status[0].status, AccountStatus::Healthy);

    let creds = broker.credentials_for("test").await.expect("creds");
    assert_eq!(creds.get("ANTHROPIC_API_KEY"), Some(&"sk-ant-from-code-exchange".to_owned()));
}

#[tokio::test]
async fn complete_login_reauth_invalid_code_fails() {
    let error_body = serde_json::json!({
        "error": "invalid_grant",
        "error_description": "authorization code expired"
    })
    .to_string();

    let (addr, _) = mock_token_server(vec![(400, error_body)]).await;
    let token_url = format!("http://{addr}/token");

    let config = login_reauth_config("test", &token_url);
    let (broker, _rx) = CredentialBroker::new(&config);

    // Initiate first so the PKCE code_verifier is stored in pending_reauth.
    broker.initiate_login_reauth("test").await.expect("initiate");

    let result = broker
        .complete_login_reauth(
            "test",
            "expired-code",
            "https://platform.claude.com/oauth/code/callback",
            "client-id",
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.contains("invalid_grant"), "error should mention invalid_grant: {err}");
}
