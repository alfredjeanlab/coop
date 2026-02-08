// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! API contract types and server implementation for HTTP and WebSocket transports.

pub mod auth;
pub mod grpc;
pub mod http;
pub mod state;
pub mod ws;

pub use state::AppState;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

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

/// Build a JSON error response from an `ErrorCode` and message.
pub fn error_response(
    code: ErrorCode,
    message: impl Into<String>,
) -> (StatusCode, Json<ErrorResponse>) {
    let status =
        StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = ErrorResponse {
        error: code.to_error_body(message),
    };
    (status, Json(body))
}

// ---------------------------------------------------------------------------
// Helpers (shared between HTTP, WebSocket, and gRPC)
// ---------------------------------------------------------------------------

/// Convert named key sequences to raw bytes for PTY input.
pub fn keys_to_bytes(keys: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    for key in keys {
        match key.as_str() {
            "Enter" | "Return" => out.push(b'\r'),
            "Tab" => out.push(b'\t'),
            "Escape" | "Esc" => out.push(0x1b),
            "Backspace" => out.push(0x7f),
            "Delete" => out.extend_from_slice(b"\x1b[3~"),
            "Up" => out.extend_from_slice(b"\x1b[A"),
            "Down" => out.extend_from_slice(b"\x1b[B"),
            "Right" => out.extend_from_slice(b"\x1b[C"),
            "Left" => out.extend_from_slice(b"\x1b[D"),
            "Home" => out.extend_from_slice(b"\x1b[H"),
            "End" => out.extend_from_slice(b"\x1b[F"),
            "PageUp" => out.extend_from_slice(b"\x1b[5~"),
            "PageDown" => out.extend_from_slice(b"\x1b[6~"),
            "Space" => out.push(b' '),
            s if s.starts_with("Ctrl-") || s.starts_with("ctrl-") => {
                if let Some(ch) = s.chars().last() {
                    let ctrl = (ch.to_ascii_uppercase() as u8).wrapping_sub(b'@');
                    out.push(ctrl);
                }
            }
            other => out.extend_from_slice(other.as_bytes()),
        }
    }
    out
}

/// Translate a named key to its terminal escape sequence.
///
/// Returns `None` for unrecognised key names (used by gRPC to surface errors).
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

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the axum `Router` with all HTTP and WebSocket routes.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(http::health))
        .route("/api/v1/screen", get(http::screen))
        .route("/api/v1/screen/text", get(http::screen_text))
        .route("/api/v1/output", get(http::output))
        .route("/api/v1/status", get(http::status))
        .route("/api/v1/input", post(http::input))
        .route("/api/v1/input/keys", post(http::input_keys))
        .route("/api/v1/resize", post(http::resize))
        .route("/api/v1/signal", post(http::signal))
        .route("/api/v1/agent/state", get(http::agent_state))
        .route("/api/v1/agent/nudge", post(http::agent_nudge))
        .route("/api/v1/agent/respond", post(http::agent_respond))
        .route("/ws", get(ws::ws_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_layer,
        ))
        .with_state(state)
}

/// Build a minimal health-only router (for `--health-port`).
pub fn build_health_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/health", get(http::health))
        .with_state(state)
}

/// Bind TCP and/or Unix socket listeners and serve the router.
pub async fn serve(
    state: Arc<AppState>,
    tcp: Option<SocketAddr>,
    _unix: Option<PathBuf>,
) -> anyhow::Result<()> {
    let router = build_router(state);

    if let Some(addr) = tcp {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, router).await?;
    }

    Ok(())
}
