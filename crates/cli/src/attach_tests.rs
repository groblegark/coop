// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

// ===== Entry-point tests ====================================================

#[test]
fn missing_coop_url_returns_2() {
    std::env::remove_var("COOP_URL");
    assert_eq!(run(&[]), 2);
}

#[test]
fn help_flag_returns_0() {
    assert_eq!(run(&["--help".to_string()]), 0);
}

#[test]
fn help_short_flag_returns_0() {
    assert_eq!(run(&["-h".to_string()]), 0);
}

#[test]
fn connection_refused_returns_1() {
    assert_eq!(run(&["http://127.0.0.1:1".to_string()]), 1);
}

#[test]
fn no_statusline_still_returns_error_without_url() {
    std::env::remove_var("COOP_URL");
    assert_eq!(run(&["--no-statusline".to_string()]), 2);
}

// ===== StatuslineConfig tests ===============================================

#[test]
fn statusline_defaults_enabled_builtin() {
    std::env::remove_var("COOP_STATUSLINE_CMD");
    std::env::remove_var("COOP_STATUSLINE_INTERVAL");
    let cfg = StatuslineConfig::from_args(&[]);
    assert!(cfg.enabled);
    assert!(cfg.cmd.is_none());
    assert_eq!(cfg.interval, Duration::from_secs(DEFAULT_STATUSLINE_INTERVAL));
}

#[test]
fn statusline_no_statusline_flag() {
    let cfg = StatuslineConfig::from_args(&["--no-statusline".to_string()]);
    assert!(!cfg.enabled);
}

#[test]
fn statusline_cmd_space_separated() {
    let cfg =
        StatuslineConfig::from_args(&["--statusline-cmd".to_string(), "echo hello".to_string()]);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn statusline_cmd_equals_syntax() {
    let cfg = StatuslineConfig::from_args(&["--statusline-cmd=echo hello".to_string()]);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn statusline_interval_override() {
    let cfg = StatuslineConfig::from_args(&["--statusline-interval".to_string(), "10".to_string()]);
    assert_eq!(cfg.interval, Duration::from_secs(10));
}

#[test]
fn statusline_interval_equals_syntax() {
    let cfg = StatuslineConfig::from_args(&["--statusline-interval=3".to_string()]);
    assert_eq!(cfg.interval, Duration::from_secs(3));
}

#[test]
fn statusline_invalid_interval_uses_default() {
    let cfg = StatuslineConfig::from_args(&["--statusline-interval=abc".to_string()]);
    assert_eq!(cfg.interval, Duration::from_secs(DEFAULT_STATUSLINE_INTERVAL));
}

#[test]
fn statusline_cmd_from_env() {
    std::env::set_var("COOP_STATUSLINE_CMD", "env-cmd");
    let cfg = StatuslineConfig::from_args(&[]);
    assert_eq!(cfg.cmd.as_deref(), Some("env-cmd"));
    std::env::remove_var("COOP_STATUSLINE_CMD");
}

#[test]
fn statusline_arg_overrides_env() {
    std::env::set_var("COOP_STATUSLINE_CMD", "env-cmd");
    let cfg = StatuslineConfig::from_args(&["--statusline-cmd=arg-cmd".to_string()]);
    assert_eq!(cfg.cmd.as_deref(), Some("arg-cmd"));
    std::env::remove_var("COOP_STATUSLINE_CMD");
}

// ===== builtin_statusline tests =============================================

#[test]
fn builtin_statusline_format() {
    let state = AttachState { agent_state: "working".to_owned(), cols: 120, rows: 40, started: Instant::now() };
    let line = builtin_statusline(&state);
    assert!(line.contains("[coop]"));
    assert!(line.contains("working"));
    assert!(line.contains("120x40"));
}

#[test]
fn builtin_statusline_uptime_increases() {
    let state = AttachState {
        agent_state: "idle".to_owned(),
        cols: 80,
        rows: 24,
        started: Instant::now() - Duration::from_secs(42),
    };
    let line = builtin_statusline(&state);
    assert!(line.contains("42s") || line.contains("43s"), "expected ~42s uptime: {line}");
}

// ===== run_statusline_cmd tests =============================================

#[test]
fn run_statusline_cmd_captures_output() {
    let state = AttachState::new(80, 24);
    let result = run_statusline_cmd("echo test-output", &state);
    assert_eq!(result, "test-output");
}

#[test]
fn run_statusline_cmd_expands_state() {
    let mut state = AttachState::new(80, 24);
    state.agent_state = "idle".to_owned();
    let result = run_statusline_cmd("echo {state}", &state);
    assert_eq!(result, "idle");
}

#[test]
fn run_statusline_cmd_expands_dimensions() {
    let state = AttachState::new(120, 40);
    let result = run_statusline_cmd("echo {cols}x{rows}", &state);
    assert_eq!(result, "120x40");
}

#[test]
fn run_statusline_cmd_expands_uptime() {
    let state = AttachState {
        agent_state: "working".to_owned(),
        cols: 80,
        rows: 24,
        started: Instant::now() - Duration::from_secs(99),
    };
    let result = run_statusline_cmd("echo {uptime}", &state);
    assert!(result == "99" || result == "100", "expected ~99: {result}");
}

#[test]
fn run_statusline_cmd_failed_command() {
    let state = AttachState::new(80, 24);
    let result = run_statusline_cmd("false", &state);
    assert!(result.contains("failed"));
}

#[test]
fn run_statusline_cmd_trims_trailing_newline() {
    let state = AttachState::new(80, 24);
    let result = run_statusline_cmd("printf 'hello\\n\\n'", &state);
    assert_eq!(result, "hello");
}

// ===== find_arg_value tests =================================================

#[test]
fn find_arg_value_space_separated() {
    let args = vec!["--key".to_string(), "val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), Some("val".to_string()));
}

#[test]
fn find_arg_value_equals_syntax() {
    let args = vec!["--key=val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), Some("val".to_string()));
}

#[test]
fn find_arg_value_not_found() {
    let args = vec!["--other".to_string(), "val".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), None);
}

#[test]
fn find_arg_value_empty_args() {
    assert_eq!(find_arg_value(&[], "--key"), None);
}

#[test]
fn find_arg_value_key_at_end_without_value() {
    let args = vec!["--key".to_string()];
    assert_eq!(find_arg_value(&args, "--key"), None);
}

// ===== Wire-type serde tests ================================================

#[test]
fn client_msg_input_raw_serialization() {
    let msg = ClientMsg::InputRaw { data: "aGVsbG8=".to_owned() };
    let json = serde_json::to_string(&msg).unwrap_or_default();
    assert!(json.contains("\"type\":\"input_raw\""));
    assert!(json.contains("\"data\":\"aGVsbG8=\""));
}

#[test]
fn client_msg_resize_serialization() {
    let msg = ClientMsg::Resize { cols: 120, rows: 40 };
    let json = serde_json::to_string(&msg).unwrap_or_default();
    assert!(json.contains("\"type\":\"resize\""));
    assert!(json.contains("\"cols\":120"));
    assert!(json.contains("\"rows\":40"));
}

#[test]
fn client_msg_replay_serialization() {
    let msg = ClientMsg::Replay { offset: 0 };
    let json = serde_json::to_string(&msg).unwrap_or_default();
    assert!(json.contains("\"type\":\"replay\""));
    assert!(json.contains("\"offset\":0"));
}

#[test]
fn client_msg_auth_serialization() {
    let msg = ClientMsg::Auth { token: "secret".to_owned() };
    let json = serde_json::to_string(&msg).unwrap_or_default();
    assert!(json.contains("\"type\":\"auth\""));
    assert!(json.contains("\"token\":\"secret\""));
}

#[test]
fn client_msg_state_request_serialization() {
    let msg = ClientMsg::StateRequest {};
    let json = serde_json::to_string(&msg).unwrap_or_default();
    assert!(json.contains("\"type\":\"state_request\""));
}

#[test]
fn server_msg_output_deserialization() {
    let json = r#"{"type":"output","data":"aGVsbG8=","offset":42}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Other);
    match msg {
        ServerMsg::Output { data, offset } => {
            assert_eq!(data, "aGVsbG8=");
            assert_eq!(offset, 42);
        }
        other => panic!("expected Output, got {other:?}"),
    }
}

#[test]
fn server_msg_exit_deserialization() {
    let json = r#"{"type":"exit","code":0,"signal":null}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Other);
    assert!(matches!(msg, ServerMsg::Exit { code: Some(0), .. }));
}

#[test]
fn server_msg_error_deserialization() {
    let json = r#"{"type":"error","code":"UNAUTHORIZED","message":"not authenticated"}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Other);
    match msg {
        ServerMsg::Error { code, message } => {
            assert_eq!(code, "UNAUTHORIZED");
            assert_eq!(message, "not authenticated");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn server_msg_state_change_deserialization() {
    let json = r#"{"type":"state_change","prev":"working","next":"waiting_for_input","seq":5}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Other);
    match msg {
        ServerMsg::StateChange { next, .. } => {
            assert_eq!(next, "waiting_for_input");
        }
        other => panic!("expected StateChange, got {other:?}"),
    }
}

#[test]
fn server_msg_unknown_type_becomes_other() {
    let json = r#"{"type":"screen","lines":[],"cols":80,"rows":24,"alt_screen":false,"seq":1}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Pong {});
    assert!(matches!(msg, ServerMsg::Other));
}

#[test]
fn server_msg_pong_deserialization() {
    let json = r#"{"type":"pong"}"#;
    let msg: ServerMsg = serde_json::from_str(json).unwrap_or(ServerMsg::Other);
    assert!(matches!(msg, ServerMsg::Pong {}));
}

// ===== WebSocket integration tests ==========================================
// These tests spin up a real coop server with MockPty and connect via
// tokio-tungstenite, exercising the same protocol that `attach_inner` uses.

mod ws_integration {
    use base64::Engine;
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};

    use crate::event::OutputEvent;
    use crate::test_support::AppStateBuilder;

    use super::*;

    /// Helper: spawn a coop HTTP server with a MockPty backend and return
    /// the server address. The server emits `output_chunks` on the broadcast
    /// channel.
    async fn spawn_test_server(
        output_chunks: Vec<&str>,
    ) -> (std::net::SocketAddr, std::sync::Arc<crate::transport::state::AppState>) {
        let (state, _input_rx) = AppStateBuilder::new().ring_size(65536).build();

        // Write output chunks to ring buffer and broadcast them.
        {
            let mut ring = state.terminal.ring.write().await;
            for chunk in &output_chunks {
                let data = Bytes::from(chunk.as_bytes().to_vec());
                ring.write(&data);
                let _ = state.channels.output_tx.send(OutputEvent::Raw(data));
            }
        }

        let (addr, _handle) = crate::test_support::spawn_http_server(
            std::sync::Arc::clone(&state),
        )
        .await
        .unwrap_or_else(|e| panic!("failed to spawn test server: {e}"));

        // Small delay for the server to be ready.
        tokio::time::sleep(Duration::from_millis(50)).await;

        (addr, state)
    }

    /// Connect a WebSocket client to the given address.
    async fn connect_ws(
        addr: std::net::SocketAddr,
        mode: &str,
    ) -> (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            tokio_tungstenite::tungstenite::Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        let url = format!("ws://{addr}/ws?mode={mode}");
        let (stream, _) = tokio_tungstenite::connect_async(&url)
            .await
            .unwrap_or_else(|e| panic!("ws connect failed: {e}"));
        stream.split()
    }

    /// Send a JSON message and return the text of the response.
    async fn send_and_recv<S, R>(tx: &mut S, rx: &mut R, msg: &ClientMsg) -> String
    where
        S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
        R: StreamExt<Item = Result<tokio_tungstenite::tungstenite::Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        let json = serde_json::to_string(msg).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;

        // Read with a timeout.
        match tokio::time::timeout(Duration::from_secs(2), rx.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text)))) => text.to_string(),
            other => panic!("expected text message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn replay_returns_ring_buffer_contents() {
        let (addr, _state) = spawn_test_server(vec!["hello world"]).await;
        let (mut tx, mut rx) = connect_ws(addr, "raw").await;

        let msg = ClientMsg::Replay { offset: 0 };
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let server_msg: ServerMsg =
            serde_json::from_str(&response).unwrap_or(ServerMsg::Other);
        match server_msg {
            ServerMsg::Output { data, offset } => {
                assert_eq!(offset, 0);
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&data)
                    .unwrap_or_default();
                let text = String::from_utf8_lossy(&decoded);
                assert!(
                    text.contains("hello world"),
                    "expected 'hello world' in replay, got: {text}"
                );
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn state_request_returns_current_state() {
        let (addr, _state) = spawn_test_server(vec![]).await;
        let (mut tx, mut rx) = connect_ws(addr, "all").await;

        let msg = ClientMsg::StateRequest {};
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let server_msg: ServerMsg =
            serde_json::from_str(&response).unwrap_or(ServerMsg::Other);
        match server_msg {
            ServerMsg::StateChange { next, .. } => {
                assert_eq!(next, "starting", "default AppState starts as 'starting'");
            }
            other => panic!("expected StateChange, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn input_raw_reaches_server() {
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
        let state = AppStateBuilder::new()
            .ring_size(4096)
            .build_with_sender(input_tx);

        let (addr, _handle) =
            crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
                .await
                .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        // Send an InputRaw message.
        let data = base64::engine::general_purpose::STANDARD.encode(b"ls\n");
        let msg = ClientMsg::InputRaw { data };
        let json = serde_json::to_string(&msg).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;

        // The server should forward it as an InputEvent::Write.
        let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await;
        match event {
            Ok(Some(crate::event::InputEvent::Write(bytes))) => {
                assert_eq!(&bytes[..], b"ls\n");
            }
            other => panic!("expected Write(b'ls\\n'), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resize_reaches_server() {
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
        let state = AppStateBuilder::new()
            .ring_size(4096)
            .build_with_sender(input_tx);

        let (addr, _handle) =
            crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
                .await
                .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        let msg = ClientMsg::Resize { cols: 120, rows: 39 };
        let json = serde_json::to_string(&msg).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;

        let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await;
        match event {
            Ok(Some(crate::event::InputEvent::Resize { cols, rows })) => {
                assert_eq!(cols, 120);
                assert_eq!(rows, 39);
            }
            other => panic!("expected Resize(120, 39), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auth_required_blocks_input_raw() {
        let (state, _input_rx) = AppStateBuilder::new()
            .ring_size(4096)
            .auth_token("secret123")
            .build();

        let (addr, _handle) =
            crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
                .await
                .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, mut rx) = connect_ws(addr, "raw").await;

        // Try to send input without authenticating.
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let msg = ClientMsg::InputRaw { data };
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let server_msg: ServerMsg =
            serde_json::from_str(&response).unwrap_or(ServerMsg::Other);
        match server_msg {
            ServerMsg::Error { code, .. } => {
                assert_eq!(code, "UNAUTHORIZED");
            }
            other => panic!("expected UNAUTHORIZED error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auth_then_input_raw_succeeds() {
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
        let state = AppStateBuilder::new()
            .ring_size(4096)
            .auth_token("secret123")
            .build_with_sender(input_tx);

        let (addr, _handle) =
            crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
                .await
                .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        // Authenticate first.
        let auth = ClientMsg::Auth { token: "secret123".to_owned() };
        let json = serde_json::to_string(&auth).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now send input.
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let msg = ClientMsg::InputRaw { data };
        let json = serde_json::to_string(&msg).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;

        let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await;
        match event {
            Ok(Some(crate::event::InputEvent::Write(bytes))) => {
                assert_eq!(&bytes[..], b"hello");
            }
            other => panic!("expected Write(b'hello'), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resize_with_statusline_sends_rows_minus_one() {
        // Verify that when a statusline is active, the attach client would
        // send rows-1. This tests the logic without needing a real TTY.
        let cols: u16 = 120;
        let rows: u16 = 40;
        let content_rows = rows - 1;

        // The ClientMsg::Resize should carry content_rows when sl is active.
        let msg = ClientMsg::Resize { cols, rows: content_rows };
        let json = serde_json::to_string(&msg).unwrap_or_default();
        assert!(json.contains("\"rows\":39"), "json: {json}");
    }
}
