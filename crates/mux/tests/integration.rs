// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for coop-mux HTTP API and aggregator.

use std::sync::Arc;

use axum::http::StatusCode;
use tokio_util::sync::CancellationToken;

use coop_mux::config::MuxConfig;
use coop_mux::state::{MuxEvent, MuxState};
use coop_mux::transport::build_router;

/// Build a test server with default config (no auth).
fn test_state() -> Arc<MuxState> {
    let config = MuxConfig {
        host: "127.0.0.1".to_owned(),
        port: 0,
        auth_token: None,
        screen_poll_ms: 60000, // slow polling for tests
        status_poll_ms: 60000,
        health_check_ms: 60000,
        max_health_failures: 3,
    };
    Arc::new(MuxState::new(config, CancellationToken::new()))
}

// -- Health endpoint ----------------------------------------------------------

#[tokio::test]
async fn health_returns_session_count() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.get("/api/v1/health").await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "running");
    assert_eq!(body["session_count"], 0);
}

// -- Session list (empty) -----------------------------------------------------

#[tokio::test]
async fn list_sessions_empty() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.get("/api/v1/sessions").await;
    resp.assert_status(StatusCode::OK);
    let body: Vec<serde_json::Value> = resp.json();
    assert!(body.is_empty());
}

// -- Deregister unknown session -----------------------------------------------

#[tokio::test]
async fn deregister_unknown_session_returns_404() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.delete("/api/v1/sessions/nonexistent").await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// -- Cached screen for unknown session ----------------------------------------

#[tokio::test]
async fn session_screen_unknown_returns_404() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.get("/api/v1/sessions/unknown/screen").await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// -- Cached status for unknown session ----------------------------------------

#[tokio::test]
async fn session_status_unknown_returns_404() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.get("/api/v1/sessions/unknown/status").await;
    resp.assert_status(StatusCode::NOT_FOUND);
}

// -- Dashboard HTML -----------------------------------------------------------

#[tokio::test]
async fn mux_dashboard_serves_html() {
    let state = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).expect("create test server");

    let resp = server.get("/mux").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("Coop Multiplexer"));
    assert!(body.contains("xterm"));
}

// -- Aggregator unit tests ----------------------------------------------------

#[tokio::test]
async fn aggregator_subscribe_receives_events() {
    let state = test_state();

    let mut rx = state.aggregator.subscribe();

    // Emit an event directly.
    let _ = state.aggregator.event_tx.send(MuxEvent::SessionOnline {
        session: "test-1".to_owned(),
        url: "http://localhost:8080".to_owned(),
    });

    let event = rx.recv().await.expect("should receive event");
    match event {
        MuxEvent::SessionOnline { session, url } => {
            assert_eq!(session, "test-1");
            assert_eq!(url, "http://localhost:8080");
        }
        _ => panic!("unexpected event type"),
    }
}

#[tokio::test]
async fn aggregator_cached_state_empty_by_default() {
    let state = test_state();
    let cached = state.aggregator.cached_state().await;
    assert!(cached.is_empty());
}

#[tokio::test]
async fn aggregator_cache_updates_on_write() {
    let state = test_state();

    // Simulate what the aggregator_feed does.
    {
        let mut cache = state.aggregator.cache.write().await;
        let entry = cache.entry("test-1".to_owned()).or_default();
        entry.agent_state = Some("idle".to_owned());
        entry.screen_cols = 120;
        entry.screen_rows = 40;
    }

    let cached = state.aggregator.cached_state().await;
    assert_eq!(cached.len(), 1);
    let entry = &cached["test-1"];
    assert_eq!(entry.agent_state.as_deref(), Some("idle"));
    assert_eq!(entry.screen_cols, 120);
    assert_eq!(entry.screen_rows, 40);
    assert!(entry.credential_status.is_none());
}

// -- MuxEvent serialization ---------------------------------------------------

#[test]
fn mux_event_state_serialization() {
    let event = MuxEvent::State {
        session: "s1".to_owned(),
        prev: "idle".to_owned(),
        next: "working".to_owned(),
        seq: 42,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "state");
    assert_eq!(json["session"], "s1");
    assert_eq!(json["prev"], "idle");
    assert_eq!(json["next"], "working");
    assert_eq!(json["seq"], 42);
}

#[test]
fn mux_event_screen_serialization() {
    let event = MuxEvent::Screen {
        session: "s1".to_owned(),
        lines: vec!["hello".to_owned(), "world".to_owned()],
        cols: 80,
        rows: 24,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "screen");
    assert_eq!(json["session"], "s1");
    assert_eq!(json["cols"], 80);
    assert_eq!(json["rows"], 24);
    assert_eq!(json["lines"].as_array().map(|a| a.len()), Some(2));
}

#[test]
fn mux_event_credential_serialization() {
    let event = MuxEvent::Credential {
        session: "s1".to_owned(),
        account: "personal".to_owned(),
        status: "healthy".to_owned(),
        error: None,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "credential");
    assert_eq!(json["session"], "s1");
    assert_eq!(json["account"], "personal");
    assert_eq!(json["status"], "healthy");
    assert!(json.get("error").is_none()); // skip_serializing_if = None
}

#[test]
fn mux_event_credential_with_error_serialization() {
    let event = MuxEvent::Credential {
        session: "s1".to_owned(),
        account: "personal".to_owned(),
        status: "refresh_failed".to_owned(),
        error: Some("token expired".to_owned()),
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["error"], "token expired");
}

#[test]
fn mux_event_session_online_serialization() {
    let event = MuxEvent::SessionOnline {
        session: "s1".to_owned(),
        url: "http://localhost:8080".to_owned(),
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "session_online");
    assert_eq!(json["session"], "s1");
    assert_eq!(json["url"], "http://localhost:8080");
}

#[test]
fn mux_event_session_offline_serialization() {
    let event = MuxEvent::SessionOffline { session: "s1".to_owned() };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "session_offline");
    assert_eq!(json["session"], "s1");
}

// -- MuxEvent deserialization (round-trip) -------------------------------------

#[test]
fn mux_event_round_trip() {
    let events = vec![
        MuxEvent::State {
            session: "s1".to_owned(),
            prev: "idle".to_owned(),
            next: "working".to_owned(),
            seq: 1,
        },
        MuxEvent::Screen {
            session: "s2".to_owned(),
            lines: vec!["line1".to_owned()],
            cols: 80,
            rows: 24,
        },
        MuxEvent::Credential {
            session: "s3".to_owned(),
            account: "acc".to_owned(),
            status: "healthy".to_owned(),
            error: None,
        },
        MuxEvent::SessionOnline { session: "s4".to_owned(), url: "http://x".to_owned() },
        MuxEvent::SessionOffline { session: "s5".to_owned() },
    ];

    for event in &events {
        let json = serde_json::to_string(event).expect("serialize");
        let back: MuxEvent = serde_json::from_str(&json).expect("deserialize");
        let json2 = serde_json::to_string(&back).expect("re-serialize");
        assert_eq!(json, json2);
    }
}
