// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! HTTP request and response types for the coop REST API.
//!
//! All 14 routes are covered. Types use `String` for state fields to match
//! the wire format (e.g. `"working"`, `"permission_prompt"`). Prompt context
//! reuses [`crate::driver::PromptContext`] directly.

use serde::{Deserialize, Serialize};

use crate::driver::PromptContext;
use crate::screen::CursorPosition;

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
