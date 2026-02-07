// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! Integration tests for the session loop + HTTP transport, exercising
//! the full stack in-process via `tower::ServiceExt`.

use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use coop::driver::AgentState;
use coop::event::InputEvent;
use coop::pty::spawn::NativePty;
use coop::ring::RingBuffer;
use coop::screen::Screen;
use coop::session::{Session, SessionConfig};
use coop::transport::http::{
    build_router, HealthResponse, InputRequest, ScreenResponse, StatusResponse,
};
use coop::transport::AppState;

fn make_app_state(input_tx: mpsc::Sender<InputEvent>) -> Arc<AppState> {
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    Arc::new(AppState {
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(65536))),
        input_tx,
        output_tx,
        state_tx,
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        agent_type: "unknown".to_owned(),
        pid: Arc::new(RwLock::new(None)),
        start_time: Instant::now(),
        nudge_encoder: None,
        respond_encoder: None,
        ws_clients: AtomicI32::new(0),
        bytes_written: AtomicU64::new(0),
        shutdown: CancellationToken::new(),
        auth_token: None,
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
// HTTP transport tests (in-process via tower::ServiceExt)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_health_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    let health: HealthResponse = serde_json::from_slice(&body)?;
    assert_eq!(health.status, "ok");
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

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    let status: StatusResponse = serde_json::from_slice(&body)?;
    assert_eq!(status.state, "starting");
    assert_eq!(status.ws_clients, 0);
    Ok(())
}

#[tokio::test]
async fn http_screen_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/screen")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    let screen: ScreenResponse = serde_json::from_slice(&body)?;
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

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/screen/text")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""));
    assert_eq!(ct, Some("text/plain; charset=utf-8"));
    Ok(())
}

#[tokio::test]
async fn http_input_endpoint() -> anyhow::Result<()> {
    let (input_tx, mut consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let req_body = serde_json::to_string(&InputRequest {
        text: "hello".to_owned(),
        enter: true,
    })?;

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/input")
                .header("content-type", "application/json")
                .body(Body::from(req_body))?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);

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
async fn http_resize_rejects_zero() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let req_body = serde_json::to_string(&serde_json::json!({"cols": 0, "rows": 24}))?;

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/resize")
                .header("content-type", "application/json")
                .body(Body::from(req_body))?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn http_nudge_returns_no_driver_for_unknown() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let req_body = serde_json::to_string(&serde_json::json!({"message": "do something"}))?;

    let resp = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agent/nudge")
                .header("content-type", "application/json")
                .body(Body::from(req_body))?,
        )
        .await?;

    // No nudge encoder configured → NO_DRIVER error
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn http_auth_rejects_bad_token() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    let app_state = Arc::new(AppState {
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(65536))),
        input_tx,
        output_tx,
        state_tx,
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        agent_type: "unknown".to_owned(),
        pid: Arc::new(RwLock::new(None)),
        start_time: Instant::now(),
        nudge_encoder: None,
        respond_encoder: None,
        ws_clients: AtomicI32::new(0),
        bytes_written: AtomicU64::new(0),
        shutdown: CancellationToken::new(),
        auth_token: Some("secret-token".to_owned()),
    });

    let router = build_router(app_state);

    // No token → 401
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Wrong token → 401
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header("authorization", "Bearer wrong-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Correct token → 200
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/health")
                .header("authorization", "Bearer secret-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);

    Ok(())
}

#[tokio::test]
async fn http_agent_state_endpoint() -> anyhow::Result<()> {
    let (input_tx, _consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let router = build_router(app_state);

    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/agent/state")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    let state: serde_json::Value = serde_json::from_slice(&body)?;
    assert_eq!(state["agent_type"], "unknown");
    assert_eq!(state["state"], "starting");
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
    let resp = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/screen")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await?;
    let screen: ScreenResponse = serde_json::from_slice(&body)?;
    let lines = screen.lines.join("\n");
    assert!(lines.contains("fullstack"), "screen: {lines:?}");

    // Verify status shows exited
    let router2 = build_router(app_state);
    let resp2 = router2
        .oneshot(
            Request::builder()
                .uri("/api/v1/status")
                .body(Body::empty())?,
        )
        .await?;

    assert_eq!(resp2.status(), StatusCode::OK);
    let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX).await?;
    let status: StatusResponse = serde_json::from_slice(&body2)?;
    assert_eq!(status.state, "exited");
    assert_eq!(status.exit_code, Some(0));

    Ok(())
}
