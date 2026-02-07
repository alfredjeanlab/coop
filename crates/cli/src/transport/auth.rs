// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! Bearer token auth middleware for axum HTTP transport.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

/// Axum middleware that checks `Authorization: Bearer <TOKEN>`.
///
/// When no token is configured (pass-through mode), all requests are allowed.
pub async fn auth_middleware(
    axum::extract::State(token): axum::extract::State<Option<Arc<str>>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(ref expected) = token else {
        return Ok(next.run(request).await);
    };

    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match header {
        Some(value)
            if value
                .strip_prefix("Bearer ")
                .is_some_and(|t| t == expected.as_ref()) =>
        {
            Ok(next.run(request).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
