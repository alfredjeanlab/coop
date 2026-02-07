// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use serde::{Deserialize, Serialize};
use std::fmt;

/// Unified error codes shared across HTTP, WebSocket, and gRPC transports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    NotReady,
    Exited,
    WriterBusy,
    Unauthorized,
    BadRequest,
    NoDriver,
    AgentBusy,
    NoPrompt,
    Internal,
}

impl ErrorCode {
    pub fn http_status(&self) -> u16 {
        match self {
            Self::NotReady => 503,
            Self::Exited => 410,
            Self::WriterBusy => 409,
            Self::Unauthorized => 401,
            Self::BadRequest => 400,
            Self::NoDriver => 404,
            Self::AgentBusy => 409,
            Self::NoPrompt => 409,
            Self::Internal => 500,
        }
    }

    pub fn grpc_code(&self) -> &'static str {
        match self {
            Self::NotReady => "UNAVAILABLE",
            Self::Exited => "NOT_FOUND",
            Self::WriterBusy => "RESOURCE_EXHAUSTED",
            Self::Unauthorized => "UNAUTHENTICATED",
            Self::BadRequest => "INVALID_ARGUMENT",
            Self::NoDriver => "UNIMPLEMENTED",
            Self::AgentBusy => "FAILED_PRECONDITION",
            Self::NoPrompt => "FAILED_PRECONDITION",
            Self::Internal => "INTERNAL",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotReady => "NOT_READY",
            Self::Exited => "EXITED",
            Self::WriterBusy => "WRITER_BUSY",
            Self::Unauthorized => "UNAUTHORIZED",
            Self::BadRequest => "BAD_REQUEST",
            Self::NoDriver => "NO_DRIVER",
            Self::AgentBusy => "AGENT_BUSY",
            Self::NoPrompt => "NO_PROMPT",
            Self::Internal => "INTERNAL",
        }
    }
}

impl ErrorCode {
    /// Convert this error code into a [`tonic::Status`] with the given message.
    pub fn to_grpc_status(&self, message: impl Into<String>) -> tonic::Status {
        let code = match self {
            Self::NotReady => tonic::Code::Unavailable,
            Self::Exited => tonic::Code::NotFound,
            Self::WriterBusy => tonic::Code::ResourceExhausted,
            Self::Unauthorized => tonic::Code::Unauthenticated,
            Self::BadRequest => tonic::Code::InvalidArgument,
            Self::NoDriver => tonic::Code::Unimplemented,
            Self::AgentBusy => tonic::Code::FailedPrecondition,
            Self::NoPrompt => tonic::Code::FailedPrecondition,
            Self::Internal => tonic::Code::Internal,
        };
        tonic::Status::new(code, message)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
