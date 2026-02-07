// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! HTTP request and response types for the coop REST API.
//!
//! All 14 routes are covered. Types use `String` for state fields to match
//! the wire format (e.g. `"working"`, `"permission_prompt"`). Prompt context
//! reuses [`crate::driver::PromptContext`] directly.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::InputEvent;
use crate::screen::CursorPosition;
use crate::transport::state::AppState;
use crate::transport::{error_response, keys_to_bytes};

// ---------------------------------------------------------------------------
// GET /api/v1/health
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub agent_type: String,
    pub terminal: TerminalSize,
    pub ws_clients: i32,
}

/// Terminal dimensions included in the health response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

// ---------------------------------------------------------------------------
// GET /api/v1/screen
// ---------------------------------------------------------------------------

/// Query parameters for `GET /api/v1/screen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenQuery {
    #[serde(default)]
    pub format: ScreenFormat,
    #[serde(default)]
    pub cursor: bool,
}

/// Output format for the screen endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenFormat {
    #[default]
    Text,
    Ansi,
}

/// Response for `GET /api/v1/screen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenResponse {
    pub lines: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub alt_screen: bool,
    pub cursor: Option<CursorPosition>,
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// GET /api/v1/output
// ---------------------------------------------------------------------------

/// Query parameters for `GET /api/v1/output`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputQuery {
    #[serde(default)]
    pub offset: u64,
    pub limit: Option<usize>,
}

/// Response for `GET /api/v1/output`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputResponse {
    pub data: String,
    pub offset: u64,
    pub next_offset: u64,
    pub total_written: u64,
}

// ---------------------------------------------------------------------------
// GET /api/v1/status
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/status`.
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

// ---------------------------------------------------------------------------
// POST /api/v1/input
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/input`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputRequest {
    pub text: String,
    #[serde(default)]
    pub enter: bool,
}

/// Response for `POST /api/v1/input`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputResponse {
    pub bytes_written: i32,
}

// ---------------------------------------------------------------------------
// POST /api/v1/input/keys
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/input/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysRequest {
    pub keys: Vec<String>,
}

/// Response for `POST /api/v1/input/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeysResponse {
    pub bytes_written: i32,
}

// ---------------------------------------------------------------------------
// POST /api/v1/resize
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/resize`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResizeRequest {
    pub cols: u16,
    pub rows: u16,
}

/// Response for `POST /api/v1/resize`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ResizeResponse {
    pub cols: u16,
    pub rows: u16,
}

// ---------------------------------------------------------------------------
// POST /api/v1/signal
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/signal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalRequest {
    pub signal: String,
}

/// Response for `POST /api/v1/signal`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalResponse {
    pub delivered: bool,
}

// ---------------------------------------------------------------------------
// GET /api/v1/agent/state
// ---------------------------------------------------------------------------

/// Response for `GET /api/v1/agent/state`.
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

// ---------------------------------------------------------------------------
// POST /api/v1/agent/nudge
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/agent/nudge`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeRequest {
    pub message: String,
}

/// Response for `POST /api/v1/agent/nudge`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NudgeResponse {
    pub delivered: bool,
    pub state_before: Option<String>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// POST /api/v1/agent/respond
// ---------------------------------------------------------------------------

/// Request body for `POST /api/v1/agent/respond`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondRequest {
    pub accept: Option<bool>,
    pub option: Option<i32>,
    pub text: Option<String>,
}

/// Response for `POST /api/v1/agent/respond`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RespondResponse {
    pub delivered: bool,
    pub prompt_type: Option<String>,
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/v1/health`
pub async fn health(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = s.screen.read().await.snapshot();
    let pid = s.child_pid.load(Ordering::Relaxed);
    let uptime = s.started_at.elapsed().as_secs() as i64;

    Json(HealthResponse {
        status: "running".to_owned(),
        pid: if pid == 0 { None } else { Some(pid as i32) },
        uptime_secs: uptime,
        agent_type: s.agent_type.clone(),
        terminal: TerminalSize {
            cols: snap.cols,
            rows: snap.rows,
        },
        ws_clients: s.ws_client_count.load(Ordering::Relaxed),
    })
}

/// `GET /api/v1/screen`
pub async fn screen(
    State(s): State<Arc<AppState>>,
    Query(q): Query<ScreenQuery>,
) -> impl IntoResponse {
    let snap = s.screen.read().await.snapshot();

    Json(ScreenResponse {
        lines: snap.lines,
        cols: snap.cols,
        rows: snap.rows,
        alt_screen: snap.alt_screen,
        cursor: if q.cursor { Some(snap.cursor) } else { None },
        sequence: snap.sequence,
    })
}

/// `GET /api/v1/screen/text`
pub async fn screen_text(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = s.screen.read().await.snapshot();
    let text = snap.lines.join("\n");
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        text,
    )
}

/// `GET /api/v1/output`
pub async fn output(
    State(s): State<Arc<AppState>>,
    Query(q): Query<OutputQuery>,
) -> impl IntoResponse {
    let ring = s.ring.read().await;
    let total = ring.total_written();

    let data = ring.read_from(q.offset);
    let (a, b) = data.unwrap_or((&[], &[]));

    let mut combined = Vec::with_capacity(a.len() + b.len());
    combined.extend_from_slice(a);
    combined.extend_from_slice(b);

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
    let agent = s.agent_state.read().await;
    let ring = s.ring.read().await;
    let screen = s.screen.read().await;
    let pid = s.child_pid.load(Ordering::Relaxed);
    let exit = s.exit_status.read().await;

    let state_str = match &*agent {
        AgentState::Exited { .. } => "exited",
        _ => {
            if pid == 0 {
                "starting"
            } else {
                "running"
            }
        }
    };

    Json(StatusResponse {
        state: state_str.to_owned(),
        pid: if pid == 0 { None } else { Some(pid as i32) },
        exit_code: exit.as_ref().and_then(|e| e.code),
        screen_seq: screen.seq(),
        bytes_read: ring.total_written(),
        bytes_written: ring.total_written(),
        ws_clients: s.ws_client_count.load(Ordering::Relaxed),
    })
}

/// `POST /api/v1/input`
pub async fn input(
    State(s): State<Arc<AppState>>,
    Json(req): Json<InputRequest>,
) -> impl IntoResponse {
    let _guard = match s.write_lock.acquire_http() {
        Ok(g) => g,
        Err(code) => {
            return error_response(code, "write lock held by another client").into_response()
        }
    };

    let mut data = req.text.into_bytes();
    if req.enter {
        data.push(b'\n');
    }
    let len = data.len() as i32;
    let _ = s.input_tx.send(InputEvent::Write(Bytes::from(data))).await;

    Json(InputResponse { bytes_written: len }).into_response()
}

/// `POST /api/v1/input/keys`
pub async fn input_keys(
    State(s): State<Arc<AppState>>,
    Json(req): Json<KeysRequest>,
) -> impl IntoResponse {
    let _guard = match s.write_lock.acquire_http() {
        Ok(g) => g,
        Err(code) => {
            return error_response(code, "write lock held by another client").into_response()
        }
    };

    let data = keys_to_bytes(&req.keys);
    let len = data.len() as i32;
    let _ = s.input_tx.send(InputEvent::Write(Bytes::from(data))).await;

    Json(KeysResponse { bytes_written: len }).into_response()
}

/// `POST /api/v1/resize`
pub async fn resize(
    State(s): State<Arc<AppState>>,
    Json(req): Json<ResizeRequest>,
) -> impl IntoResponse {
    let _ = s
        .input_tx
        .send(InputEvent::Resize {
            cols: req.cols,
            rows: req.rows,
        })
        .await;

    Json(ResizeResponse {
        cols: req.cols,
        rows: req.rows,
    })
}

/// `POST /api/v1/signal`
pub async fn signal(
    State(s): State<Arc<AppState>>,
    Json(req): Json<SignalRequest>,
) -> impl IntoResponse {
    let signum = match signal_from_name(&req.signal) {
        Some(n) => n,
        None => {
            return error_response(
                ErrorCode::BadRequest,
                format!("unknown signal: {}", req.signal),
            )
            .into_response()
        }
    };

    let _ = s.input_tx.send(InputEvent::Signal(signum)).await;
    Json(SignalResponse { delivered: true }).into_response()
}

/// `GET /api/v1/agent/state`
pub async fn agent_state(State(s): State<Arc<AppState>>) -> impl IntoResponse {
    if s.nudge_encoder.is_none() && s.respond_encoder.is_none() {
        return error_response(ErrorCode::NoDriver, "no agent driver configured").into_response();
    }

    let state = s.agent_state.read().await;
    let screen = s.screen.read().await;

    Json(AgentStateResponse {
        agent_type: s.agent_type.clone(),
        state: state.as_str().to_owned(),
        since_seq: 0,
        screen_seq: screen.seq(),
        detection_tier: "unknown".to_owned(),
        prompt: state.prompt().cloned(),
        idle_grace_remaining_secs: None,
    })
    .into_response()
}

/// `POST /api/v1/agent/nudge`
pub async fn agent_nudge(
    State(s): State<Arc<AppState>>,
    Json(req): Json<NudgeRequest>,
) -> impl IntoResponse {
    let encoder = match &s.nudge_encoder {
        Some(enc) => Arc::clone(enc),
        None => {
            return error_response(ErrorCode::NoDriver, "no agent driver configured")
                .into_response()
        }
    };

    let _guard = match s.write_lock.acquire_http() {
        Ok(g) => g,
        Err(code) => {
            return error_response(code, "write lock held by another client").into_response()
        }
    };

    let state = s.agent_state.read().await;
    let state_before = state.as_str().to_owned();

    let steps = encoder.encode(&req.message);
    for step in &steps {
        let _ = s
            .input_tx
            .send(InputEvent::Write(Bytes::from(step.bytes.clone())))
            .await;
        if let Some(delay) = step.delay_after {
            tokio::time::sleep(delay).await;
        }
    }

    Json(NudgeResponse {
        delivered: true,
        state_before: Some(state_before),
        reason: None,
    })
    .into_response()
}

/// `POST /api/v1/agent/respond`
pub async fn agent_respond(
    State(s): State<Arc<AppState>>,
    Json(req): Json<RespondRequest>,
) -> impl IntoResponse {
    let encoder = match &s.respond_encoder {
        Some(enc) => Arc::clone(enc),
        None => {
            return error_response(ErrorCode::NoDriver, "no agent driver configured")
                .into_response()
        }
    };

    let _guard = match s.write_lock.acquire_http() {
        Ok(g) => g,
        Err(code) => {
            return error_response(code, "write lock held by another client").into_response()
        }
    };

    let state = s.agent_state.read().await;
    let prompt = state.prompt();
    if prompt.is_none() {
        return error_response(ErrorCode::NoPrompt, "no prompt active").into_response();
    }

    let prompt_type = state.as_str().to_owned();

    let steps = match &*state {
        AgentState::PermissionPrompt { .. } => {
            encoder.encode_permission(req.accept.unwrap_or(false))
        }
        AgentState::PlanPrompt { .. } => {
            encoder.encode_plan(req.accept.unwrap_or(false), req.text.as_deref())
        }
        AgentState::AskUser { .. } => {
            encoder.encode_question(req.option.map(|o| o as u32), req.text.as_deref())
        }
        _ => {
            return error_response(ErrorCode::NoPrompt, "no prompt active").into_response();
        }
    };

    drop(state);

    for step in &steps {
        let _ = s
            .input_tx
            .send(InputEvent::Write(Bytes::from(step.bytes.clone())))
            .await;
        if let Some(delay) = step.delay_after {
            tokio::time::sleep(delay).await;
        }
    }

    Json(RespondResponse {
        delivered: true,
        prompt_type: Some(prompt_type),
        reason: None,
    })
    .into_response()
}

/// Map a signal name (e.g. "SIGINT", "SIGTERM") to its numeric value.
fn signal_from_name(name: &str) -> Option<i32> {
    let name = name.strip_prefix("SIG").unwrap_or(name);
    match name.to_uppercase().as_str() {
        "HUP" => Some(1),
        "INT" => Some(2),
        "QUIT" => Some(3),
        "TERM" => Some(15),
        "KILL" => Some(9),
        "USR1" => Some(10),
        "USR2" => Some(12),
        _ => None,
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
