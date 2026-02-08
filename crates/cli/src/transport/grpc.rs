// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! gRPC transport implementing the `Coop` service defined in `coop.v1`.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::{encode_key, parse_signal};
use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::transport::state::AppState;

/// Generated protobuf types for the `coop.v1` package.
pub mod proto {
    tonic::include_proto!("coop.v1");
}

// ---------------------------------------------------------------------------
// Type conversions: domain → proto
// ---------------------------------------------------------------------------

/// Convert a domain [`crate::screen::CursorPosition`] to proto.
pub fn cursor_to_proto(c: &crate::screen::CursorPosition) -> proto::CursorPosition {
    proto::CursorPosition {
        row: c.row as i32,
        col: c.col as i32,
    }
}

/// Convert a domain [`crate::screen::ScreenSnapshot`] to proto [`proto::ScreenSnapshot`].
pub fn screen_snapshot_to_proto(s: &crate::screen::ScreenSnapshot) -> proto::ScreenSnapshot {
    proto::ScreenSnapshot {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: Some(cursor_to_proto(&s.cursor)),
        sequence: s.sequence,
    }
}

/// Convert a domain [`crate::screen::ScreenSnapshot`] to a [`proto::GetScreenResponse`],
/// optionally omitting the cursor.
pub fn screen_snapshot_to_response(
    s: &crate::screen::ScreenSnapshot,
    include_cursor: bool,
) -> proto::GetScreenResponse {
    proto::GetScreenResponse {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: if include_cursor {
            Some(cursor_to_proto(&s.cursor))
        } else {
            None
        },
        sequence: s.sequence,
    }
}

/// Convert a domain [`PromptContext`] to proto.
pub fn prompt_to_proto(p: &PromptContext) -> proto::PromptContext {
    proto::PromptContext {
        r#type: p.prompt_type.clone(),
        tool: p.tool.clone(),
        input_preview: p.input_preview.clone(),
        question: p.question.clone(),
        options: p.options.clone(),
        summary: p.summary.clone(),
        screen_lines: p.screen_lines.clone(),
    }
}

/// Convert a domain [`StateChangeEvent`] to proto [`proto::AgentStateEvent`].
pub fn state_change_to_proto(e: &StateChangeEvent) -> proto::AgentStateEvent {
    proto::AgentStateEvent {
        prev: e.prev.as_str().to_owned(),
        next: e.next.as_str().to_owned(),
        seq: e.seq,
        prompt: e.next.prompt().map(prompt_to_proto),
    }
}

// ---------------------------------------------------------------------------
// gRPC service
// ---------------------------------------------------------------------------

/// gRPC implementation of the `coop.v1.Coop` service.
pub struct CoopGrpc {
    state: Arc<AppState>,
}

impl CoopGrpc {
    /// Create a new gRPC service backed by the given shared state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    /// Build a [`tonic`] router for this service.
    pub fn into_router(self) -> tonic::transport::server::Router {
        tonic::transport::Server::builder().add_service(proto::coop_server::CoopServer::new(self))
    }
}

type GrpcStream<T> = Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl proto::coop_server::Coop for CoopGrpc {
    // -----------------------------------------------------------------------
    // 1. GetHealth
    // -----------------------------------------------------------------------
    async fn get_health(
        &self,
        _request: Request<proto::GetHealthRequest>,
    ) -> Result<Response<proto::GetHealthResponse>, Status> {
        let pid = self.state.child_pid.load(Ordering::Relaxed);
        let uptime = self.state.started_at.elapsed().as_secs() as i64;
        let ws = self.state.ws_client_count.load(Ordering::Relaxed);

        Ok(Response::new(proto::GetHealthResponse {
            status: "ok".to_owned(),
            pid: if pid == 0 { None } else { Some(pid as i32) },
            uptime_secs: uptime,
            agent_type: self.state.agent_type.clone(),
            ws_clients: ws,
        }))
    }

    // -----------------------------------------------------------------------
    // 2. GetScreen
    // -----------------------------------------------------------------------
    async fn get_screen(
        &self,
        request: Request<proto::GetScreenRequest>,
    ) -> Result<Response<proto::GetScreenResponse>, Status> {
        let req = request.into_inner();
        let screen = self.state.screen.read().await;
        let snap = screen.snapshot();
        Ok(Response::new(screen_snapshot_to_response(
            &snap,
            req.include_cursor,
        )))
    }

    // -----------------------------------------------------------------------
    // 3. GetStatus
    // -----------------------------------------------------------------------
    async fn get_status(
        &self,
        _request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<proto::GetStatusResponse>, Status> {
        let pid = self.state.child_pid.load(Ordering::Relaxed);
        let agent = self.state.agent_state.read().await;
        let screen = self.state.screen.read().await;
        let ring = self.state.ring.read().await;
        let uptime = self.state.started_at.elapsed().as_secs() as i64;
        let exit = self.state.exit_status.read().await;

        let exit_code = exit.as_ref().and_then(|e| e.code);

        let bw = self.state.bytes_written.load(Ordering::Relaxed);
        let ws = self.state.ws_client_count.load(Ordering::Relaxed);

        Ok(Response::new(proto::GetStatusResponse {
            state: agent.as_str().to_owned(),
            pid: if pid == 0 { None } else { Some(pid as i32) },
            uptime_secs: uptime,
            exit_code,
            screen_seq: screen.seq(),
            bytes_read: ring.total_written(),
            bytes_written: bw,
            ws_clients: ws,
        }))
    }

    // -----------------------------------------------------------------------
    // 4. SendInput
    // -----------------------------------------------------------------------
    async fn send_input(
        &self,
        request: Request<proto::SendInputRequest>,
    ) -> Result<Response<proto::SendInputResponse>, Status> {
        let req = request.into_inner();
        let mut payload = req.text.into_bytes();
        if req.enter {
            payload.push(b'\r');
        }
        let len = payload.len() as i32;
        self.state
            .input_tx
            .send(InputEvent::Write(Bytes::from(payload)))
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::SendInputResponse {
            bytes_written: len,
        }))
    }

    // -----------------------------------------------------------------------
    // 5. SendKeys
    // -----------------------------------------------------------------------
    async fn send_keys(
        &self,
        request: Request<proto::SendKeysRequest>,
    ) -> Result<Response<proto::SendKeysResponse>, Status> {
        let req = request.into_inner();
        let mut total = 0i32;
        for key in &req.keys {
            let encoded = encode_key(key).ok_or_else(|| {
                ErrorCode::BadRequest.to_grpc_status(format!("unknown key: {key}"))
            })?;
            total += encoded.len() as i32;
            self.state
                .input_tx
                .send(InputEvent::Write(Bytes::from(encoded)))
                .await
                .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
        }
        Ok(Response::new(proto::SendKeysResponse {
            bytes_written: total,
        }))
    }

    // -----------------------------------------------------------------------
    // 6. Resize
    // -----------------------------------------------------------------------
    async fn resize(
        &self,
        request: Request<proto::ResizeRequest>,
    ) -> Result<Response<proto::ResizeResponse>, Status> {
        let req = request.into_inner();
        if req.cols <= 0 || req.rows <= 0 {
            return Err(ErrorCode::BadRequest.to_grpc_status("cols and rows must be positive"));
        }
        let cols = req.cols as u16;
        let rows = req.rows as u16;
        self.state
            .input_tx
            .send(InputEvent::Resize { cols, rows })
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::ResizeResponse {
            cols: cols as i32,
            rows: rows as i32,
        }))
    }

    // -----------------------------------------------------------------------
    // 7. SendSignal
    // -----------------------------------------------------------------------
    async fn send_signal(
        &self,
        request: Request<proto::SendSignalRequest>,
    ) -> Result<Response<proto::SendSignalResponse>, Status> {
        let req = request.into_inner();
        let signum = parse_signal(&req.signal).ok_or_else(|| {
            ErrorCode::BadRequest.to_grpc_status(format!("unknown signal: {}", req.signal))
        })?;
        self.state
            .input_tx
            .send(InputEvent::Signal(signum))
            .await
            .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
        Ok(Response::new(proto::SendSignalResponse { delivered: true }))
    }

    // -----------------------------------------------------------------------
    // 8. GetAgentState
    // -----------------------------------------------------------------------
    async fn get_agent_state(
        &self,
        _request: Request<proto::GetAgentStateRequest>,
    ) -> Result<Response<proto::GetAgentStateResponse>, Status> {
        let agent = self.state.agent_state.read().await;
        let screen = self.state.screen.read().await;

        let since_seq = self.state.state_seq.load(Ordering::Relaxed);
        let tier = self.state.detection_tier.load(Ordering::Relaxed);
        let detection_tier = if tier == u8::MAX {
            "none".to_owned()
        } else {
            tier.to_string()
        };

        let idle_grace_remaining_secs = {
            let deadline = self
                .state
                .idle_grace_deadline
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            deadline.map(|dl| {
                let now = std::time::Instant::now();
                if now < dl {
                    (dl - now).as_secs_f32()
                } else {
                    0.0
                }
            })
        };

        Ok(Response::new(proto::GetAgentStateResponse {
            agent_type: self.state.agent_type.clone(),
            state: agent.as_str().to_owned(),
            since_seq,
            screen_seq: screen.seq(),
            detection_tier,
            prompt: agent.prompt().map(prompt_to_proto),
            idle_grace_remaining_secs,
        }))
    }

    // -----------------------------------------------------------------------
    // 9. Nudge
    // -----------------------------------------------------------------------
    async fn nudge(
        &self,
        request: Request<proto::NudgeRequest>,
    ) -> Result<Response<proto::NudgeResponse>, Status> {
        let req = request.into_inner();

        let encoder = self
            .state
            .nudge_encoder
            .as_ref()
            .ok_or_else(|| ErrorCode::NoDriver.to_grpc_status("no nudge encoder configured"))?;

        let _guard = self
            .state
            .write_lock
            .acquire_http()
            .map_err(|code| code.to_grpc_status("write lock held by another client"))?;

        let agent = self.state.agent_state.read().await;
        let state_before = agent.as_str().to_owned();

        match &*agent {
            AgentState::WaitingForInput => {}
            other => {
                return Ok(Response::new(proto::NudgeResponse {
                    delivered: false,
                    state_before,
                    reason: Some(format!("agent is {}", other.as_str())),
                }));
            }
        }

        let steps = encoder.encode(&req.message);
        // Release the read lock before writing
        drop(agent);

        for step in steps {
            self.state
                .input_tx
                .send(InputEvent::Write(Bytes::from(step.bytes)))
                .await
                .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
            if let Some(delay) = step.delay_after {
                tokio::time::sleep(delay).await;
            }
        }

        Ok(Response::new(proto::NudgeResponse {
            delivered: true,
            state_before,
            reason: None,
        }))
    }

    // -----------------------------------------------------------------------
    // 10. Respond
    // -----------------------------------------------------------------------
    async fn respond(
        &self,
        request: Request<proto::RespondRequest>,
    ) -> Result<Response<proto::RespondResponse>, Status> {
        let req = request.into_inner();

        let encoder =
            self.state.respond_encoder.as_ref().ok_or_else(|| {
                ErrorCode::NoDriver.to_grpc_status("no respond encoder configured")
            })?;

        let _guard = self
            .state
            .write_lock
            .acquire_http()
            .map_err(|code| code.to_grpc_status("write lock held by another client"))?;

        let agent = self.state.agent_state.read().await;

        let steps = match &*agent {
            AgentState::PermissionPrompt { .. } => {
                let accept = req.accept.unwrap_or(false);
                encoder.encode_permission(accept)
            }
            AgentState::PlanPrompt { .. } => {
                let accept = req.accept.unwrap_or(false);
                encoder.encode_plan(accept, req.text.as_deref())
            }
            AgentState::AskUser { .. } => {
                encoder.encode_question(req.option.map(|o| o as u32), req.text.as_deref())
            }
            other => {
                return Err(ErrorCode::NoPrompt
                    .to_grpc_status(format!("agent is {} (no active prompt)", other.as_str())));
            }
        };

        let prompt_type = agent.as_str().to_owned();
        // Release the read lock before writing
        drop(agent);

        for step in steps {
            self.state
                .input_tx
                .send(InputEvent::Write(Bytes::from(step.bytes)))
                .await
                .map_err(|_| ErrorCode::WriterBusy.to_grpc_status("input channel closed"))?;
            if let Some(delay) = step.delay_after {
                tokio::time::sleep(delay).await;
            }
        }

        Ok(Response::new(proto::RespondResponse {
            delivered: true,
            prompt_type,
            reason: None,
        }))
    }

    // -----------------------------------------------------------------------
    // 11. StreamOutput
    // -----------------------------------------------------------------------
    type StreamOutputStream = GrpcStream<proto::OutputChunk>;

    async fn stream_output(
        &self,
        request: Request<proto::StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let from_offset = request.into_inner().from_offset;
        let (tx, rx) = mpsc::channel(64);

        // Replay buffered data from ring buffer
        {
            let ring = self.state.ring.read().await;
            if let Some((a, b)) = ring.read_from(from_offset) {
                let mut data = Vec::with_capacity(a.len() + b.len());
                data.extend_from_slice(a);
                data.extend_from_slice(b);
                if !data.is_empty() {
                    let _ = tx
                        .send(Ok(proto::OutputChunk {
                            data,
                            offset: from_offset,
                        }))
                        .await;
                }
            }
        }

        // Subscribe to live output
        let mut output_rx = self.state.output_tx.subscribe();
        let ring = Arc::clone(&self.state.ring);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::Raw(data)) => {
                        let r = ring.read().await;
                        let offset = r.total_written() - data.len() as u64;
                        drop(r);
                        let chunk = proto::OutputChunk {
                            data: data.to_vec(),
                            offset,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Ok(OutputEvent::ScreenUpdate { .. }) => {
                        // Skip screen-update events in raw output stream
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Skip missed messages
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // -----------------------------------------------------------------------
    // 12. StreamScreen
    // -----------------------------------------------------------------------
    type StreamScreenStream = GrpcStream<proto::ScreenSnapshot>;

    async fn stream_screen(
        &self,
        _request: Request<proto::StreamScreenRequest>,
    ) -> Result<Response<Self::StreamScreenStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut output_rx = self.state.output_tx.subscribe();
        let screen = Arc::clone(&self.state.screen);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::ScreenUpdate { .. }) => {
                        let s = screen.read().await;
                        let snap = s.snapshot();
                        drop(s);
                        let proto_snap = screen_snapshot_to_proto(&snap);
                        if tx.send(Ok(proto_snap)).await.is_err() {
                            break;
                        }
                    }
                    Ok(OutputEvent::Raw(_)) => {
                        // Skip raw events in screen stream
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    // -----------------------------------------------------------------------
    // 13. StreamState
    // -----------------------------------------------------------------------
    type StreamStateStream = GrpcStream<proto::AgentStateEvent>;

    async fn stream_state(
        &self,
        _request: Request<proto::StreamStateRequest>,
    ) -> Result<Response<Self::StreamStateStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut state_rx = self.state.state_tx.subscribe();

        tokio::spawn(async move {
            loop {
                match state_rx.recv().await {
                    Ok(event) => {
                        let proto_event = state_change_to_proto(&event);
                        if tx.send(Ok(proto_event)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

#[cfg(test)]
#[path = "grpc_tests.rs"]
mod tests;
