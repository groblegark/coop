// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use axum::http::StatusCode;

use crate::test_support::{AnyhowExt, StoreBuilder, StoreCtx};
use crate::transport::build_router;

/// PUT /api/v1/env/:key stores a pending var, GET reads it back.
#[tokio::test]
async fn put_then_get_pending_env() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().child_pid(1234).build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .put("/api/v1/env/MY_VAR")
        .json(&serde_json::json!({ "value": "hello" }))
        .await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["key"], "MY_VAR");
    assert_eq!(body["updated"], true);

    let resp = server.get("/api/v1/env/MY_VAR").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["key"], "MY_VAR");
    assert_eq!(body["value"], "hello");
    assert_eq!(body["source"], "pending");
    Ok(())
}

/// DELETE /api/v1/env/:key removes a pending var.
#[tokio::test]
async fn delete_pending_env() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().child_pid(1234).build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Store a value first.
    server.put("/api/v1/env/TEMP").json(&serde_json::json!({ "value": "x" })).await;

    let resp = server.delete("/api/v1/env/TEMP").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["updated"], true);

    // Deleting again should return updated=false.
    let resp = server.delete("/api/v1/env/TEMP").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["updated"], false);
    Ok(())
}

/// GET /api/v1/env/:key for a non-existent key returns null value with source=child.
#[tokio::test]
async fn get_nonexistent_env_returns_null() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().child_pid(1234).build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/env/DOES_NOT_EXIST").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["key"], "DOES_NOT_EXIST");
    assert!(body["value"].is_null());
    assert_eq!(body["source"], "child");
    Ok(())
}

/// GET /api/v1/env lists all pending vars (child vars empty on non-Linux).
#[tokio::test]
async fn list_env_includes_pending() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().child_pid(1234).build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    server.put("/api/v1/env/A").json(&serde_json::json!({ "value": "1" })).await;
    server.put("/api/v1/env/B").json(&serde_json::json!({ "value": "2" })).await;

    let resp = server.get("/api/v1/env").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["pending"]["A"], "1");
    assert_eq!(body["pending"]["B"], "2");
    Ok(())
}

/// GET /api/v1/session/cwd returns 410 when child_pid is 0 (not running).
#[tokio::test]
async fn cwd_returns_410_when_no_child() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build(); // child_pid = 0
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/session/cwd").await;
    resp.assert_status(StatusCode::GONE);
    Ok(())
}

/// GET /api/v1/env returns 410 when child_pid is 0 (not running).
#[tokio::test]
async fn list_env_returns_410_when_no_child() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build(); // child_pid = 0
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/env").await;
    resp.assert_status(StatusCode::GONE);
    Ok(())
}

/// PUT /api/v1/env/:key overwrites an existing pending var.
#[tokio::test]
async fn put_env_overwrites_existing() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().child_pid(1234).build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    server.put("/api/v1/env/X").json(&serde_json::json!({ "value": "old" })).await;
    server.put("/api/v1/env/X").json(&serde_json::json!({ "value": "new" })).await;

    let resp = server.get("/api/v1/env/X").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["value"], "new");
    Ok(())
}
