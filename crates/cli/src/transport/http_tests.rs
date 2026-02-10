// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use axum::http::StatusCode;
use base64::Engine;

use crate::driver::AgentState;
use crate::event::InputEvent;
use crate::test_support::{AnyhowExt, AppStateBuilder, StubNudgeEncoder};
use crate::transport::build_router;

fn test_state() -> (Arc<crate::transport::state::AppState>, tokio::sync::mpsc::Receiver<InputEvent>)
{
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
    let content_type =
        resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");
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
async fn input_raw_sends_event() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // "hello" base64-encoded
    let resp =
        server.post("/api/v1/input/raw").json(&serde_json::json!({"data": "aGVsbG8="})).await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"bytes_written\":5"), "body: {body}");

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(_))));
    Ok(())
}

#[tokio::test]
async fn input_raw_rejects_bad_base64() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/input/raw")
        .json(&serde_json::json!({"data": "not-valid-base64!!!"}))
        .await;
    resp.assert_status(StatusCode::BAD_REQUEST);
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

    let resp =
        server.post("/api/v1/resize").json(&serde_json::json!({"cols": 120, "rows": 40})).await;
    resp.assert_status(StatusCode::OK);

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Resize { cols: 120, rows: 40 })));
    Ok(())
}

#[tokio::test]
async fn signal_delivers() -> anyhow::Result<()> {
    let (state, mut rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.post("/api/v1/signal").json(&serde_json::json!({"signal": "SIGINT"})).await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":true"));

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Signal(crate::event::PtySignal::Int))));
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
    assert!(body.contains("\"uptime_secs\":"), "body: {body}");
    Ok(())
}

#[tokio::test]
async fn agent_state_without_driver_returns_state() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"state\":\"starting\""), "body: {body}");
    Ok(())
}

#[tokio::test]
async fn agent_nudge_not_ready_503() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    // ready defaults to false — nudge should be gated
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp =
        server.post("/api/v1/agent/nudge").json(&serde_json::json!({"message": "hello"})).await;
    resp.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    Ok(())
}

#[tokio::test]
async fn agent_nudge_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp =
        server.post("/api/v1/agent/nudge").json(&serde_json::json!({"message": "hello"})).await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn agent_respond_no_driver_404() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp =
        server.post("/api/v1/agent/respond").json(&serde_json::json!({"accept": true})).await;
    resp.assert_status(StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn auth_rejects_without_token() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().child_pid(1234).auth_token("secret").build();

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
        .agent_state(AgentState::Error { detail: "rate_limit_error".to_owned() })
        .build();
    // Populate error fields as session loop would
    *state.driver.error.write().await = Some(crate::transport::state::ErrorInfo {
        detail: "rate_limit_error".to_owned(),
        category: crate::driver::ErrorCategory::RateLimited,
    });

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"error_detail\":\"rate_limit_error\""), "body: {body}");
    assert!(body.contains("\"error_category\":\"rate_limited\""), "body: {body}");
    Ok(())
}

#[tokio::test]
async fn agent_state_omits_error_fields_when_not_error() -> anyhow::Result<()> {
    let (state, _rx) =
        AppStateBuilder::new().child_pid(1234).agent_state(AgentState::Working).build();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/agent/state").await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(!body.contains("error_detail"), "error_detail should be absent: {body}");
    assert!(!body.contains("error_category"), "error_category should be absent: {body}");
    Ok(())
}

#[tokio::test]
async fn agent_nudge_rejected_when_working() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Working)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp =
        server.post("/api/v1/agent/nudge").json(&serde_json::json!({"message": "hello"})).await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"delivered\":false"));
    assert!(body.contains("agent is working"), "body: {body}");
    Ok(())
}

#[tokio::test]
async fn agent_nudge_delivered_when_waiting() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Idle)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp =
        server.post("/api/v1/agent/nudge").json(&serde_json::json!({"message": "hello"})).await;
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

    let resp =
        server.post("/api/v1/resize").json(&serde_json::json!({"cols": 0, "rows": 24})).await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn resize_rejects_zero_rows() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).map_err(|e| anyhow::anyhow!("{e}"))?;

    let resp =
        server.post("/api/v1/resize").json(&serde_json::json!({"cols": 80, "rows": 0})).await;
    resp.assert_status(StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_token() -> anyhow::Result<()> {
    let (state, _rx) = test_state();
    assert!(!state.lifecycle.shutdown.is_cancelled());
    let app = build_router(state.clone());
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.post("/api/v1/shutdown").json(&serde_json::json!({})).await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"accepted\":true"));
    assert!(state.lifecycle.shutdown.is_cancelled());
    Ok(())
}

#[tokio::test]
async fn shutdown_requires_auth() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().child_pid(1234).auth_token("secret").build();
    let app = build_router(state.clone());
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Without auth token — should be rejected
    let resp = server.post("/api/v1/shutdown").json(&serde_json::json!({})).await;
    resp.assert_status(StatusCode::UNAUTHORIZED);
    assert!(!state.lifecycle.shutdown.is_cancelled());

    // With auth token — should succeed
    let resp = server
        .post("/api/v1/shutdown")
        .add_header(
            axum::http::header::AUTHORIZATION,
            axum::http::HeaderValue::from_static("Bearer secret"),
        )
        .json(&serde_json::json!({}))
        .await;
    resp.assert_status(StatusCode::OK);
    assert!(state.lifecycle.shutdown.is_cancelled());
    Ok(())
}

use crate::start::{StartConfig, StartEventConfig};
use crate::stop::{StopConfig, StopMode};

fn stop_state(
    config: StopConfig,
) -> (Arc<crate::transport::state::AppState>, tokio::sync::mpsc::Receiver<InputEvent>) {
    AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Working)
        .stop_config(config)
        .build()
}

#[tokio::test]
async fn hooks_stop_allow_mode_returns_empty() -> anyhow::Result<()> {
    let (state, _rx) = stop_state(StopConfig::default());
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
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
    let (state, _rx) = stop_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["decision"], "block");
    assert!(body["reason"].as_str().unwrap_or("").contains("Finish work first."));
    Ok(())
}

#[tokio::test]
async fn hooks_stop_signal_mode_allows_after_signal() -> anyhow::Result<()> {
    let config = StopConfig { mode: StopMode::Signal, prompt: None, schema: None };
    let (state, _rx) = stop_state(config);
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
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "should allow after signal");
    Ok(())
}

#[tokio::test]
async fn hooks_stop_safety_valve_always_allows() -> anyhow::Result<()> {
    let config = StopConfig { mode: StopMode::Signal, prompt: None, schema: None };
    let (state, _rx) = stop_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // stop_hook_active = true => must allow
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": true}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "safety valve must allow");
    Ok(())
}

#[tokio::test]
async fn hooks_stop_unrecoverable_error_allows() -> anyhow::Result<()> {
    let config = StopConfig { mode: StopMode::Signal, prompt: None, schema: None };
    let (state, _rx) = stop_state(config);
    // Set unrecoverable error state.
    *state.driver.error.write().await = Some(crate::transport::state::ErrorInfo {
        detail: "invalid api key".to_owned(),
        category: crate::driver::ErrorCategory::Unauthorized,
    });

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "unrecoverable error should allow");
    Ok(())
}

#[tokio::test]
async fn resolve_stop_stores_body() -> anyhow::Result<()> {
    let (state, _rx) = stop_state(StopConfig::default());
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
    assert!(state.stop.signaled.load(std::sync::atomic::Ordering::Acquire));
    // Check that signal body is stored.
    let stored = state.stop.signal_body.read().await;
    let stored_val = stored.as_ref().expect("signal body should be stored");
    assert_eq!(stored_val["status"], "complete");
    Ok(())
}

#[tokio::test]
async fn get_stop_config_returns_current() -> anyhow::Result<()> {
    let config =
        StopConfig { mode: StopMode::Signal, prompt: Some("test prompt".to_owned()), schema: None };
    let (state, _rx) = stop_state(config);
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
    let (state, _rx) = stop_state(StopConfig::default());
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
    let config = StopConfig { mode: StopMode::Signal, prompt: None, schema: None };
    let (state, _rx) = stop_state(config);
    let mut stop_rx = state.stop.stop_tx.subscribe();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // First call should block.
    server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;

    let event = stop_rx.try_recv()?;
    assert_eq!(event.stop_type.as_str(), "blocked");
    assert_eq!(event.seq, 0);
    Ok(())
}

#[tokio::test]
async fn signal_consumed_after_stop_check() -> anyhow::Result<()> {
    let config = StopConfig { mode: StopMode::Signal, prompt: None, schema: None };
    let (state, _rx) = stop_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Signal, then check stop — should allow.
    server.post("/api/v1/hooks/stop/resolve").json(&serde_json::json!({"ok": true})).await;
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("decision").is_none(), "first check after signal should allow");

    // Second stop check should block again (signal was consumed).
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["decision"], "block", "second check should block (signal consumed)");
    Ok(())
}

#[tokio::test]
async fn auth_exempt_for_hooks_stop_and_resolve() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().child_pid(1234).auth_token("secret-token").build();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Hooks stop should work without auth.
    let resp = server
        .post("/api/v1/hooks/stop")
        .json(&serde_json::json!({"event": "stop", "data": {"stop_hook_active": false}}))
        .await;
    resp.assert_status(StatusCode::OK);

    // Resolve stop should work without auth.
    let resp =
        server.post("/api/v1/hooks/stop/resolve").json(&serde_json::json!({"ok": true})).await;
    resp.assert_status(StatusCode::OK);

    // Start hook should work without auth.
    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {}}))
        .await;
    resp.assert_status(StatusCode::OK);

    // But other endpoints should still require auth.
    let resp = server.get("/api/v1/screen").await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    Ok(())
}

fn start_state(
    config: StartConfig,
) -> (Arc<crate::transport::state::AppState>, tokio::sync::mpsc::Receiver<InputEvent>) {
    AppStateBuilder::new()
        .child_pid(1234)
        .agent_state(AgentState::Working)
        .start_config(config)
        .build()
}

#[tokio::test]
async fn hooks_start_empty_config_returns_empty() -> anyhow::Result<()> {
    let (state, _rx) = start_state(StartConfig::default());
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.is_empty(), "empty config should return empty body: {body}");
    Ok(())
}

#[tokio::test]
async fn hooks_start_text_returns_base64_script() -> anyhow::Result<()> {
    let config = StartConfig { text: Some("hello context".to_owned()), ..Default::default() };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("base64 -d"), "should contain base64 decode: {body}");
    assert!(body.contains("printf"), "should contain printf: {body}");
    Ok(())
}

#[tokio::test]
async fn hooks_start_shell_returns_commands() -> anyhow::Result<()> {
    let config = StartConfig {
        shell: vec!["echo one".to_owned(), "echo two".to_owned()],
        ..Default::default()
    };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert_eq!(body, "echo one\necho two");
    Ok(())
}

#[tokio::test]
async fn hooks_start_text_and_shell_combined() -> anyhow::Result<()> {
    let config = StartConfig {
        text: Some("ctx".to_owned()),
        shell: vec!["echo done".to_owned()],
        ..Default::default()
    };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("base64 -d"));
    assert_eq!(lines[1], "echo done");
    Ok(())
}

#[tokio::test]
async fn hooks_start_event_override() -> anyhow::Result<()> {
    let mut events = std::collections::BTreeMap::new();
    events.insert(
        "clear".to_owned(),
        StartEventConfig { text: Some("override".to_owned()), shell: vec![] },
    );
    let config = StartConfig { text: Some("default".to_owned()), shell: vec![], event: events };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {"source": "clear"}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    // Verify override text is used (base64 of "override" not "default")
    let override_b64 = base64::engine::general_purpose::STANDARD.encode(b"override");
    assert!(body.contains(&override_b64), "should use override config: {body}");
    Ok(())
}

#[tokio::test]
async fn hooks_start_event_fallback() -> anyhow::Result<()> {
    let mut events = std::collections::BTreeMap::new();
    events.insert("clear".to_owned(), StartEventConfig::default());
    let config = StartConfig { text: Some("fallback".to_owned()), shell: vec![], event: events };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Unknown source → falls back to top-level
    let resp = server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {"source": "resume"}}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    let fallback_b64 = base64::engine::general_purpose::STANDARD.encode(b"fallback");
    assert!(body.contains(&fallback_b64), "should fall back to top-level: {body}");
    Ok(())
}

#[tokio::test]
async fn hooks_start_emits_event() -> anyhow::Result<()> {
    let config = StartConfig { text: Some("ctx".to_owned()), ..Default::default() };
    let (state, _rx) = start_state(config);
    let mut start_rx = state.start.start_tx.subscribe();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    server
        .post("/api/v1/hooks/start")
        .json(
            &serde_json::json!({"event": "start", "data": {"source": "init", "session_id": "s1"}}),
        )
        .await;

    let event = start_rx.try_recv()?;
    assert_eq!(event.source, "init");
    assert_eq!(event.session_id.as_deref(), Some("s1"));
    assert!(event.injected);
    assert_eq!(event.seq, 0);
    Ok(())
}

#[tokio::test]
async fn get_start_config_returns_current() -> anyhow::Result<()> {
    let config = StartConfig { text: Some("test text".to_owned()), ..Default::default() };
    let (state, _rx) = start_state(config);
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    let resp = server.get("/api/v1/config/start").await;
    resp.assert_status(StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["text"], "test text");
    Ok(())
}

#[tokio::test]
async fn put_start_config_updates() -> anyhow::Result<()> {
    let (state, _rx) = start_state(StartConfig::default());
    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // Default has no text.
    let resp = server.get("/api/v1/config/start").await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert!(body.get("text").is_none());

    // Update.
    let resp = server
        .put("/api/v1/config/start")
        .json(&serde_json::json!({"text": "new context", "shell": ["echo hi"]}))
        .await;
    resp.assert_status(StatusCode::OK);
    let body = resp.text();
    assert!(body.contains("\"updated\":true"));

    // Verify.
    let resp = server.get("/api/v1/config/start").await;
    let body: serde_json::Value = serde_json::from_str(&resp.text())?;
    assert_eq!(body["text"], "new context");
    assert_eq!(body["shell"][0], "echo hi");
    Ok(())
}

#[tokio::test]
async fn hooks_start_extracts_session_type_as_source() -> anyhow::Result<()> {
    let config = StartConfig::default();
    let (state, _rx) = start_state(config);
    let mut start_rx = state.start.start_tx.subscribe();

    let app = build_router(state);
    let server = axum_test::TestServer::new(app).anyhow()?;

    // session_type should be used as source when source is absent
    server
        .post("/api/v1/hooks/start")
        .json(&serde_json::json!({"event": "start", "data": {"session_type": "init"}}))
        .await;

    let event = start_rx.try_recv()?;
    assert_eq!(event.source, "init");
    Ok(())
}
