// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! HTTP request/response types and axum handler implementations.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::auth::auth_middleware;
use super::{encode_key, parse_signal, AppState, ErrorResponse};
use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::InputEvent;
use crate::screen::CursorPosition;

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub agent_type: String,
    pub terminal: TerminalSize,
    pub ws_clients: i32,
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
    pub cursor: Option<CursorPosition>,
    pub sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutputQuery {
    #[serde(default)]
    pub offset: u64,
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
pub struct StatusResponse {
    pub state: String,
    pub pid: Option<i32>,
    pub exit_code: Option<i32>,
    pub screen_seq: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub ws_clients: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRequest {
    pub text: String,
    #[serde(default)]
    pub enter: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputResponse {
    pub bytes_written: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysRequest {
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysResponse {
    pub bytes_written: i32,
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
    pub agent_type: String,
    pub state: String,
    pub since_seq: u64,
    pub screen_seq: u64,
    pub detection_tier: String,
    pub prompt: Option<PromptContext>,
    pub idle_grace_remaining_secs: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeRequest {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeResponse {
    pub delivered: bool,
    pub state_before: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondRequest {
    pub accept: Option<bool>,
    pub option: Option<i32>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondResponse {
    pub delivered: bool,
    pub prompt_type: Option<String>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the axum router with all HTTP endpoints.
pub fn build_router(state: Arc<AppState>) -> Router {
    let auth_token: Option<Arc<str>> = state.auth_token.as_deref().map(Arc::from);

    Router::new()
        .route("/api/v1/health", get(handle_health))
        .route("/api/v1/screen", get(handle_screen))
        .route("/api/v1/screen/text", get(handle_screen_text))
        .route("/api/v1/output", get(handle_output))
        .route("/api/v1/status", get(handle_status))
        .route("/api/v1/input", post(handle_input))
        .route("/api/v1/input/keys", post(handle_keys))
        .route("/api/v1/resize", post(handle_resize))
        .route("/api/v1/signal", post(handle_signal))
        .route("/api/v1/agent/state", get(handle_agent_state))
        .route("/api/v1/agent/nudge", post(handle_nudge))
        .route("/api/v1/agent/respond", post(handle_respond))
        .route("/ws", get(super::ws::ws_upgrade))
        .layer(axum::middleware::from_fn_with_state(
            auth_token,
            auth_middleware,
        ))
        .with_state(state)
}

/// Build a minimal health-only router (for `--health-port`).
pub fn build_health_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(handle_health))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let pid = *state.pid.read().await;
    let uptime = state.start_time.elapsed().as_secs() as i64;
    let screen = state.screen.read().await;
    let snap = screen.snapshot();
    let ws = state.ws_clients.load(Ordering::Relaxed);

    Json(HealthResponse {
        status: "ok".to_owned(),
        pid: pid.map(|p| p as i32),
        uptime_secs: uptime,
        agent_type: state.agent_type.clone(),
        terminal: TerminalSize {
            cols: snap.cols,
            rows: snap.rows,
        },
        ws_clients: ws,
    })
}

async fn handle_screen(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ScreenQuery>,
) -> Json<ScreenResponse> {
    let screen = state.screen.read().await;
    let snap = screen.snapshot();

    Json(ScreenResponse {
        lines: snap.lines,
        cols: snap.cols,
        rows: snap.rows,
        alt_screen: snap.alt_screen,
        cursor: if query.cursor {
            Some(snap.cursor)
        } else {
            None
        },
        sequence: snap.sequence,
    })
}

async fn handle_screen_text(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let screen = state.screen.read().await;
    let snap = screen.snapshot();
    let text = snap.lines.join("\n");
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        text,
    )
}

async fn handle_output(
    State(state): State<Arc<AppState>>,
    Query(query): Query<OutputQuery>,
) -> Result<Json<OutputResponse>, (StatusCode, Json<ErrorResponse>)> {
    let ring = state.ring.read().await;
    let total = ring.total_written();

    let data = match ring.read_from(query.offset) {
        Some((a, b)) => {
            let mut buf = Vec::with_capacity(a.len() + b.len());
            buf.extend_from_slice(a);
            buf.extend_from_slice(b);
            if let Some(limit) = query.limit {
                buf.truncate(limit);
            }
            buf
        }
        None => Vec::new(),
    };

    let next_offset = query.offset + data.len() as u64;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);

    Ok(Json(OutputResponse {
        data: encoded,
        offset: query.offset,
        next_offset,
        total_written: total,
    }))
}

async fn handle_status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let pid = *state.pid.read().await;
    let agent = state.agent_state.read().await;
    let screen = state.screen.read().await;
    let ring = state.ring.read().await;

    let exit_code = if let AgentState::Exited { status } = &*agent {
        status.code
    } else {
        None
    };

    let bw = state.bytes_written.load(Ordering::Relaxed);
    let ws = state.ws_clients.load(Ordering::Relaxed);

    Json(StatusResponse {
        state: agent.as_str().to_owned(),
        pid: pid.map(|p| p as i32),
        exit_code,
        screen_seq: screen.seq(),
        bytes_read: ring.total_written(),
        bytes_written: bw,
        ws_clients: ws,
    })
}

async fn handle_input(
    State(state): State<Arc<AppState>>,
    Json(req): Json<InputRequest>,
) -> Result<(StatusCode, Json<InputResponse>), (StatusCode, Json<ErrorResponse>)> {
    let mut payload = req.text.into_bytes();
    if req.enter {
        payload.push(b'\r');
    }
    let len = payload.len() as i32;
    state
        .input_tx
        .send(InputEvent::Write(Bytes::from(payload)))
        .await
        .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
    Ok((StatusCode::OK, Json(InputResponse { bytes_written: len })))
}

async fn handle_keys(
    State(state): State<Arc<AppState>>,
    Json(req): Json<KeysRequest>,
) -> Result<(StatusCode, Json<KeysResponse>), (StatusCode, Json<ErrorResponse>)> {
    let mut total = 0i32;
    for key in &req.keys {
        let encoded = encode_key(key)
            .ok_or_else(|| ErrorCode::BadRequest.to_http_response(format!("unknown key: {key}")))?;
        total += encoded.len() as i32;
        state
            .input_tx
            .send(InputEvent::Write(Bytes::from(encoded)))
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
    }
    Ok((
        StatusCode::OK,
        Json(KeysResponse {
            bytes_written: total,
        }),
    ))
}

async fn handle_resize(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ResizeRequest>,
) -> Result<(StatusCode, Json<ResizeResponse>), (StatusCode, Json<ErrorResponse>)> {
    if req.cols == 0 || req.rows == 0 {
        return Err(ErrorCode::BadRequest.to_http_response("cols and rows must be positive"));
    }
    state
        .input_tx
        .send(InputEvent::Resize {
            cols: req.cols,
            rows: req.rows,
        })
        .await
        .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
    Ok((
        StatusCode::OK,
        Json(ResizeResponse {
            cols: req.cols,
            rows: req.rows,
        }),
    ))
}

async fn handle_signal(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SignalRequest>,
) -> Result<(StatusCode, Json<SignalResponse>), (StatusCode, Json<ErrorResponse>)> {
    let signum = parse_signal(&req.signal).ok_or_else(|| {
        ErrorCode::BadRequest.to_http_response(format!("unknown signal: {}", req.signal))
    })?;
    state
        .input_tx
        .send(InputEvent::Signal(signum))
        .await
        .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
    Ok((StatusCode::OK, Json(SignalResponse { delivered: true })))
}

async fn handle_agent_state(State(state): State<Arc<AppState>>) -> Json<AgentStateResponse> {
    let agent = state.agent_state.read().await;
    let screen = state.screen.read().await;

    Json(AgentStateResponse {
        agent_type: state.agent_type.clone(),
        state: agent.as_str().to_owned(),
        since_seq: 0,
        screen_seq: screen.seq(),
        detection_tier: String::new(),
        prompt: agent.prompt().cloned(),
        idle_grace_remaining_secs: None,
    })
}

async fn handle_nudge(
    State(state): State<Arc<AppState>>,
    Json(req): Json<NudgeRequest>,
) -> Result<Json<NudgeResponse>, (StatusCode, Json<ErrorResponse>)> {
    let agent = state.agent_state.read().await;
    let state_before = Some(agent.as_str().to_owned());

    let encoder = state
        .nudge_encoder
        .as_ref()
        .ok_or_else(|| ErrorCode::NoDriver.to_http_response("no nudge encoder configured"))?;

    match &*agent {
        AgentState::WaitingForInput => {}
        other => {
            return Ok(Json(NudgeResponse {
                delivered: false,
                state_before,
                reason: Some(format!("agent is {}", other.as_str())),
            }));
        }
    }

    let steps = encoder.encode(&req.message);
    drop(agent);

    for step in steps {
        state
            .input_tx
            .send(InputEvent::Write(Bytes::from(step.bytes)))
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
        if let Some(delay) = step.delay_after {
            tokio::time::sleep(delay).await;
        }
    }

    Ok(Json(NudgeResponse {
        delivered: true,
        state_before,
        reason: None,
    }))
}

async fn handle_respond(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RespondRequest>,
) -> Result<Json<RespondResponse>, (StatusCode, Json<ErrorResponse>)> {
    let agent = state.agent_state.read().await;

    let encoder = state
        .respond_encoder
        .as_ref()
        .ok_or_else(|| ErrorCode::NoDriver.to_http_response("no respond encoder configured"))?;

    let steps = match &*agent {
        AgentState::PermissionPrompt { .. } => {
            let accept = req.accept.unwrap_or(false);
            encoder.encode_permission(accept)
        }
        AgentState::PlanPrompt { .. } => {
            let accept = req.accept.unwrap_or(false);
            encoder.encode_plan(accept, req.text.as_deref())
        }
        AgentState::AskUser { .. } => {
            encoder.encode_question(req.option.map(|o| o as u32), req.text.as_deref())
        }
        other => {
            return Err(ErrorCode::NoPrompt
                .to_http_response(format!("agent is {} (no active prompt)", other.as_str())));
        }
    };

    let prompt_type = Some(agent.as_str().to_owned());
    drop(agent);

    for step in steps {
        state
            .input_tx
            .send(InputEvent::Write(Bytes::from(step.bytes)))
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_http_response("input channel closed"))?;
        if let Some(delay) = step.delay_after {
            tokio::time::sleep(delay).await;
        }
    }

    Ok(Json(RespondResponse {
        delivered: true,
        prompt_type,
        reason: None,
    }))
}
