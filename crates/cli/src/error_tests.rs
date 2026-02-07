// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use super::*;

#[yare::parameterized(
    not_ready = { ErrorCode::NotReady, tonic::Code::Unavailable },
    exited = { ErrorCode::Exited, tonic::Code::NotFound },
    writer_busy = { ErrorCode::WriterBusy, tonic::Code::ResourceExhausted },
    unauthorized = { ErrorCode::Unauthorized, tonic::Code::Unauthenticated },
    bad_request = { ErrorCode::BadRequest, tonic::Code::InvalidArgument },
    no_driver = { ErrorCode::NoDriver, tonic::Code::Unimplemented },
    agent_busy = { ErrorCode::AgentBusy, tonic::Code::FailedPrecondition },
    no_prompt = { ErrorCode::NoPrompt, tonic::Code::FailedPrecondition },
    internal = { ErrorCode::Internal, tonic::Code::Internal },
)]
fn to_grpc_status(error_code: ErrorCode, expected: tonic::Code) {
    let status = error_code.to_grpc_status("test message");
    assert_eq!(status.code(), expected);
    assert_eq!(status.message(), "test message");
}
