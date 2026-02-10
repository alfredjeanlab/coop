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
use crate::event::{OutputEvent, TransitionEvent};
use crate::start::StartConfig;
use crate::stop::StopConfig;
use crate::transport::handler::{
    compute_health, compute_status, error_message, extract_error_fields, handle_input,
    handle_input_raw, handle_keys, handle_nudge, handle_resize, handle_respond, handle_signal,
    TransportQuestionAnswer,
};
use crate::transport::state::Store;

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
    cursor: bool,
) -> proto::GetScreenResponse {
    proto::GetScreenResponse {
        lines: s.lines.clone(),
        cols: s.cols as i32,
        rows: s.rows as i32,
        alt_screen: s.alt_screen,
        cursor: if cursor { Some(cursor_to_proto(&s.cursor)) } else { None },
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

/// Convert a domain [`TransitionEvent`] to proto [`proto::TransitionEvent`].
pub fn transition_to_proto(e: &TransitionEvent) -> proto::TransitionEvent {
    let (error_detail, error_category) = extract_error_fields(&e.next);
    let cause = if e.cause.is_empty() { None } else { Some(e.cause.clone()) };
    proto::TransitionEvent {
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
    state: Arc<Store>,
}

impl CoopGrpc {
    /// Create a new gRPC service backed by the given shared state.
    pub fn new(state: Arc<Store>) -> Self {
        Self { state }
    }

    /// Build a [`tonic`] router for this service.
    ///
    /// When an auth token is configured, an interceptor validates Bearer
    /// tokens on all RPCs except `GetHealth` and `GetReady`.
    pub fn into_router(self) -> tonic::transport::server::Router {
        let auth_token = self.state.config.auth_token.clone();
        let mut server = tonic::transport::Server::builder();
        if let Some(token) = auth_token {
            let interceptor = GrpcAuthInterceptor { token };
            server.add_service(proto::coop_server::CoopServer::with_interceptor(self, interceptor))
        } else {
            server.add_service(proto::coop_server::CoopServer::new(self))
        }
    }
}

/// gRPC interceptor that validates Bearer tokens on all RPCs.
///
/// Unlike the HTTP auth middleware which exempts health/ready probes,
/// gRPC auth applies uniformly — gRPC clients are orchestration tools
/// that always have the auth token available.
#[derive(Clone)]
struct GrpcAuthInterceptor {
    token: String,
}

impl tonic::service::Interceptor for GrpcAuthInterceptor {
    fn call(&mut self, req: Request<()>) -> Result<Request<()>, Status> {
        let header = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing authorization header"))?;

        let bearer = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| Status::unauthenticated("invalid authorization scheme"))?;

        if crate::transport::auth::constant_time_eq(bearer, &self.token) {
            Ok(req)
        } else {
            Err(Status::unauthenticated("invalid token"))
        }
    }
}

type GrpcStream<T> = Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl proto::coop_server::Coop for CoopGrpc {
    // -- Terminal -------------------------------------------------------------

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

    async fn get_ready(
        &self,
        _request: Request<proto::GetReadyRequest>,
    ) -> Result<Response<proto::GetReadyResponse>, Status> {
        let ready = self.state.ready.load(Ordering::Acquire);
        Ok(Response::new(proto::GetReadyResponse { ready }))
    }

    async fn get_screen(
        &self,
        request: Request<proto::GetScreenRequest>,
    ) -> Result<Response<proto::GetScreenResponse>, Status> {
        let req = request.into_inner();
        let screen = self.state.terminal.screen.read().await;
        let snap = screen.snapshot();
        Ok(Response::new(screen_snapshot_to_response(&snap, req.cursor)))
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

    type StreamOutputStream = GrpcStream<proto::OutputChunk>;

    async fn stream_output(
        &self,
        request: Request<proto::StreamOutputRequest>,
    ) -> Result<Response<Self::StreamOutputStream>, Status> {
        let from_offset = request.into_inner().from_offset;
        let (tx, rx) = mpsc::channel(crate::SIGNAL_CHANNEL_CAPACITY);

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

    async fn send_input(
        &self,
        request: Request<proto::SendInputRequest>,
    ) -> Result<Response<proto::SendInputResponse>, Status> {
        let req = request.into_inner();
        let len = handle_input(&self.state, req.text, req.enter).await;
        Ok(Response::new(proto::SendInputResponse { bytes_written: len }))
    }

    async fn send_input_raw(
        &self,
        request: Request<proto::SendInputRawRequest>,
    ) -> Result<Response<proto::SendInputRawResponse>, Status> {
        let req = request.into_inner();
        let len = handle_input_raw(&self.state, req.data).await;
        Ok(Response::new(proto::SendInputRawResponse { bytes_written: len }))
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

    // -- Agent ----------------------------------------------------------------

    async fn get_agent(
        &self,
        _request: Request<proto::GetAgentRequest>,
    ) -> Result<Response<proto::GetAgentResponse>, Status> {
        let agent = self.state.driver.agent_state.read().await;
        let screen = self.state.terminal.screen.read().await;

        let detection = self.state.driver.detection.read().await;

        Ok(Response::new(proto::GetAgentResponse {
            agent: self.state.config.agent.to_string(),
            state: agent.as_str().to_owned(),
            since_seq: self.state.driver.state_seq.load(Ordering::Acquire),
            screen_seq: screen.seq(),
            detection_tier: detection.tier_str(),
            detection_cause: detection.cause.clone(),
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

    type StreamAgentStream = GrpcStream<proto::TransitionEvent>;

    async fn stream_agent(
        &self,
        _request: Request<proto::StreamAgentRequest>,
    ) -> Result<Response<Self::StreamAgentStream>, Status> {
        let state_rx = self.state.channels.state_tx.subscribe();
        let stream = spawn_broadcast_stream(state_rx, |event| Some(transition_to_proto(&event)));
        Ok(Response::new(stream))
    }

    type StreamPromptOutcomesStream = GrpcStream<proto::PromptOutcomeEvent>;

    async fn stream_prompt_outcomes(
        &self,
        _request: Request<proto::StreamPromptOutcomesRequest>,
    ) -> Result<Response<Self::StreamPromptOutcomesStream>, Status> {
        let prompt_rx = self.state.channels.prompt_tx.subscribe();
        let stream = spawn_broadcast_stream(prompt_rx, |event| {
            Some(proto::PromptOutcomeEvent {
                source: event.source,
                r#type: event.r#type,
                subtype: event.subtype,
                option: event.option,
            })
        });
        Ok(Response::new(stream))
    }

    // -- Stop hook ------------------------------------------------------------

    async fn get_stop_config(
        &self,
        _request: Request<proto::GetStopConfigRequest>,
    ) -> Result<Response<proto::GetStopConfigResponse>, Status> {
        let config = self.state.stop.config.read().await;
        let json = serde_json::to_string(&*config)
            .map_err(|e| Status::internal(format!("serialize error: {e}")))?;
        Ok(Response::new(proto::GetStopConfigResponse { config_json: json }))
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

    type StreamStopEventsStream = GrpcStream<proto::StopEvent>;

    async fn stream_stop_events(
        &self,
        _request: Request<proto::StreamStopEventsRequest>,
    ) -> Result<Response<Self::StreamStopEventsStream>, Status> {
        let stop_rx = self.state.stop.stop_tx.subscribe();
        let stream = spawn_broadcast_stream(stop_rx, |event| {
            Some(proto::StopEvent {
                r#type: event.r#type.as_str().to_owned(),
                signal_json: event.signal.map(|v| v.to_string()),
                error_detail: event.error_detail,
                seq: event.seq,
            })
        });
        Ok(Response::new(stream))
    }

    // -- Start hook -----------------------------------------------------------

    async fn get_start_config(
        &self,
        _request: Request<proto::GetStartConfigRequest>,
    ) -> Result<Response<proto::GetStartConfigResponse>, Status> {
        let config = self.state.start.config.read().await;
        let json = serde_json::to_string(&*config)
            .map_err(|e| Status::internal(format!("serialize error: {e}")))?;
        Ok(Response::new(proto::GetStartConfigResponse { config_json: json }))
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

    // -- Lifecycle ------------------------------------------------------------

    async fn shutdown(
        &self,
        _request: Request<proto::ShutdownRequest>,
    ) -> Result<Response<proto::ShutdownResponse>, Status> {
        self.state.lifecycle.shutdown.cancel();
        Ok(Response::new(proto::ShutdownResponse { accepted: true }))
    }
}

#[cfg(test)]
#[path = "grpc_tests.rs"]
mod tests;
