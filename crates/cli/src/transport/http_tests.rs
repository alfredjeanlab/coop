// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use axum::http::StatusCode;

use crate::driver::{AgentState, NudgeEncoder, NudgeStep};
use crate::event::InputEvent;
use crate::test_support::{AnyhowExt, TestAppStateBuilder};
use crate::transport::build_router;

fn test_state() -> (
    Arc<crate::transport::state::AppState>,
    tokio::sync::mpsc::Receiver<InputEvent>,
) {
    TestAppStateBuilder::new().child_pid(1234).build()
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
        let mut ring = state.ring.write().await;
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
    assert!(matches!(event, Some(InputEvent::Signal(2))));
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
async fn write_endpoint_conflict_409() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    // Pre-acquire the write lock via WS
    state.write_lock.acquire_ws("other-client").anyhow()?;

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/input")
        .json(&serde_json::json!({"text": "hello"}))
        .await;
    resp.assert_status(StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn auth_rejects_without_token() -> anyhow::Result<()> {
    let (state, _rx) = TestAppStateBuilder::new()
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

struct StubNudgeEncoder;
impl NudgeEncoder for StubNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: message.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
}

#[tokio::test]
async fn agent_nudge_rejected_when_working() -> anyhow::Result<()> {
    let (state, _rx) = TestAppStateBuilder::new()
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
    let (state, _rx) = TestAppStateBuilder::new()
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
