// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use axum::http::StatusCode;

use crate::test_support::{AnyhowExt, StoreBuilder, StoreCtx};
use crate::transport::build_router;

/// POST /api/v1/session/profiles registers profiles.
#[tokio::test]
async fn register_profiles_returns_count() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/session/profiles")
        .json(&serde_json::json!({
            "profiles": [
                { "name": "alice", "credentials": { "API_KEY": "key-a" } },
                { "name": "bob", "credentials": { "API_KEY": "key-b" } },
            ]
        }))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["registered"], 2);
    Ok(())
}

/// GET /api/v1/session/profiles lists registered profiles.
#[tokio::test]
async fn list_profiles_returns_registered() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build();
    // Pre-register profiles directly.
    store
        .profile
        .register(
            vec![
                crate::profile::ProfileEntry {
                    name: "alice".to_owned(),
                    credentials: [("API_KEY".to_owned(), "key-a".to_owned())].into(),
                },
                crate::profile::ProfileEntry {
                    name: "bob".to_owned(),
                    credentials: [("API_KEY".to_owned(), "key-b".to_owned())].into(),
                },
            ],
            None,
        )
        .await;

    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/session/profiles").await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["profiles"].as_array().map(|a| a.len()), Some(2));
    assert_eq!(body["active_profile"], "alice");
    assert_eq!(body["profiles"][0]["status"], "active");
    assert_eq!(body["profiles"][1]["status"], "available");
    Ok(())
}
