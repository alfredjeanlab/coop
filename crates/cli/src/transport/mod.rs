// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! Transport layer: shared state, helpers, and API contract types.

pub mod auth;
pub mod grpc;
pub mod http;
pub mod ws;

use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, NudgeEncoder, PromptContext, RespondEncoder};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

/// Shared state between the session loop and transport layers.
pub struct AppState {
    pub screen: Arc<RwLock<Screen>>,
    pub ring: Arc<RwLock<RingBuffer>>,
    pub input_tx: mpsc::Sender<InputEvent>,
    pub output_tx: broadcast::Sender<OutputEvent>,
    pub state_tx: broadcast::Sender<StateChangeEvent>,
    pub agent_state: Arc<RwLock<AgentState>>,
    pub agent_type: String,
    pub pid: Arc<RwLock<Option<u32>>>,
    pub start_time: Instant,
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    pub ws_clients: AtomicI32,
    pub bytes_written: AtomicU64,
    pub shutdown: CancellationToken,
    pub auth_token: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers (shared between HTTP and gRPC)
// ---------------------------------------------------------------------------

/// Translate a named key to its terminal escape sequence.
pub fn encode_key(name: &str) -> Option<Vec<u8>> {
    let bytes: &[u8] = match name.to_lowercase().as_str() {
        "enter" | "return" => b"\r",
        "tab" => b"\t",
        "escape" | "esc" => b"\x1b",
        "backspace" => b"\x7f",
        "delete" | "del" => b"\x1b[3~",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" | "page_up" => b"\x1b[5~",
        "pagedown" | "page_down" => b"\x1b[6~",
        "insert" => b"\x1b[2~",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        "space" => b" ",
        "ctrl-a" => b"\x01",
        "ctrl-b" => b"\x02",
        "ctrl-c" => b"\x03",
        "ctrl-d" => b"\x04",
        "ctrl-e" => b"\x05",
        "ctrl-f" => b"\x06",
        "ctrl-g" => b"\x07",
        "ctrl-h" => b"\x08",
        "ctrl-k" => b"\x0b",
        "ctrl-l" => b"\x0c",
        "ctrl-n" => b"\x0e",
        "ctrl-o" => b"\x0f",
        "ctrl-p" => b"\x10",
        "ctrl-r" => b"\x12",
        "ctrl-s" => b"\x13",
        "ctrl-t" => b"\x14",
        "ctrl-u" => b"\x15",
        "ctrl-w" => b"\x17",
        "ctrl-z" => b"\x1a",
        _ => return None,
    };
    Some(bytes.to_vec())
}

/// Parse a signal name (e.g. "SIGINT", "INT", "2") into a signal number.
pub fn parse_signal(name: &str) -> Option<i32> {
    let upper = name.to_uppercase();
    let bare: &str = match upper.strip_prefix("SIG") {
        Some(s) => s,
        None => &upper,
    };

    match bare {
        "HUP" | "1" => Some(1),
        "INT" | "2" => Some(2),
        "QUIT" | "3" => Some(3),
        "KILL" | "9" => Some(9),
        "TERM" | "15" => Some(15),
        "USR1" | "10" => Some(10),
        "USR2" | "12" => Some(12),
        "CONT" | "18" => Some(18),
        "STOP" | "19" => Some(19),
        "TSTP" | "20" => Some(20),
        "WINCH" | "28" => Some(28),
        _ => None,
    }
}

/// Convert a domain [`PromptContext`] to an HTTP [`http::AgentStateResponse`]-compatible prompt.
pub fn prompt_to_http(p: &PromptContext) -> crate::driver::PromptContext {
    p.clone()
}

// ---------------------------------------------------------------------------
// Error response types
// ---------------------------------------------------------------------------

/// Top-level error response envelope shared across HTTP and WebSocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

/// Error body containing a machine-readable code and human-readable message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ErrorBody {
    /// Create an `ErrorBody` from a code string and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl ErrorCode {
    /// Convert this error code into a transport [`ErrorBody`].
    pub fn to_error_body(&self, message: impl Into<String>) -> ErrorBody {
        ErrorBody {
            code: self.as_str().to_owned(),
            message: message.into(),
        }
    }

    /// Convert this error code into an axum JSON error response.
    pub fn to_http_response(
        &self,
        message: impl Into<String>,
    ) -> (axum::http::StatusCode, axum::Json<ErrorResponse>) {
        let status = axum::http::StatusCode::from_u16(self.http_status())
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        let body = ErrorResponse {
            error: self.to_error_body(message),
        };
        (status, axum::Json(body))
    }
}
