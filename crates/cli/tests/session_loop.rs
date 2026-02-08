// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for the session loop + HTTP transport, exercising
//! the full stack in-process via `axum_test::TestServer`.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use axum::http::StatusCode;
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use coop::driver::AgentState;
use coop::event::InputEvent;
use coop::pty::spawn::NativePty;
use coop::ring::RingBuffer;
use coop::screen::Screen;
use coop::session::{Session, SessionConfig};
use coop::transport::http::{HealthResponse, InputRequest, ScreenResponse, StatusResponse};
use coop::transport::state::WriteLock;
use coop::transport::{build_router, AppState};

fn make_app_state(input_tx: mpsc::Sender<InputEvent>) -> Arc<AppState> {
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(65536))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(0)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        bytes_written: AtomicU64::new(0),
        auth_token: None,
        nudge_encoder: None,
        respond_encoder: None,
        shutdown: CancellationToken::new(),
    })
}

// ---------------------------------------------------------------------------
// Session loop tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_echo_captures_output_and_exits_zero() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["echo".into(), "integration".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state: Arc::clone(&app_state),
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    let status = session.run().await?;
    assert_eq!(status.code, Some(0));

    // Ring should contain output
    let ring = app_state.ring.read().await;
    assert!(ring.total_written() > 0);
    let (a, b) = ring.read_from(0).ok_or(anyhow::anyhow!("no ring data"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);
    let text = String::from_utf8_lossy(&data);
    assert!(text.contains("integration"), "ring: {text:?}");

    // Screen should contain output
    let screen = app_state.screen.read().await;
    let snap = screen.snapshot();
    let lines = snap.lines.join("\n");
    assert!(lines.contains("integration"), "screen: {lines:?}");

    Ok(())
}

#[tokio::test]
async fn session_input_roundtrip() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx.clone());
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["/bin/cat".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state: Arc::clone(&app_state),
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    let session_handle = tokio::spawn(async move { session.run().await });

    // Send input via the channel (simulating transport layer)
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"roundtrip\n")))
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send Ctrl-D to close cat
    input_tx
        .send(InputEvent::Write(Bytes::from_static(b"\x04")))
        .await?;
    drop(input_tx);

    let status = session_handle.await??;
    assert_eq!(status.code, Some(0));

    // Verify output captured in ring
    let ring = app_state.ring.read().await;
    let (a, b) = ring.read_from(0).ok_or(anyhow::anyhow!("no ring data"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);
    let text = String::from_utf8_lossy(&data);
    assert!(text.contains("roundtrip"), "ring: {text:?}");

    Ok(())
}

#[tokio::test]
async fn session_shutdown_terminates_child() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["/bin/sh".into(), "-c".into(), "sleep 60".into()], 80, 24)?;
    let sd = shutdown.clone();
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state,
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown: sd,
    });

    // Cancel after a short delay
    let cancel = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let status = session.run().await?;
    assert!(
        status.code.is_some() || status.signal.is_some(),
        "expected exit: {status:?}"
    );
    Ok(())
}

#[tokio::test]
async fn session_exited_state_broadcast() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["true".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state: Arc::clone(&app_state),
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    let _ = session.run().await?;

    // After run(), agent_state should be Exited
    let agent = app_state.agent_state.read().await;
    match &*agent {
        AgentState::Exited { status } => {
            assert_eq!(status.code, Some(0));
        }
        other => {
            anyhow::bail!("expected Exited state, got {:?}", other.as_str());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP transport tests (via axum_test::TestServer)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_health_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/health").await;
    resp.assert_status(StatusCode::OK);
    let health: HealthResponse = resp.json();
    assert_eq!(health.status, "running");
    assert_eq!(health.agent_type, "unknown");
    assert_eq!(health.terminal.cols, 80);
    assert_eq!(health.terminal.rows, 24);
    Ok(())
}

#[tokio::test]
async fn http_status_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/status").await;
    resp.assert_status(StatusCode::OK);
    let status: StatusResponse = resp.json();
    assert_eq!(status.state, "starting");
    assert_eq!(status.ws_clients, 0);
    Ok(())
}

#[tokio::test]
async fn http_screen_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::OK);
    let screen: ScreenResponse = resp.json();
    assert_eq!(screen.cols, 80);
    assert_eq!(screen.rows, 24);
    assert!(!screen.alt_screen);
    Ok(())
}

#[tokio::test]
async fn http_screen_text_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/screen/text").await;
    resp.assert_status(StatusCode::OK);
    let ct_header = resp.header("content-type");
    let ct = ct_header.to_str().unwrap_or("");
    assert_eq!(ct, "text/plain; charset=utf-8");
    Ok(())
}

#[tokio::test]
async fn http_input_endpoint() -> anyhow::Result<()> {
    let (input_tx, mut consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/input")
        .json(&InputRequest {
            text: "hello".to_owned(),
            enter: true,
        })
        .await;

    resp.assert_status(StatusCode::OK);

    // Verify the input was received on the channel
    let event = consumer_input_rx.recv().await;
    match event {
        Some(InputEvent::Write(data)) => {
            assert_eq!(&data[..], b"hello\r");
        }
        other => {
            anyhow::bail!("expected Write event, got: {other:?}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn http_nudge_returns_no_driver_for_unknown() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/agent/nudge")
        .json(&serde_json::json!({"message": "do something"}))
        .await;

    // No nudge encoder configured → NO_DRIVER error
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn http_auth_rejects_bad_token() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    let app_state = Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(65536))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(0)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        bytes_written: AtomicU64::new(0),
        auth_token: Some("secret-token".to_owned()),
        nudge_encoder: None,
        respond_encoder: None,
        shutdown: CancellationToken::new(),
    });

    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    // Health endpoint skips auth
    let resp = server.get("/api/v1/health").await;
    resp.assert_status(StatusCode::OK);

    // No token on protected route → 401
    let resp = server.get("/api/v1/status").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // Wrong token → 401
    let resp = server
        .get("/api/v1/status")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer wrong-token"),
        )
        .await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    // Correct token → 200
    let resp = server
        .get("/api/v1/status")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer secret-token"),
        )
        .await;
    resp.assert_status(StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn http_agent_state_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/agent/state").await;
    // No driver configured → NO_DRIVER error (404)
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

// ---------------------------------------------------------------------------
// Full stack: session + HTTP transport
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_stack_echo_screen_via_http() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["echo".into(), "fullstack".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state: Arc::clone(&app_state),
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    // Run session to completion
    let _ = session.run().await?;

    // Now query the HTTP layer
    let router = build_router(Arc::clone(&app_state));
    let server = axum_test::TestServer::new(router).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::OK);
    let screen: ScreenResponse = resp.json();
    let lines = screen.lines.join("\n");
    assert!(lines.contains("fullstack"), "screen: {lines:?}");

    // Verify status shows exited
    let resp2 = server.get("/api/v1/status").await;
    resp2.assert_status(StatusCode::OK);
    let status: StatusResponse = resp2.json();
    assert_eq!(status.state, "exited");
    assert_eq!(status.exit_code, Some(0));

    Ok(())
}
