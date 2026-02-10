// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::test_support::{AnyhowExt, StoreBuilder, StoreCtx};
use crate::transport::build_router;
use crate::usage::UsageDelta;

/// GET /api/v1/session/usage returns zero counters initially.
#[tokio::test]
async fn usage_returns_zero_initially() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build();
    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/session/usage").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["input_tokens"], 0);
    assert_eq!(body["output_tokens"], 0);
    assert_eq!(body["request_count"], 0);
    Ok(())
}

/// GET /api/v1/session/usage reflects accumulated deltas.
#[tokio::test]
async fn usage_reflects_accumulated_deltas() -> anyhow::Result<()> {
    let StoreCtx { store, .. } = StoreBuilder::new().build();

    store
        .usage
        .accumulate(UsageDelta {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            cost_usd: 0.005,
            duration_api_ms: 1200,
        })
        .await;

    let app = build_router(store);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/session/usage").await;
    resp.assert_status_ok();
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["input_tokens"], 100);
    assert_eq!(body["output_tokens"], 50);
    assert_eq!(body["cache_read_tokens"], 20);
    assert_eq!(body["cache_write_tokens"], 10);
    assert_eq!(body["request_count"], 1);
    assert_eq!(body["total_api_ms"], 1200);
    Ok(())
}
