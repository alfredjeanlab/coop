// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! gRPC transport implementing the `Coop` service defined in `coop.v1`.

use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use super::read_ring_combined;
use crate::driver::PromptContext;
use crate::error::ErrorCode;
use crate::event::{OutputEvent, StateChangeEvent};
use crate::start::StartConfig;
use crate::stop::StopConfig;
use crate::transport::handler::{
    compute_health, compute_status, error_message, extract_error_fields, handle_input, handle_keys,
    handle_nudge, handle_resize, handle_respond, handle_signal, TransportQuestionAnswer,
};
use crate::transport::state::AppState;

/// Generated protobuf types for the `coop.v1` package.
pub mod proto {
    tonic::include_proto!("coop.v1");
}

/// Convert a domain [`crate::screen::CursorPosition`] to proto.
pub fn cursor_to_proto(c: &crate::screen::CursorPosition) -> proto::CursorPosition {
    proto::CursorPosition { row: c.row as i32, col: c.col as i32 }
}

/// Convert a domain [`crate::screen::ScreenSnapshot`] to proto [`proto::ScreenSnapshot`].
pub fn screen_snapshot_to_proto(s: &crate::screen::ScreenSnapshot) -> proto::ScreenSnapshot {
    proto::ScreenSnapshot {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: Some(cursor_to_proto(&s.cursor)),
        seq: s.sequence,
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
        cursor: if include_cursor { Some(cursor_to_proto(&s.cursor)) } else { None },
        seq: s.sequence,
    }
}

/// Convert a domain [`PromptContext`] to proto.
pub fn prompt_to_proto(p: &PromptContext) -> proto::PromptContext {
    proto::PromptContext {
        r#type: p.kind.as_str().to_owned(),
        tool: p.tool.clone(),
        input: p.input.clone(),
        auth_url: p.auth_url.clone(),
        questions: p
            .questions
            .iter()
            .map(|q| proto::QuestionContext {
                question: q.question.clone(),
                options: q.options.clone(),
            })
            .collect(),
        question_current: p.question_current as u32,
        options: p.options.clone(),
        options_fallback: p.options_fallback,
        subtype: p.subtype.clone(),
        ready: p.ready,
    }
}

/// Convert a domain [`StateChangeEvent`] to proto [`proto::AgentStateEvent`].
pub fn state_change_to_proto(e: &StateChangeEvent) -> proto::AgentStateEvent {
    let (error_detail, error_category) = extract_error_fields(&e.next);
    let cause = if e.cause.is_empty() { None } else { Some(e.cause.clone()) };
    proto::AgentStateEvent {
        prev: e.prev.as_str().to_owned(),
        next: e.next.as_str().to_owned(),
        seq: e.seq,
        prompt: e.next.prompt().map(prompt_to_proto),
        error_detail,
        error_category,
        cause,
        last_message: e.last_message.clone(),
    }
}

/// Spawn a task that reads from a broadcast receiver, maps each event through
/// `map_fn`, and forwards the result into a gRPC response stream.
fn spawn_broadcast_stream<E, T, F>(rx: broadcast::Receiver<E>, map_fn: F) -> GrpcStream<T>
where
    E: Clone + Send + 'static,
    T: Send + 'static,
    F: Fn(E) -> Option<T> + Send + 'static,
{
    let (tx, receiver) = mpsc::channel(16);
    tokio::spawn(async move {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Some(item) = map_fn(event) {
                        if tx.send(Ok(item)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Box::pin(ReceiverStream::new(receiver))
}

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
    async fn get_health(
        &self,
        _request: Request<proto::GetHealthRequest>,
    ) -> Result<Response<proto::GetHealthResponse>, Status> {
        let h = compute_health(&self.state).await;
        Ok(Response::new(proto::GetHealthResponse {
            status: h.status,
            pid: h.pid,
            uptime_secs: h.uptime_secs,
            agent: h.agent,
            ws_clients: h.ws_clients,
            terminal_cols: h.terminal_cols as i32,
            terminal_rows: h.terminal_rows as i32,
            ready: h.ready,
        }))
    }

    async fn get_screen(
        &self,
        request: Request<proto::GetScreenRequest>,
    ) -> Result<Response<proto::GetScreenResponse>, Status> {
        let req = request.into_inner();
        let screen = self.state.terminal.screen.read().await;
        let snap = screen.snapshot();
        Ok(Response::new(screen_snapshot_to_response(&snap, req.include_cursor)))
    }

    async fn get_status(
        &self,
        _request: Request<proto::GetStatusRequest>,
    ) -> Result<Response<proto::GetStatusResponse>, Status> {
        let st = compute_status(&self.state).await;
        Ok(Response::new(proto::GetStatusResponse {
            state: st.state,
            pid: st.pid,
            uptime_secs: st.uptime_secs,
            exit_code: st.exit_code,
            screen_seq: st.screen_seq,
            bytes_read: st.bytes_read,
            bytes_written: st.bytes_written,
            ws_clients: st.ws_clients,
        }))
    }

    async fn send_input(
        &self,
        request: Request<proto::SendInputRequest>,
    ) -> Result<Response<proto::SendInputResponse>, Status> {
        let req = request.into_inner();
        let len = handle_input(&self.state, req.text, req.enter).await;
        Ok(Response::new(proto::SendInputResponse { bytes_written: len }))
    }

    async fn send_keys(
        &self,
        request: Request<proto::SendKeysRequest>,
    ) -> Result<Response<proto::SendKeysResponse>, Status> {
        let req = request.into_inner();
        let len = handle_keys(&self.state, &req.keys).await.map_err(|bad_key| {
            ErrorCode::BadRequest.to_grpc_status(format!("unknown key: {bad_key}"))
        })?;
        Ok(Response::new(proto::SendKeysResponse { bytes_written: len }))
    }

    async fn resize(
        &self,
        request: Request<proto::ResizeRequest>,
    ) -> Result<Response<proto::ResizeResponse>, Status> {
        let req = request.into_inner();
        let cols: u16 = req
            .cols
            .try_into()
            .map_err(|_| ErrorCode::BadRequest.to_grpc_status("cols must be a positive u16"))?;
        let rows: u16 = req
            .rows
            .try_into()
            .map_err(|_| ErrorCode::BadRequest.to_grpc_status("rows must be a positive u16"))?;
        handle_resize(&self.state, cols, rows)
            .await
            .map_err(|code| code.to_grpc_status("cols and rows must be positive"))?;
        Ok(Response::new(proto::ResizeResponse { cols: cols as i32, rows: rows as i32 }))
    }

    async fn send_signal(
        &self,
        request: Request<proto::SendSignalRequest>,
    ) -> Result<Response<proto::SendSignalResponse>, Status> {
        let req = request.into_inner();
        handle_signal(&self.state, &req.signal).await.map_err(|bad_signal| {
            ErrorCode::BadRequest.to_grpc_status(format!("unknown signal: {bad_signal}"))
        })?;
        Ok(Response::new(proto::SendSignalResponse { delivered: true }))
    }

    async fn get_agent_state(
        &self,
        _request: Request<proto::GetAgentStateRequest>,
    ) -> Result<Response<proto::GetAgentStateResponse>, Status> {
        let agent = self.state.driver.agent_state.read().await;
        let screen = self.state.terminal.screen.read().await;

        Ok(Response::new(proto::GetAgentStateResponse {
            agent: self.state.config.agent.to_string(),
            state: agent.as_str().to_owned(),
            since_seq: self.state.driver.state_seq.load(Ordering::Acquire),
            screen_seq: screen.seq(),
            detection_tier: self.state.driver.detection_tier_str(),
            prompt: agent.prompt().map(prompt_to_proto),
            error_detail: self.state.driver.error.read().await.as_ref().map(|e| e.detail.clone()),
            error_category: self
                .state
                .driver
                .error
                .read()
                .await
                .as_ref()
                .map(|e| e.category.as_str().to_owned()),
            last_message: self.state.driver.last_message.read().await.clone(),
        }))
    }

    async fn nudge(
        &self,
        request: Request<proto::NudgeRequest>,
    ) -> Result<Response<proto::NudgeResponse>, Status> {
        let req = request.into_inner();
        match handle_nudge(&self.state, &req.message).await {
            Ok(outcome) => Ok(Response::new(proto::NudgeResponse {
                delivered: outcome.delivered,
                state_before: outcome.state_before,
                reason: outcome.reason,
            })),
            Err(code) => Err(code.to_grpc_status(error_message(code))),
        }
    }

    async fn respond(
        &self,
        request: Request<proto::RespondRequest>,
    ) -> Result<Response<proto::RespondResponse>, Status> {
        let req = request.into_inner();
        let answers: Vec<TransportQuestionAnswer> = req
            .answers
            .iter()
            .map(|a| TransportQuestionAnswer { option: a.option, text: a.text.clone() })
            .collect();
        match handle_respond(&self.state, req.accept, req.option, req.text.as_deref(), &answers)
            .await
        {
            Ok(outcome) => Ok(Response::new(proto::RespondResponse {
                delivered: outcome.delivered,
                prompt_type: outcome.prompt_type,
                reason: outcome.reason,
            })),
            Err(code) => Err(code.to_grpc_status(error_message(code))),
        }
    }

    type StreamOutputStream = GrpcStream<proto::OutputChunk>;

    async fn stream_output(
        &self,
        request: Request<proto::StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let from_offset = request.into_inner().from_offset;
        let (tx, rx) = mpsc::channel(64);

        // Replay buffered data from ring buffer
        {
            let ring = self.state.terminal.ring.read().await;
            let data = read_ring_combined(&ring, from_offset);
            if !data.is_empty() {
                let _ = tx.send(Ok(proto::OutputChunk { data, offset: from_offset })).await;
            }
        }

        // Subscribe to live output
        let mut output_rx = self.state.channels.output_tx.subscribe();
        let terminal = Arc::clone(&self.state.terminal);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::Raw(data)) => {
                        let r = terminal.ring.read().await;
                        let offset = r.total_written() - data.len() as u64;
                        drop(r);
                        let chunk = proto::OutputChunk { data: data.to_vec(), offset };
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

    type StreamScreenStream = GrpcStream<proto::ScreenSnapshot>;

    async fn stream_screen(
        &self,
        _request: Request<proto::StreamScreenRequest>,
    ) -> Result<Response<Self::StreamScreenStream>, Status> {
        let (tx, rx) = mpsc::channel(16);
        let mut output_rx = self.state.channels.output_tx.subscribe();
        let terminal = Arc::clone(&self.state.terminal);

        tokio::spawn(async move {
            loop {
                match output_rx.recv().await {
                    Ok(OutputEvent::ScreenUpdate { .. }) => {
                        let s = terminal.screen.read().await;
                        let snap = s.snapshot();
                        drop(s);
                        let proto_snap = screen_snapshot_to_proto(&snap);
                        if tx.send(Ok(proto_snap)).await.is_err() {
                            break;
                        }
                    }
                    Ok(OutputEvent::Raw(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    type StreamStateStream = GrpcStream<proto::AgentStateEvent>;

    async fn stream_state(
        &self,
        _request: Request<proto::StreamStateRequest>,
    ) -> Result<Response<Self::StreamStateStream>, Status> {
        let state_rx = self.state.channels.state_tx.subscribe();
        let stream = spawn_broadcast_stream(state_rx, |event| Some(state_change_to_proto(&event)));
        Ok(Response::new(stream))
    }

    async fn shutdown(
        &self,
        _request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<proto::ShutdownResponse>, Status> {
        self.state.lifecycle.shutdown.cancel();
        Ok(Response::new(proto::ShutdownResponse { accepted: true }))
    }

    async fn resolve_stop(
        &self,
        request: Request<proto::ResolveStopRequest>,
    ) -> Result<Response<proto::ResolveStopResponse>, Status> {
        let req = request.into_inner();
        let body: serde_json::Value = serde_json::from_str(&req.body_json)
            .map_err(|e| Status::invalid_argument(format!("invalid JSON: {e}")))?;
        let stop = &self.state.stop;
        *stop.signal_body.write().await = Some(body);
        stop.signaled.store(true, std::sync::atomic::Ordering::Release);
        Ok(Response::new(proto::ResolveStopResponse { accepted: true }))
    }

    async fn put_stop_config(
        &self,
        request: Request<proto::PutStopConfigRequest>,
    ) -> Result<Response<proto::PutStopConfigResponse>, Status> {
        let req = request.into_inner();
        let new_config: StopConfig = serde_json::from_str(&req.config_json)
            .map_err(|e| Status::invalid_argument(format!("invalid config JSON: {e}")))?;
        *self.state.stop.config.write().await = new_config;
        Ok(Response::new(proto::PutStopConfigResponse { updated: true }))
    }

    async fn get_stop_config(
        &self,
        _request: Request<proto::GetStopConfigRequest>,
    ) -> Result<Response<proto::GetStopConfigResponse>, Status> {
        let config = self.state.stop.config.read().await;
        let json = serde_json::to_string(&*config)
            .map_err(|e| Status::internal(format!("serialize error: {e}")))?;
        Ok(Response::new(proto::GetStopConfigResponse { config_json: json }))
    }

    type StreamStopEventsStream = GrpcStream<proto::StopEvent>;

    async fn stream_stop_events(
        &self,
        _request: Request<proto::StreamStopEventsRequest>,
    ) -> Result<Response<Self::StreamStopEventsStream>, Status> {
        let stop_rx = self.state.stop.stop_tx.subscribe();
        let stream = spawn_broadcast_stream(stop_rx, |event| {
            Some(proto::StopEvent {
                stop_type: event.stop_type.as_str().to_owned(),
                signal_json: event.signal.map(|v| v.to_string()),
                error_detail: event.error_detail,
                seq: event.seq,
            })
        });
        Ok(Response::new(stream))
    }

    async fn put_start_config(
        &self,
        request: Request<proto::PutStartConfigRequest>,
    ) -> Result<Response<proto::PutStartConfigResponse>, Status> {
        let req = request.into_inner();
        let new_config: StartConfig = serde_json::from_str(&req.config_json)
            .map_err(|e| Status::invalid_argument(format!("invalid config JSON: {e}")))?;
        *self.state.start.config.write().await = new_config;
        Ok(Response::new(proto::PutStartConfigResponse { updated: true }))
    }

    async fn get_start_config(
        &self,
        _request: Request<proto::GetStartConfigRequest>,
    ) -> Result<Response<proto::GetStartConfigResponse>, Status> {
        let config = self.state.start.config.read().await;
        let json = serde_json::to_string(&*config)
            .map_err(|e| Status::internal(format!("serialize error: {e}")))?;
        Ok(Response::new(proto::GetStartConfigResponse { config_json: json }))
    }

    type StreamStartEventsStream = GrpcStream<proto::StartEvent>;

    async fn stream_start_events(
        &self,
        _request: Request<proto::StreamStartEventsRequest>,
    ) -> Result<Response<Self::StreamStartEventsStream>, Status> {
        let start_rx = self.state.start.start_tx.subscribe();
        let stream = spawn_broadcast_stream(start_rx, |event| {
            Some(proto::StartEvent {
                source: event.source,
                session_id: event.session_id,
                injected: event.injected,
                seq: event.seq,
            })
        });
        Ok(Response::new(stream))
    }
}

#[cfg(test)]
#[path = "grpc_tests.rs"]
mod tests;
