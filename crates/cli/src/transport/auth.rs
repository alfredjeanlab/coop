// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ErrorCode;
use crate::transport::state::AppState;
use crate::transport::ErrorResponse;

/// Validate a Bearer token from HTTP headers.
///
/// Returns `Ok(())` when `expected` is `None` (auth disabled) or when the
/// header matches. Returns `Err(ErrorCode::Unauthorized)` otherwise.
pub fn validate_bearer(headers: &HeaderMap, expected: Option<&str>) -> Result<(), ErrorCode> {
    let expected = match expected {
        Some(tok) => tok,
        None => return Ok(()),
    };

    let header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(ErrorCode::Unauthorized)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(ErrorCode::Unauthorized)?;
    if token == expected {
        Ok(())
    } else {
        Err(ErrorCode::Unauthorized)
    }
}

/// Validate a token from a WebSocket upgrade query string (`?token=...`).
///
/// Returns `Ok(())` when `expected` is `None` (auth disabled) or the token
/// matches. Returns `Err(ErrorCode::Unauthorized)` otherwise.
pub fn validate_ws_query(query: &str, expected: Option<&str>) -> Result<(), ErrorCode> {
    let expected = match expected {
        Some(tok) => tok,
        None => return Ok(()),
    };

    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("token=") {
            if value == expected {
                return Ok(());
            }
        }
    }

    Err(ErrorCode::Unauthorized)
}

/// Validate a token from the WebSocket `Auth` message.
pub fn validate_ws_auth(token: &str, expected: Option<&str>) -> Result<(), ErrorCode> {
    match expected {
        None => Ok(()),
        Some(tok) if tok == token => Ok(()),
        Some(_) => Err(ErrorCode::Unauthorized),
    }
}

/// Axum middleware that enforces Bearer token authentication on all routes
/// except `/api/v1/health` and WebSocket upgrades (`/ws`).
///
/// When `auth_token` is `None` in `AppState`, all requests pass through.
pub async fn auth_layer(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = req.uri().path();

    // Health and WebSocket endpoints skip HTTP auth.
    // WebSocket auth is handled in the WS handler via query param or Auth message.
    if path == "/api/v1/health" || path == "/ws" {
        return next.run(req).await;
    }

    if let Err(code) = validate_bearer(req.headers(), state.auth_token.as_deref()) {
        let body = ErrorResponse {
            error: code.to_error_body("unauthorized"),
        };
        return (
            StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::UNAUTHORIZED),
            axum::Json(body),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
