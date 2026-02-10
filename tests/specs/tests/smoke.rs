// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! End-to-end smoke tests that spawn the real `coop` binary and exercise
//! HTTP, WebSocket, gRPC, Unix socket, and health port transports.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use coop::transport::grpc::proto;
use coop_specs::CoopProcess;

const TIMEOUT: Duration = Duration::from_secs(10);

// -- HTTP (TCP) ---------------------------------------------------------------

#[tokio::test]
async fn http_health() -> anyhow::Result<()> {
    let coop = CoopProcess::start(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let resp: serde_json::Value =
        reqwest::get(format!("{}/api/v1/health", coop.base_url())).await?.json().await?;

    assert_eq!(resp["status"], "running");
    assert_eq!(resp["agent"], "unknown");
    assert!(resp["terminal"]["cols"].is_number());
    assert!(resp["terminal"]["rows"].is_number());
    assert!(resp["pid"].is_number());

    Ok(())
}

#[tokio::test]
async fn http_screen_captures_output() -> anyhow::Result<()> {
    let coop = CoopProcess::start(&["echo", "smoke-marker"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/screen/text", coop.base_url());
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("screen never showed expected output");
        }
        let text = client.get(&url).send().await?.text().await?;
        if text.contains("smoke-marker") {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn http_input_roundtrip() -> anyhow::Result<()> {
    let coop = CoopProcess::start(&["cat"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let client = reqwest::Client::new();
    client
        .post(format!("{}/api/v1/input", coop.base_url()))
        .json(&serde_json::json!({ "text": "hello-roundtrip", "enter": true }))
        .send()
        .await?;

    let url = format!("{}/api/v1/screen/text", coop.base_url());
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("screen never showed input echo");
        }
        let text = client.get(&url).send().await?.text().await?;
        if text.contains("hello-roundtrip") {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn http_shutdown() -> anyhow::Result<()> {
    let mut coop = CoopProcess::start(&["sleep", "60"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let client = reqwest::Client::new();
    let resp: serde_json::Value =
        client.post(format!("{}/api/v1/shutdown", coop.base_url())).send().await?.json().await?;
    assert_eq!(resp["accepted"], true);

    let _status = coop.wait_exit(TIMEOUT).await?;

    Ok(())
}

// -- WebSocket ----------------------------------------------------------------

#[tokio::test]
async fn ws_ping_pong() -> anyhow::Result<()> {
    let coop = CoopProcess::start(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let (mut ws, _) = tokio_tungstenite::connect_async(coop.ws_url()).await?;
    ws.send(Message::Text(r#"{"event":"ping"}"#.into())).await?;

    let msg = tokio::time::timeout(TIMEOUT, ws.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("ws stream ended"))??;

    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => anyhow::bail!("expected text ws message, got: {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(parsed["event"], "pong");

    Ok(())
}

#[tokio::test]
async fn ws_get_health() -> anyhow::Result<()> {
    let coop = CoopProcess::start(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let (mut ws, _) = tokio_tungstenite::connect_async(coop.ws_url()).await?;
    ws.send(Message::Text(r#"{"event":"health:get"}"#.into())).await?;

    let msg = tokio::time::timeout(TIMEOUT, ws.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("ws stream ended"))??;

    let text = match msg {
        Message::Text(t) => t.to_string(),
        other => anyhow::bail!("expected text ws message, got: {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(parsed["event"], "health");
    assert_eq!(parsed["status"], "running");
    assert_eq!(parsed["agent"], "unknown");

    Ok(())
}

// -- gRPC ---------------------------------------------------------------------

#[tokio::test]
async fn grpc_health() -> anyhow::Result<()> {
    let coop = CoopProcess::build().grpc().spawn(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let endpoint = tonic::transport::Channel::from_shared(coop.grpc_url())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let channel = endpoint.connect().await.map_err(|e| anyhow::anyhow!("grpc connect: {e}"))?;
    let mut client = proto::coop_client::CoopClient::new(channel);

    let resp = client.get_health(proto::GetHealthRequest {}).await?.into_inner();
    assert_eq!(resp.status, "running");
    assert_eq!(resp.agent, "unknown");
    assert!(resp.terminal_cols > 0);
    assert!(resp.terminal_rows > 0);

    Ok(())
}

#[tokio::test]
async fn grpc_screen() -> anyhow::Result<()> {
    let coop = CoopProcess::build().grpc().spawn(&["echo", "grpc-screen-test"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let endpoint = tonic::transport::Channel::from_shared(coop.grpc_url())
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let channel = endpoint.connect().await.map_err(|e| anyhow::anyhow!("grpc connect: {e}"))?;
    let mut client = proto::coop_client::CoopClient::new(channel);

    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("grpc screen never showed expected output");
        }
        let resp = client.get_screen(proto::GetScreenRequest { cursor: false }).await?.into_inner();
        let text = resp.lines.join("\n");
        if text.contains("grpc-screen-test") {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// -- Unix socket --------------------------------------------------------------

#[tokio::test]
async fn socket_health() -> anyhow::Result<()> {
    let coop = CoopProcess::build().no_tcp().socket().spawn(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let socket_path = coop.socket_path().ok_or_else(|| anyhow::anyhow!("no socket path"))?;
    let body = coop_specs::unix_http_get(socket_path, "/api/v1/health").await?;
    let resp: serde_json::Value = serde_json::from_str(&body)?;

    assert_eq!(resp["status"], "running");
    assert_eq!(resp["agent"], "unknown");

    Ok(())
}

#[tokio::test]
async fn socket_screen() -> anyhow::Result<()> {
    let coop = CoopProcess::build().no_tcp().socket().spawn(&["echo", "socket-marker"])?;
    coop.wait_healthy(TIMEOUT).await?;

    let socket_path = coop.socket_path().ok_or_else(|| anyhow::anyhow!("no socket path"))?;
    let deadline = tokio::time::Instant::now() + TIMEOUT;

    loop {
        if tokio::time::Instant::now() > deadline {
            anyhow::bail!("socket screen never showed expected output");
        }
        let body = coop_specs::unix_http_get(socket_path, "/api/v1/screen/text").await?;
        if body.contains("socket-marker") {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// -- Health port --------------------------------------------------------------

#[tokio::test]
async fn health_port_serves_health() -> anyhow::Result<()> {
    let coop = CoopProcess::build().health().spawn(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    // Health endpoint works on the health port
    let resp: serde_json::Value =
        reqwest::get(format!("{}/api/v1/health", coop.health_url())).await?.json().await?;
    assert_eq!(resp["status"], "running");

    Ok(())
}

#[tokio::test]
async fn health_port_rejects_other_routes() -> anyhow::Result<()> {
    let coop = CoopProcess::build().health().spawn(&["sleep", "10"])?;
    coop.wait_healthy(TIMEOUT).await?;

    // Screen endpoint should 404 on the health-only port
    let resp = reqwest::get(format!("{}/api/v1/screen", coop.health_url())).await?;
    assert_eq!(resp.status().as_u16(), 404);

    // Input endpoint should 404 on the health-only port
    let resp = reqwest::Client::new()
        .post(format!("{}/api/v1/input", coop.health_url()))
        .json(&serde_json::json!({ "text": "x" }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}
