// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! HTTP request/response types and axum handler implementations.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::driver::{ErrorCategory, PromptContext};
use crate::error::ErrorCode;
use crate::screen::CursorPosition;
use crate::start::{compose_start_script, StartConfig};
use crate::stop::{generate_block_reason, StopConfig, StopMode, StopType};
use crate::transport::handler::{
    compute_health, compute_status, error_message, handle_input, handle_input_raw, handle_keys,
    handle_nudge, handle_resize, handle_respond, handle_signal, TransportQuestionAnswer,
};
use crate::transport::read_ring_combined;
use crate::transport::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub agent: String,
    pub terminal: TerminalSize,
    pub ws_clients: i32,
    pub ready: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScreenQuery {
    #[serde(default)]
    pub format: ScreenFormat,
    #[serde(default)]
    pub cursor: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenFormat {
    #[default]
    Text,
    Ansi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenResponse {
    pub lines: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub alt_screen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<CursorPosition>,
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutputQuery {
    #[serde(default)]
    pub offset: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputResponse {
    pub data: String,
    pub offset: u64,
    pub next_offset: u64,
    pub total_written: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRequest {
    pub text: String,
    #[serde(default)]
    pub enter: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRawRequest {
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputResponse {
    pub bytes_written: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysRequest {
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResizeResponse {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRequest {
    pub signal: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalResponse {
    pub delivered: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStateResponse {
    pub agent: String,
    pub state: String,
    pub since_seq: u64,
    pub screen_seq: u64,
    pub detection_tier: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<TransportQuestionAnswer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<i32>,
}

/// `GET /api/v1/health`
pub async fn health(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let h = compute_health(&s).await;
    Json(HealthResponse {
        status: h.status,
        pid: h.pid,
        uptime_secs: h.uptime_secs,
        agent: h.agent,
        terminal: TerminalSize { cols: h.terminal_cols, rows: h.terminal_rows },
        ws_clients: h.ws_clients,
        ready: h.ready,
    })
}

/// Response for the readiness probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyResponse {
    pub ready: bool,
}

/// `GET /api/v1/ready` — readiness probe (200 when ready, 503 otherwise).
pub async fn ready(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let is_ready = s.ready.load(Ordering::Acquire);
    let status = if is_ready {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(ReadyResponse { ready: is_ready }))
}

/// `GET /api/v1/screen`
pub async fn screen(
    State(s): State<Arc<AppState>>,
    Query(q): Query<ScreenQuery>,
) -> impl IntoResponse {
    let snap = s.terminal.screen.read().await.snapshot();

    Json(ScreenResponse {
        lines: snap.lines,
        cols: snap.cols,
        rows: snap.rows,
        alt_screen: snap.alt_screen,
        cursor: if q.cursor { Some(snap.cursor) } else { None },
        seq: snap.sequence,
    })
}

/// `GET /api/v1/screen/text`
pub async fn screen_text(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = s.terminal.screen.read().await.snapshot();
    let text = snap.lines.join("\n");
    ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], text)
}

/// `GET /api/v1/output`
pub async fn output(
    State(s): State<Arc<AppState>>,
    Query(q): Query<OutputQuery>,
) -> impl IntoResponse {
    let ring = s.terminal.ring.read().await;
    let total = ring.total_written();

    let mut combined = read_ring_combined(&ring, q.offset);

    if let Some(limit) = q.limit {
        combined.truncate(limit);
    }

    let read_len = combined.len() as u64;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);

    Json(OutputResponse {
        data: encoded,
        offset: q.offset,
        next_offset: q.offset + read_len,
        total_written: total,
    })
}

/// `GET /api/v1/status`
pub async fn status(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    Json(compute_status(&s).await)
}

/// `POST /api/v1/input`
pub async fn input(
    State(s): State<Arc<AppState>>,
    Json(req): Json<InputRequest>,
) -> impl IntoResponse {
    let len = handle_input(&s, req.text, req.enter).await;
    Json(InputResponse { bytes_written: len }).into_response()
}

/// `POST /api/v1/input/raw`
pub async fn input_raw(
    State(s): State<Arc<AppState>>,
    Json(req): Json<InputRawRequest>,
) -> impl IntoResponse {
    let decoded = match base64::engine::general_purpose::STANDARD.decode(&req.data) {
        Ok(d) => d,
        Err(_) => {
            return ErrorCode::BadRequest.to_http_response("invalid base64 data").into_response()
        }
    };
    let len = handle_input_raw(&s, decoded).await;
    Json(InputResponse { bytes_written: len }).into_response()
}

/// `POST /api/v1/input/keys`
pub async fn input_keys(
    State(s): State<Arc<AppState>>,
    Json(req): Json<KeysRequest>,
) -> impl IntoResponse {
    match handle_keys(&s, &req.keys).await {
        Ok(len) => Json(InputResponse { bytes_written: len }).into_response(),
        Err(bad_key) => ErrorCode::BadRequest
            .to_http_response(format!("unknown key: {bad_key}"))
            .into_response(),
    }
}

/// `POST /api/v1/resize`
pub async fn resize(
    State(s): State<Arc<AppState>>,
    Json(req): Json<ResizeRequest>,
) -> impl IntoResponse {
    match handle_resize(&s, req.cols, req.rows).await {
        Ok(()) => Json(ResizeResponse { cols: req.cols, rows: req.rows }).into_response(),
        Err(_) => {
            ErrorCode::BadRequest.to_http_response("cols and rows must be positive").into_response()
        }
    }
}

/// `POST /api/v1/signal`
pub async fn signal(
    State(s): State<Arc<AppState>>,
    Json(req): Json<SignalRequest>,
) -> impl IntoResponse {
    match handle_signal(&s, &req.signal).await {
        Ok(()) => Json(SignalResponse { delivered: true }).into_response(),
        Err(bad_signal) => ErrorCode::BadRequest
            .to_http_response(format!("unknown signal: {bad_signal}"))
            .into_response(),
    }
}

/// `GET /api/v1/agent/state`
pub async fn agent_state(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let state = s.driver.agent_state.read().await;
    let screen = s.terminal.screen.read().await;

    Json(AgentStateResponse {
        agent: s.config.agent.to_string(),
        state: state.as_str().to_owned(),
        since_seq: s.driver.state_seq.load(Ordering::Acquire),
        screen_seq: screen.seq(),
        detection_tier: s.driver.detection_tier_str(),
        prompt: state.prompt().cloned(),
        error_detail: s.driver.error.read().await.as_ref().map(|e| e.detail.clone()),
        error_category: s
            .driver
            .error
            .read()
            .await
            .as_ref()
            .map(|e| e.category.as_str().to_owned()),
        last_message: s.driver.last_message.read().await.clone(),
    })
    .into_response()
}

/// `POST /api/v1/agent/nudge`
pub async fn agent_nudge(
    State(s): State<Arc<AppState>>,
    Json(req): Json<NudgeRequest>,
) -> impl IntoResponse {
    match handle_nudge(&s, &req.message).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(code) => code.to_http_response(error_message(code)).into_response(),
    }
}

/// `POST /api/v1/agent/respond`
pub async fn agent_respond(
    State(s): State<Arc<AppState>>,
    Json(req): Json<RespondRequest>,
) -> impl IntoResponse {
    match handle_respond(&s, req.accept, req.option, req.text.as_deref(), &req.answers).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(code) => code.to_http_response(error_message(code)).into_response(),
    }
}

/// Event-wrapped input from the stop hook (piped from stdin via curl).
///
/// Matches the same `{"event":"stop","data":{...}}` envelope that hooks
/// write to the FIFO pipe, so the endpoint receives the same format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookInput {
    // NOTE(compat): Maintain consistent structure for all hook payloads
    #[allow(dead_code)]
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<StopHookData>,
}

/// Inner data carried inside the stop-event envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StopHookData {
    /// When `true`, this is a safety-valve invocation that must be allowed.
    #[serde(default)]
    pub stop_hook_active: bool,
}

/// Verdict returned to the hook script.
///
/// Empty object `{}` means "allow" (no `decision` field).
/// `{"decision":"block","reason":"..."}` means "block".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopHookVerdict {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message: Option<String>,
}

/// `POST /api/v1/hooks/stop` — called by the hook script, returns verdict.
pub async fn hooks_stop(
    State(s): State<Arc<AppState>>,
    Json(input): Json<StopHookInput>,
) -> impl IntoResponse {
    let stop = &s.stop;
    let config = stop.config.read().await;
    let last_message = s.driver.last_message.read().await.clone();

    // 1. Mode = Allow → always allow.
    if config.mode == StopMode::Allow {
        drop(config);
        stop.emit(StopType::Allowed, None, None);
        return Json(StopHookVerdict { decision: None, reason: None, last_message })
            .into_response();
    }

    // 2. Safety valve: stop_hook_active = true → must allow.
    let stop_hook_active = input.data.as_ref().is_some_and(|d| d.stop_hook_active);
    if stop_hook_active {
        drop(config);
        stop.emit(StopType::SafetyValve, None, None);
        return Json(StopHookVerdict { decision: None, reason: None, last_message })
            .into_response();
    }

    // 3. Unrecoverable error → allow.
    {
        let error = s.driver.error.read().await;
        if let Some(ref info) = *error {
            let is_unrecoverable =
                matches!(info.category, ErrorCategory::Unauthorized | ErrorCategory::OutOfCredits);
            if is_unrecoverable {
                let detail = Some(info.detail.clone());
                drop(error);
                drop(config);
                stop.emit(StopType::Error, None, detail);
                return Json(StopHookVerdict { decision: None, reason: None, last_message })
                    .into_response();
            }
        }
    }

    // 4. Signal received → allow and reset.
    if stop.signaled.swap(false, std::sync::atomic::Ordering::AcqRel) {
        let body = stop.signal_body.write().await.take();
        drop(config);
        stop.emit(StopType::Signaled, body, None);
        return Json(StopHookVerdict { decision: None, reason: None, last_message })
            .into_response();
    }

    // 5. Block: generate reason and return block verdict.
    let reason = generate_block_reason(&config);
    drop(config);
    stop.emit(StopType::Blocked, None, None);
    Json(StopHookVerdict { decision: Some("block".to_owned()), reason: Some(reason), last_message })
        .into_response()
}

/// `POST /api/v1/hooks/stop/resolve` — store signal body, set flag.
pub async fn resolve_stop(
    State(s): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let stop = &s.stop;
    *stop.signal_body.write().await = Some(body);
    stop.signaled.store(true, std::sync::atomic::Ordering::Release);
    Json(serde_json::json!({ "accepted": true }))
}

/// `GET /api/v1/config/stop` — read current stop config.
pub async fn get_stop_config(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let config = s.stop.config.read().await;
    Json(config.clone())
}

/// `POST /api/v1/shutdown` — initiate graceful coop shutdown.
pub async fn shutdown(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    s.lifecycle.shutdown.cancel();
    Json(serde_json::json!({ "accepted": true }))
}

/// `PUT /api/v1/config/stop` — update stop config.
pub async fn put_stop_config(
    State(s): State<Arc<AppState>>,
    Json(new_config): Json<StopConfig>,
) -> impl IntoResponse {
    *s.stop.config.write().await = new_config;
    Json(serde_json::json!({ "updated": true }))
}

/// Event-wrapped input from the start hook (piped from stdin via curl).
///
/// Matches the `{"event":"start","data":{...}}` envelope that hooks
/// write to the FIFO pipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartHookInput {
    // NOTE(compat): Maintain consistent structure for all hook payloads
    #[allow(dead_code)]
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// `POST /api/v1/hooks/start` — called by the hook script, returns shell script.
pub async fn hooks_start(
    State(s): State<Arc<AppState>>,
    Json(input): Json<StartHookInput>,
) -> impl IntoResponse {
    let start = &s.start;
    let config = start.config.read().await;

    // Extract source from data.source or data.session_type, default "unknown".
    let source = input
        .data
        .as_ref()
        .and_then(|d| {
            d.get("source")
                .or_else(|| d.get("session_type"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_owned())
        })
        .unwrap_or_else(|| "unknown".to_owned());

    // Extract session_id from data.session_id.
    let session_id = input
        .data
        .as_ref()
        .and_then(|d| d.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_owned()));

    let script = compose_start_script(&config, &source);
    drop(config);

    let injected = !script.is_empty();
    start.emit(source, session_id, injected);

    ([(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], script)
}

/// `GET /api/v1/config/start` — read current start config.
pub async fn get_start_config(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let config = s.start.config.read().await;
    Json(config.clone())
}

/// `PUT /api/v1/config/start` — update start config.
pub async fn put_start_config(
    State(s): State<Arc<AppState>>,
    Json(new_config): Json<StartConfig>,
) -> impl IntoResponse {
    *s.start.config.write().await = new_config;
    Json(serde_json::json!({ "updated": true }))
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
