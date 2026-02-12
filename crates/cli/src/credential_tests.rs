// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
            device_auth_url: None,
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
            r#static: true,
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
                r#static: false,
                refresh_margin_secs: 900,
            },
            AccountConfig {
                name: "api-key".to_owned(),
                provider: "anthropic".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                r#static: true,
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
                r#static: true,
                refresh_margin_secs: 0,
            },
            AccountConfig {
                name: "google".to_owned(),
                provider: "gemini".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                r#static: true,
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
                r#static: true,
                refresh_margin_secs: 0,
            },
            AccountConfig {
                name: "bad".to_owned(),
                provider: "claude".to_owned(),
                token_url: None,
                client_id: None,
                device_auth_url: None,
                r#static: false,
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

    // Should get RefreshFailed event (not multiple — revoked stops retries).
    let event = rx.try_recv();
    assert!(event.is_ok());
    match event.expect("event") {
        CredentialEvent::RefreshFailed { account, .. } => {
            assert_eq!(account, "test");
        }
        other => panic!("expected RefreshFailed, got {other:?}"),
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
