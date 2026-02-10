// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

/// Guard for tests that mutate environment variables. Prevents parallel races.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ===== Entry-point tests ====================================================

#[tokio::test]
async fn missing_coop_url_returns_2() {
    let _lock = ENV_LOCK.lock();
    std::env::remove_var("COOP_URL");
    std::env::remove_var("COOP_SOCKET");
    std::env::remove_var("COOP_AUTH_TOKEN");
    assert_eq!(run(&[]).await, 2);
}

#[tokio::test]
async fn help_flag_returns_0() {
    assert_eq!(run(&["--help".to_string()]).await, 0);
}

#[tokio::test]
async fn help_short_flag_returns_0() {
    assert_eq!(run(&["-h".to_string()]).await, 0);
}

#[tokio::test]
async fn connection_refused_returns_1() {
    assert_eq!(run(&["http://127.0.0.1:1".to_string()]).await, 1);
}

// ===== AttachArgs / StatuslineConfig tests ==================================

use clap::Parser;

fn parse_args(args: &[&str]) -> AttachArgs {
    let argv: Vec<&str> = std::iter::once("coop-attach").chain(args.iter().copied()).collect();
    AttachArgs::try_parse_from(argv).unwrap_or_else(|e| panic!("parse failed: {e}"))
}

#[test]
fn args_defaults_enabled_builtin() {
    let _lock = ENV_LOCK.lock();
    std::env::remove_var("COOP_STATUSLINE_CMD");
    std::env::remove_var("COOP_STATUSLINE_INTERVAL");
    std::env::remove_var("COOP_URL");
    std::env::remove_var("COOP_AUTH_TOKEN");
    std::env::remove_var("COOP_SOCKET");
    let args = parse_args(&[]);
    let cfg = StatuslineConfig::from(&args);
    assert!(cfg.enabled);
    assert!(cfg.cmd.is_none());
    assert_eq!(cfg.interval, Duration::from_secs(DEFAULT_STATUSLINE_INTERVAL));
}

#[test]
fn args_no_statusline_flag() {
    let args = parse_args(&["--no-statusline"]);
    let cfg = StatuslineConfig::from(&args);
    assert!(!cfg.enabled);
}

#[test]
fn args_statusline_cmd_space_separated() {
    let args = parse_args(&["--statusline-cmd", "echo hello"]);
    let cfg = StatuslineConfig::from(&args);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn args_statusline_cmd_equals_syntax() {
    let args = parse_args(&["--statusline-cmd=echo hello"]);
    let cfg = StatuslineConfig::from(&args);
    assert_eq!(cfg.cmd.as_deref(), Some("echo hello"));
}

#[test]
fn args_statusline_interval_override() {
    let args = parse_args(&["--statusline-interval", "10"]);
    let cfg = StatuslineConfig::from(&args);
    assert_eq!(cfg.interval, Duration::from_secs(10));
}

#[test]
fn args_statusline_interval_equals_syntax() {
    let args = parse_args(&["--statusline-interval=3"]);
    let cfg = StatuslineConfig::from(&args);
    assert_eq!(cfg.interval, Duration::from_secs(3));
}

#[test]
fn args_invalid_interval_is_parse_error() {
    let argv = ["coop-attach", "--statusline-interval=abc"];
    assert!(AttachArgs::try_parse_from(argv).is_err());
}

#[test]
fn args_url_positional() {
    let args = parse_args(&["http://localhost:8080"]);
    assert_eq!(args.url.as_deref(), Some("http://localhost:8080"));
}

#[test]
fn args_socket_flag() {
    let args = parse_args(&["--socket", "/tmp/coop.sock"]);
    assert_eq!(args.socket.as_deref(), Some("/tmp/coop.sock"));
}

#[test]
fn args_auth_token_flag() {
    let args = parse_args(&["--auth-token", "secret"]);
    assert_eq!(args.auth_token.as_deref(), Some("secret"));
}

#[test]
fn args_max_reconnects_default() {
    let args = parse_args(&[]);
    assert_eq!(args.max_reconnects, 10);
}

#[test]
fn args_max_reconnects_override() {
    let args = parse_args(&["--max-reconnects", "0"]);
    assert_eq!(args.max_reconnects, 0);
}

// ===== builtin_statusline tests =============================================

#[test]
fn builtin_statusline_format() {
    let state = AttachState {
        agent_state: "working".to_owned(),
        cols: 120,
        rows: 40,
        started: Instant::now(),
        next_offset: 0,
    };
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
        next_offset: 0,
    };
    let line = builtin_statusline(&state);
    assert!(line.contains("42s") || line.contains("43s"), "expected ~42s uptime: {line}");
}

// ===== run_statusline_cmd tests =============================================

#[tokio::test]
async fn run_statusline_cmd_captures_output() {
    let state = AttachState::new(crate::DEFAULT_TERM_COLS, crate::DEFAULT_TERM_ROWS);
    let result = run_statusline_cmd("echo test-output", &state).await;
    assert_eq!(result, "test-output");
}

#[tokio::test]
async fn run_statusline_cmd_expands_state() {
    let mut state = AttachState::new(crate::DEFAULT_TERM_COLS, crate::DEFAULT_TERM_ROWS);
    state.agent_state = "idle".to_owned();
    let result = run_statusline_cmd("echo {state}", &state).await;
    assert_eq!(result, "idle");
}

#[tokio::test]
async fn run_statusline_cmd_expands_dimensions() {
    let state = AttachState::new(120, 40);
    let result = run_statusline_cmd("echo {cols}x{rows}", &state).await;
    assert_eq!(result, "120x40");
}

#[tokio::test]
async fn run_statusline_cmd_expands_uptime() {
    let state = AttachState {
        agent_state: "working".to_owned(),
        cols: 80,
        rows: 24,
        started: Instant::now() - Duration::from_secs(99),
        next_offset: 0,
    };
    let result = run_statusline_cmd("echo {uptime}", &state).await;
    assert!(result == "99" || result == "100", "expected ~99: {result}");
}

#[tokio::test]
async fn run_statusline_cmd_failed_command() {
    let state = AttachState::new(crate::DEFAULT_TERM_COLS, crate::DEFAULT_TERM_ROWS);
    let result = run_statusline_cmd("false", &state).await;
    assert!(result.contains("failed"));
}

#[tokio::test]
async fn run_statusline_cmd_trims_trailing_newline() {
    let state = AttachState::new(crate::DEFAULT_TERM_COLS, crate::DEFAULT_TERM_ROWS);
    let result = run_statusline_cmd("printf 'hello\\n\\n'", &state).await;
    assert_eq!(result, "hello");
}

// ===== WebSocket integration tests ==========================================
// These tests spin up a real coop server with MockPty and connect via
// tokio-tungstenite, exercising the same protocol that `attach` uses.

mod ws_integration {
    use base64::Engine;
    use bytes::Bytes;
    use futures_util::{SinkExt, StreamExt};

    use crate::event::OutputEvent;
    use crate::test_support::AppStateBuilder;
    use crate::transport::ws::{ClientMessage, ServerMessage};

    use super::*;

    /// Helper: spawn a coop HTTP server with a MockPty backend and return
    /// the server address. The server emits `output_chunks` on the broadcast
    /// channel.
    async fn spawn_test_server(
        output_chunks: Vec<&str>,
    ) -> (std::net::SocketAddr, std::sync::Arc<crate::transport::state::AppState>) {
        let (state, _input_rx) =
            AppStateBuilder::new().ring_size(crate::test_support::TEST_RING_SIZE).build();

        // Write output chunks to ring buffer and broadcast them.
        {
            let mut ring = state.terminal.ring.write().await;
            for chunk in &output_chunks {
                let data = Bytes::from(chunk.as_bytes().to_vec());
                ring.write(&data);
                let _ = state.channels.output_tx.send(OutputEvent::Raw(data));
            }
        }

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
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
    async fn send_and_recv<S, R>(tx: &mut S, rx: &mut R, msg: &ClientMessage) -> String
    where
        S: SinkExt<tokio_tungstenite::tungstenite::Message> + Unpin,
        R: StreamExt<
                Item = Result<
                    tokio_tungstenite::tungstenite::Message,
                    tokio_tungstenite::tungstenite::Error,
                >,
            > + Unpin,
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

        let msg = ClientMessage::Replay { offset: 0, limit: None };
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let parsed: Result<ServerMessage, _> = serde_json::from_str(&response);
        match parsed {
            Ok(ServerMessage::ReplayResult { data, offset, .. }) => {
                assert_eq!(offset, 0);
                let decoded =
                    base64::engine::general_purpose::STANDARD.decode(&data).unwrap_or_default();
                let text = String::from_utf8_lossy(&decoded);
                assert!(
                    text.contains("hello world"),
                    "expected 'hello world' in replay, got: {text}"
                );
            }
            other => panic!("expected ReplayResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn state_request_returns_current_state() {
        let (addr, _state) = spawn_test_server(vec![]).await;
        let (mut tx, mut rx) = connect_ws(addr, "all").await;

        let msg = ClientMessage::StateRequest {};
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let parsed: Result<ServerMessage, _> = serde_json::from_str(&response);
        match parsed {
            Ok(ServerMessage::AgentState { state, .. }) => {
                assert_eq!(state, "starting", "default AppState starts as 'starting'");
            }
            other => panic!("expected AgentState, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn input_raw_reaches_server() {
        let (input_tx, mut input_rx) = tokio::sync::mpsc::channel(64);
        let state = AppStateBuilder::new().ring_size(4096).build_with_sender(input_tx);

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
            .await
            .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        // Send an InputRaw message.
        let data = base64::engine::general_purpose::STANDARD.encode(b"ls\n");
        let msg = ClientMessage::InputRaw { data };
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
        let state = AppStateBuilder::new().ring_size(4096).build_with_sender(input_tx);

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
            .await
            .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        let msg = ClientMessage::Resize { cols: 120, rows: 39 };
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
        let (state, _input_rx) =
            AppStateBuilder::new().ring_size(4096).auth_token("secret123").build();

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
            .await
            .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, mut rx) = connect_ws(addr, "raw").await;

        // Try to send input without authenticating.
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let msg = ClientMessage::InputRaw { data };
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let parsed: Result<ServerMessage, _> = serde_json::from_str(&response);
        match parsed {
            Ok(ServerMessage::Error { code, .. }) => {
                assert_eq!(code, "UNAUTHORIZED");
            }
            other => panic!("expected UNAUTHORIZED error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auth_required_blocks_resize() {
        let (state, _input_rx) =
            AppStateBuilder::new().ring_size(4096).auth_token("secret123").build();

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
            .await
            .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, mut rx) = connect_ws(addr, "raw").await;

        // Try to resize without authenticating.
        let msg = ClientMessage::Resize { cols: 120, rows: 40 };
        let response = send_and_recv(&mut tx, &mut rx, &msg).await;

        let parsed: Result<ServerMessage, _> = serde_json::from_str(&response);
        match parsed {
            Ok(ServerMessage::Error { code, .. }) => {
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

        let (addr, _handle) = crate::test_support::spawn_http_server(std::sync::Arc::clone(&state))
            .await
            .unwrap_or_else(|e| panic!("server: {e}"));
        tokio::time::sleep(Duration::from_millis(50)).await;

        let (mut tx, _rx) = connect_ws(addr, "raw").await;

        // Authenticate first.
        let auth = ClientMessage::Auth { token: "secret123".to_owned() };
        let json = serde_json::to_string(&auth).unwrap_or_default();
        let _ = tx.send(tokio_tungstenite::tungstenite::Message::Text(json)).await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Now send input.
        let data = base64::engine::general_purpose::STANDARD.encode(b"hello");
        let msg = ClientMessage::InputRaw { data };
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
}
