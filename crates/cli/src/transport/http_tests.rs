// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::sync::atomic::{AtomicI32, AtomicU32};
use std::sync::Arc;
use std::time::Instant;

use axum::http::StatusCode;
use tokio::sync::{broadcast, mpsc, RwLock};

use crate::driver::AgentState;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::transport::build_router;
use crate::transport::state::{AppState, WriteLock};

fn test_state() -> (Arc<AppState>, mpsc::Receiver<InputEvent>) {
    let (input_tx, input_rx) = mpsc::channel(16);
    let (output_tx, _) = broadcast::channel::<OutputEvent>(16);
    let (state_tx, _) = broadcast::channel::<StateChangeEvent>(16);

    let state = Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(4096))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(1234)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        auth_token: None,
        nudge_encoder: None,
        respond_encoder: None,
    });

    (state, input_rx)
}

#[tokio::test]
async fn health_200() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
        let mut ring = state.ring.write().await;
        ring.write(b"hello world");
    }
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/signal")
        .json(&serde_json::json!({"signal": "SIGINT"}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":true"));

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Signal(2))));
    Ok(())
}

#[tokio::test]
async fn status_running() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn agent_nudge_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/agent/respond")
        .json(&serde_json::json!({"accept": true}))
        .await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn write_endpoint_conflict_409() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    // Pre-acquire the write lock via WS
    state
        .write_lock
        .acquire_ws("other-client")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp = server
        .post("/api/v1/input")
        .json(&serde_json::json!({"text": "hello"}))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn auth_rejects_without_token() -> anyhow::Result<()> {
    let (input_tx, _rx) = mpsc::channel(16);
    let (output_tx, _) = broadcast::channel::<OutputEvent>(16);
    let (state_tx, _) = broadcast::channel::<StateChangeEvent>(16);

    let state = Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(4096))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(1234)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        auth_token: Some("secret".to_owned()),
        nudge_encoder: None,
        respond_encoder: None,
    });

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

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
