// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! API contract types and server implementation for HTTP and WebSocket transports.

pub mod auth;
pub mod grpc;
pub mod http;
pub mod state;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::middleware;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;
use state::AppState;

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
