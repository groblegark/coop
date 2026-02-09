// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use axum::http::StatusCode;

use crate::driver::AgentState;
use crate::event::InputEvent;
use crate::test_support::{AnyhowExt, AppStateBuilder, StubNudgeEncoder};
use crate::transport::build_router;

fn test_state() -> (
    Arc<crate::transport::state::AppState>,
    tokio::sync::mpsc::Receiver<InputEvent>,
) {
    AppStateBuilder::new().child_pid(1234).build()
}

#[tokio::test]
async fn health_200() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/health").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"status\":\"running\""));
    assert!(body.contains("\"pid\":1234"));
    Ok(())
}

#[tokio::test]
async fn screen_snapshot() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"cols\":80"));
    assert!(body.contains("\"rows\":24"));
    Ok(())
}

#[tokio::test]
async fn screen_text_plain() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/screen/text").await;
    resp.assert_status(StatusCode::OK);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.contains("text/plain"));
    Ok(())
}

#[tokio::test]
async fn output_with_offset() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    {
        let mut ring = state.terminal.ring.write().await;
        ring.write(b"hello world");
    }
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/output?offset=0").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"total_written\":11"));
    Ok(())
}

#[tokio::test]
async fn input_sends_event() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/input")
        .json(&serde_json::json!({"text": "hello", "enter": true}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"bytes_written\":6"));

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(_))));
    Ok(())
}

#[tokio::test]
async fn keys_sends_event() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/input/keys")
        .json(&serde_json::json!({"keys": ["Escape", "Enter", "Ctrl-C"]}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"bytes_written\":3"));

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(_))));
    Ok(())
}

#[tokio::test]
async fn resize_sends_event() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/resize")
        .json(&serde_json::json!({"cols": 120, "rows": 40}))
        .await;
    resp.assert_status(StatusCode::OK);

    let event = rx.recv().await;
    assert!(matches!(
        event,
        Some(InputEvent::Resize {
            cols: 120,
            rows: 40
        })
    ));
    Ok(())
}

#[tokio::test]
async fn signal_delivers() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/signal")
        .json(&serde_json::json!({"signal": "SIGINT"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":true"));

    let event = rx.recv().await;
    assert!(matches!(
        event,
        Some(InputEvent::Signal(crate::event::PtySignal::Int))
    ));
    Ok(())
}

#[tokio::test]
async fn status_running() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/status").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"state\":\"running\""));
    Ok(())
}

#[tokio::test]
async fn agent_state_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn agent_nudge_not_ready_503() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    // ready defaults to false — nudge should be gated
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/agent/nudge")
        .json(&serde_json::json!({"message": "hello"}))
        .await;
    resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn agent_nudge_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/agent/nudge")
        .json(&serde_json::json!({"message": "hello"}))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn agent_respond_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/agent/respond")
        .json(&serde_json::json!({"accept": true}))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn auth_rejects_without_token() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .auth_token("secret")
        .build();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Health should be accessible without auth
    let resp = server.get("/api/v1/health").await;
    resp.assert_status(StatusCode::OK);

    // Screen should require auth
    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // With correct bearer token, should pass
    let resp = server
        .get("/api/v1/screen")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer secret"),
        )
        .await;
    resp.assert_status(StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn agent_state_includes_error_fields() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Error {
            detail: "rate_limit_error".to_owned(),
        })
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    // Populate error fields as session loop would
    *state.driver.error_detail.write().await = Some("rate_limit_error".to_owned());
    *state.driver.error_category.write().await = Some(crate::driver::ErrorCategory::RateLimited);

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(
        body.contains("\"error_detail\":\"rate_limit_error\""),
        "body: {body}"
    );
    assert!(
        body.contains("\"error_category\":\"rate_limited\""),
        "body: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn agent_state_omits_error_fields_when_not_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Working)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(
        !body.contains("error_detail"),
        "error_detail should be absent: {body}"
    );
    assert!(
        !body.contains("error_category"),
        "error_category should be absent: {body}"
    );
    Ok(())
}

#[tokio::test]
async fn agent_nudge_rejected_when_working() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Working)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/agent/nudge")
        .json(&serde_json::json!({"message": "hello"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":false"));
    assert!(body.contains("agent_busy"));
    Ok(())
}

#[tokio::test]
async fn agent_nudge_delivered_when_waiting() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::WaitingForInput)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/agent/nudge")
        .json(&serde_json::json!({"message": "hello"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":true"));
    Ok(())
}

#[tokio::test]
async fn resize_rejects_zero_cols() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/resize")
        .json(&serde_json::json!({"cols": 0, "rows": 24}))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn resize_rejects_zero_rows() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/resize")
        .json(&serde_json::json!({"cols": 80, "rows": 0}))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    Ok(())
}

// ---------------------------------------------------------------------------
// Stop hook endpoint tests
// ---------------------------------------------------------------------------

use crate::stop::{StopConfig, StopMode, StopState};

fn test_state_with_stop(
    config: StopConfig,
) -> (
    Arc<crate::transport::state::AppState>,
    tokio::sync::mpsc::Receiver<InputEvent>,
) {
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(16);
    let (output_tx, _) = tokio::sync::broadcast::channel::<crate::event::OutputEvent>(256);
    let (state_tx, _) = tokio::sync::broadcast::channel::<crate::event::StateChangeEvent>(64);

    let state = Arc::new(crate::transport::state::AppState {
        terminal: Arc::new(crate::transport::state::TerminalState {
            screen: tokio::sync::RwLock::new(crate::screen::Screen::new(80, 24)),
            ring: tokio::sync::RwLock::new(crate::ring::RingBuffer::new(4096)),
            ring_total_written: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            child_pid: std::sync::atomic::AtomicU32::new(1234),
            exit_status: tokio::sync::RwLock::new(None),
        }),
        driver: Arc::new(crate::transport::state::DriverState {
            agent_state: tokio::sync::RwLock::new(AgentState::Working),
            state_seq: std::sync::atomic::AtomicU64::new(0),
            detection_tier: std::sync::atomic::AtomicU8::new(u8::MAX),
            idle_grace_deadline: Arc::new(parking_lot::Mutex::new(None)),
            error_detail: tokio::sync::RwLock::new(None),
            error_category: tokio::sync::RwLock::new(None),
        }),
        channels: crate::transport::state::TransportChannels {
            input_tx,
            output_tx,
            state_tx,
        },
        config: crate::transport::state::SessionSettings {
            started_at: std::time::Instant::now(),
            agent: crate::driver::AgentType::Unknown,
            auth_token: None,
            nudge_encoder: None,
            respond_encoder: None,
            idle_grace_duration: std::time::Duration::from_secs(60),
        },
        lifecycle: crate::transport::state::LifecycleState {
            shutdown: tokio_util::sync::CancellationToken::new(),
            ws_client_count: std::sync::atomic::AtomicI32::new(0),
            bytes_written: std::sync::atomic::AtomicU64::new(0),
        },
        ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        nudge_mutex: Arc::new(tokio::sync::Mutex::new(())),
        stop: Arc::new(StopState::new(
            config,
            "http://127.0.0.1:0/api/v1/hooks/stop/resolve".to_owned(),
        )),
    });
    (state, input_rx)
}

#[tokio::test]
async fn hooks_stop_allow_mode_returns_empty() -> anyhow::Result<()> {
    let (state, _rx) = test_state_with_stop(StopConfig::default());
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    // Allow mode returns empty object (no decision field).
    assert!(body.get("decision").is_none());
    Ok(())
}

#[tokio::test]
async fn hooks_stop_signal_mode_blocks_without_signal() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: Some("Finish work first.".to_owned()),
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["decision"], "block");
    assert!(body["reason"]
        .as_str()
        .unwrap_or("")
        .contains("Finish work first."));
    Ok(())
}

#[tokio::test]
async fn hooks_stop_signal_mode_allows_after_signal() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let app = build_router(state.clone());
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Send a signal first.
    let resp = server
        .post("/api/v1/hooks/stop/resolve")
        .json(&serde_json::json!({"status": "done"}))
        .await;
    resp.assert_status(StatusCode::OK);

    // Now stop should be allowed.
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "should allow after signal");
    Ok(())
}

#[tokio::test]
async fn hooks_stop_safety_valve_always_allows() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // stop_hook_active = true => must allow
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": true}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "safety valve must allow");
    Ok(())
}

#[tokio::test]
async fn hooks_stop_unrecoverable_error_allows() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    // Set unrecoverable error state.
    *state.driver.error_category.write().await = Some(crate::driver::ErrorCategory::Unauthorized);
    *state.driver.error_detail.write().await = Some("invalid api key".to_owned());

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(
        body.get("decision").is_none(),
        "unrecoverable error should allow"
    );
    Ok(())
}

#[tokio::test]
async fn resolve_stop_stores_body() -> anyhow::Result<()> {
    let (state, _rx) = test_state_with_stop(StopConfig::default());
    let app = build_router(state.clone());
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop/resolve")
        .json(&serde_json::json!({"status": "complete", "notes": "all good"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"accepted\":true"));

    // Check that signal flag is set.
    assert!(state
        .stop
        .signaled
        .load(std::sync::atomic::Ordering::Acquire));
    // Check that signal body is stored.
    let stored = state.stop.signal_body.read().await;
    let stored_val = stored.as_ref().expect("signal body should be stored");
    assert_eq!(stored_val["status"], "complete");
    Ok(())
}

#[tokio::test]
async fn get_stop_config_returns_current() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: Some("test prompt".to_owned()),
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/config/stop").await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["mode"], "signal");
    assert_eq!(body["prompt"], "test prompt");
    Ok(())
}

#[tokio::test]
async fn put_stop_config_updates() -> anyhow::Result<()> {
    let (state, _rx) = test_state_with_stop(StopConfig::default());
    let app = build_router(state.clone());
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Default is allow mode.
    let resp = server.get("/api/v1/config/stop").await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["mode"], "allow");

    // Update to signal mode.
    let resp = server
        .put("/api/v1/config/stop")
        .json(&serde_json::json!({"mode": "signal", "prompt": "Wait for signal"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"updated\":true"));

    // Verify the update.
    let resp = server.get("/api/v1/config/stop").await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["mode"], "signal");
    assert_eq!(body["prompt"], "Wait for signal");
    Ok(())
}

#[tokio::test]
async fn hooks_stop_emits_stop_events() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let mut stop_rx = state.stop.stop_tx.subscribe();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // First call should block.
    server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;

    let event = stop_rx.try_recv()?;
    assert_eq!(event.stop_type.as_str(), "blocked");
    assert_eq!(event.seq, 0);
    Ok(())
}

#[tokio::test]
async fn signal_consumed_after_stop_check() -> anyhow::Result<()> {
    let config = StopConfig {
        mode: StopMode::Signal,
        prompt: None,
        schema: None,
    };
    let (state, _rx) = test_state_with_stop(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Signal, then check stop — should allow.
    server
        .post("/api/v1/hooks/stop/resolve")
        .json(&serde_json::json!({"ok": true}))
        .await;
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(
        body.get("decision").is_none(),
        "first check after signal should allow"
    );

    // Second stop check should block again (signal was consumed).
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(
        body["decision"], "block",
        "second check should block (signal consumed)"
    );
    Ok(())
}

#[tokio::test]
async fn auth_exempt_for_hooks_stop_and_resolve() -> anyhow::Result<()> {
    let config = StopConfig::default();
    // Build state with auth token.
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(16);
    let (output_tx, _) = tokio::sync::broadcast::channel::<crate::event::OutputEvent>(256);
    let (state_tx, _) = tokio::sync::broadcast::channel::<crate::event::StateChangeEvent>(64);
    let state = Arc::new(crate::transport::state::AppState {
        terminal: Arc::new(crate::transport::state::TerminalState {
            screen: tokio::sync::RwLock::new(crate::screen::Screen::new(80, 24)),
            ring: tokio::sync::RwLock::new(crate::ring::RingBuffer::new(4096)),
            ring_total_written: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            child_pid: std::sync::atomic::AtomicU32::new(1234),
            exit_status: tokio::sync::RwLock::new(None),
        }),
        driver: Arc::new(crate::transport::state::DriverState {
            agent_state: tokio::sync::RwLock::new(AgentState::Working),
            state_seq: std::sync::atomic::AtomicU64::new(0),
            detection_tier: std::sync::atomic::AtomicU8::new(u8::MAX),
            idle_grace_deadline: Arc::new(parking_lot::Mutex::new(None)),
            error_detail: tokio::sync::RwLock::new(None),
            error_category: tokio::sync::RwLock::new(None),
        }),
        channels: crate::transport::state::TransportChannels {
            input_tx,
            output_tx,
            state_tx,
        },
        config: crate::transport::state::SessionSettings {
            started_at: std::time::Instant::now(),
            agent: crate::driver::AgentType::Unknown,
            auth_token: Some("secret-token".to_owned()),
            nudge_encoder: None,
            respond_encoder: None,
            idle_grace_duration: std::time::Duration::from_secs(60),
        },
        lifecycle: crate::transport::state::LifecycleState {
            shutdown: tokio_util::sync::CancellationToken::new(),
            ws_client_count: std::sync::atomic::AtomicI32::new(0),
            bytes_written: std::sync::atomic::AtomicU64::new(0),
        },
        ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        nudge_mutex: Arc::new(tokio::sync::Mutex::new(())),
        stop: Arc::new(StopState::new(
            config,
            "http://127.0.0.1:0/api/v1/hooks/stop/resolve".to_owned(),
        )),
    });
    let _input_rx = input_rx;

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Hooks stop should work without auth.
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"stop_hook_active": false}))
        .await;
    resp.assert_status(StatusCode::OK);

    // Resolve stop should work without auth.
    let resp = server
        .post("/api/v1/hooks/stop/resolve")
        .json(&serde_json::json!({"ok": true}))
        .await;
    resp.assert_status(StatusCode::OK);

    // But other endpoints should still require auth.
    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    Ok(())
}
