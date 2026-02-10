// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! NATS publisher integration tests using a real `nats-server` process.
//!
//! These tests require `nats-server` on `$PATH`.  If it's unavailable the
//! tests are skipped (not failed).

use std::time::Duration;

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use coop::driver::AgentState;
use coop::event::TransitionEvent;
use coop::stop::{StopEvent, StopType};
use coop::test_support::{NatsServer, StoreBuilder};
use coop::transport::nats_pub::{NatsPublisher, StateEventPayload, StopEventPayload};

const RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// Skip test if `nats-server` is not available.
macro_rules! require_nats {
    () => {
        match NatsServer::start() {
            Some(s) => s,
            None => {
                eprintln!("nats-server not found — skipping test");
                return Ok(());
            }
        }
    };
}

/// Connect a bare `async_nats::Client` to the test server.
async fn nats_client(server: &NatsServer) -> anyhow::Result<async_nats::Client> {
    let client = async_nats::connect(&server.url()).await?;
    Ok(client)
}

#[tokio::test]
async fn nats_publishes_state_transition() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    // Subscribe before publisher starts so we don't miss the message.
    let sub_client = nats_client(&server).await?;
    let mut sub = sub_client.subscribe("test.state").await?;

    // Start publisher
    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "test".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    // Send a state transition via broadcast channel.
    let _ = store.channels.state_tx.send(TransitionEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 1,
        cause: String::new(),
        last_message: None,
    });

    // Receive on NATS subscription.
    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;

    let payload: StateEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.prev, "starting");
    assert_eq!(payload.next, "working");
    assert_eq!(payload.seq, 1);
    assert_eq!(payload.cause, None);
    assert_eq!(payload.last_message, None);

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_publishes_state_with_cause_and_message() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let sub_client = nats_client(&server).await?;
    let mut sub = sub_client.subscribe("ev.state").await?;

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "ev".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    let _ = store.channels.state_tx.send(TransitionEvent {
        prev: AgentState::Working,
        next: AgentState::Idle,
        seq: 42,
        cause: "tool completed".to_owned(),
        last_message: Some("I finished the task".to_owned()),
    });

    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;

    let payload: StateEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.prev, "working");
    assert_eq!(payload.next, "idle");
    assert_eq!(payload.seq, 42);
    assert_eq!(payload.cause.as_deref(), Some("tool completed"));
    assert_eq!(payload.last_message.as_deref(), Some("I finished the task"));

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_publishes_stop_event() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let sub_client = nats_client(&server).await?;
    let mut sub = sub_client.subscribe("test.stop").await?;

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "test".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    // Emit a stop event.
    store.stop.emit(StopType::Allowed, None, None);

    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;

    let payload: StopEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.stop_type, "allowed");
    assert_eq!(payload.signal_json, None);
    assert_eq!(payload.error_detail, None);
    assert_eq!(payload.seq, 0);

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_publishes_stop_event_with_signal() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let sub_client = nats_client(&server).await?;
    let mut sub = sub_client.subscribe("test.stop").await?;

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "test".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    let signal_body = serde_json::json!({"status": "done", "message": "all tasks complete"});
    store.stop.emit(StopType::Signaled, Some(signal_body.clone()), None);

    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;

    let payload: StopEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.stop_type, "signaled");
    assert!(payload.signal_json.is_some());
    // signal_json is the stringified JSON value.
    let parsed: serde_json::Value =
        serde_json::from_str(payload.signal_json.as_deref().unwrap_or(""))?;
    assert_eq!(parsed, signal_body);
    assert_eq!(payload.seq, 0);

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_publishes_stop_event_with_error() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let sub_client = nats_client(&server).await?;
    let mut sub = sub_client.subscribe("test.stop").await?;

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "test".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    store.stop.emit(StopType::Error, None, Some("unrecoverable failure".to_owned()));

    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;

    let payload: StopEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.stop_type, "error");
    assert_eq!(payload.error_detail.as_deref(), Some("unrecoverable failure"));

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_shutdown_stops_publisher() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "test".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    // Cancel immediately.
    shutdown.cancel();

    // Publisher task should finish promptly.
    tokio::time::timeout(Duration::from_secs(2), pub_handle).await??;
    Ok(())
}

#[tokio::test]
async fn nats_connect_via_config() -> anyhow::Result<()> {
    let server = require_nats!();

    let config = coop::transport::nats_pub::NatsPubConfig {
        url: server.url(),
        token: None,
        prefix: "cfg".to_owned(),
    };

    let publisher = NatsPublisher::connect(&config).await?;

    // Quick smoke test: publisher connected successfully.
    // Verify by running it briefly and shutting down.
    let (state_tx, _) = tokio::sync::broadcast::channel::<TransitionEvent>(8);
    let (stop_tx, _) = tokio::sync::broadcast::channel::<StopEvent>(8);
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let handle = tokio::spawn(async move {
        publisher.run(state_tx.subscribe(), stop_tx.subscribe(), sd).await;
    });
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), handle).await??;
    Ok(())
}

#[tokio::test]
async fn nats_concurrent_subscribers_receive_events() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    // Create 3 NATS subscribers.
    let mut subs = Vec::new();
    for _ in 0..3 {
        let client = nats_client(&server).await?;
        let sub = client.subscribe("multi.state").await?;
        subs.push(sub);
    }

    // Start publisher.
    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "multi".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    // Send one event.
    let _ = store.channels.state_tx.send(TransitionEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 7,
        cause: String::new(),
        last_message: None,
    });

    // All 3 should receive it.
    for sub in &mut subs {
        let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
            .await?
            .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;
        let payload: StateEventPayload = serde_json::from_slice(&msg.payload)?;
        assert_eq!(payload.next, "working");
    }

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}

#[tokio::test]
async fn nats_subject_prefix_is_respected() -> anyhow::Result<()> {
    let server = require_nats!();
    let (store, _rx) = StoreBuilder::new().build();

    let sub_client = nats_client(&server).await?;
    // Subscribe to custom prefix.
    let mut sub = sub_client.subscribe("my.custom.prefix.state").await?;
    // Also subscribe to default prefix — should NOT receive.
    let mut wrong_sub = sub_client.subscribe("coop.events.state").await?;

    let pub_client = nats_client(&server).await?;
    let publisher = NatsPublisher::new(pub_client, "my.custom.prefix".to_owned());
    let state_rx = store.channels.state_tx.subscribe();
    let stop_rx = store.stop.stop_tx.subscribe();
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let pub_handle = tokio::spawn(async move {
        publisher.run(state_rx, stop_rx, sd).await;
    });

    let _ = store.channels.state_tx.send(TransitionEvent {
        prev: AgentState::Working,
        next: AgentState::Idle,
        seq: 1,
        cause: String::new(),
        last_message: None,
    });

    // Correct prefix should receive.
    let msg = tokio::time::timeout(RECV_TIMEOUT, sub.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("subscription closed"))?;
    let payload: StateEventPayload = serde_json::from_slice(&msg.payload)?;
    assert_eq!(payload.next, "idle");

    // Wrong prefix should timeout.
    let result = tokio::time::timeout(Duration::from_millis(200), wrong_sub.next()).await;
    assert!(result.is_err(), "wrong prefix should not receive events");

    shutdown.cancel();
    let _ = pub_handle.await;
    Ok(())
}
