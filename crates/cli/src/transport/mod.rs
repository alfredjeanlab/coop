// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! API contract types for HTTP and WebSocket transports.
//!
//! These are types-only definitions — no server logic or handler
//! implementations. The types are sufficient for both server stubs
//! (Epic 05, 14) and client code to be written against.

pub mod http;
pub mod ws;

use serde::{Deserialize, Serialize};

use crate::error::ErrorCode;

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
