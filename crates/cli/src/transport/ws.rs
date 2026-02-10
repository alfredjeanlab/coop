// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! WebSocket message types and handler for the coop real-time protocol.
//!
//! Messages use internally-tagged JSON enums (`{"type": "input", ...}`) as
//! specified in DESIGN.md. Two top-level enums cover server-to-client and
//! client-to-server directions.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::{OutputEvent, StateChangeEvent};
use crate::screen::{CursorPosition, ScreenSnapshot};
use crate::start::StartEvent;
use crate::stop::StopEvent;
use crate::transport::auth;
use crate::transport::handler::{
    compute_status, error_message, extract_error_fields, handle_input, handle_input_raw,
    handle_keys, handle_nudge, handle_resize, handle_respond, handle_signal, NudgeOutcome,
    RespondOutcome, SessionStatus, TransportQuestionAnswer,
};
use crate::transport::read_ring_combined;
use crate::transport::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Output {
        data: String,
        offset: u64,
    },
    Screen {
        lines: Vec<String>,
        cols: u16,
        rows: u16,
        alt_screen: bool,
        cursor: Option<CursorPosition>,
        seq: u64,
    },
    StateChange {
        prev: String,
        next: String,
        seq: u64,
        prompt: Box<Option<PromptContext>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_category: Option<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        cause: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Error {
        code: String,
        message: String,
    },
    NudgeResult {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        state_before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    RespondResult {
        delivered: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Status {
        state: String,
        pid: Option<i32>,
        uptime_secs: i64,
        exit_code: Option<i32>,
        screen_seq: u64,
        bytes_read: u64,
        bytes_written: u64,
        ws_clients: i32,
    },
    Stop {
        stop_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_detail: Option<String>,
        seq: u64,
    },
    Start {
        source: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        injected: bool,
        seq: u64,
    },
    PromptAction {
        source: String,
        prompt_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        subtype: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        option: Option<u32>,
    },
    Pong {},
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Input {
        text: String,
        #[serde(default)]
        enter: bool,
    },
    InputRaw {
        data: String,
    },
    Keys {
        keys: Vec<String>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    ScreenRequest {},
    StateRequest {},
    StatusRequest {},
    Nudge {
        message: String,
    },
    Respond {
        accept: Option<bool>,
        text: Option<String>,
        #[serde(default)]
        answers: Vec<TransportQuestionAnswer>,
        option: Option<i32>,
    },
    Replay {
        offset: u64,
    },
    Auth {
        token: String,
    },
    Signal {
        signal: String,
    },
    Shutdown {},
    Ping {},
}

/// WebSocket subscription mode (query parameter on upgrade).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionMode {
    Raw,
    Screen,
    State,
    #[default]
    All,
}

/// Query parameters for WebSocket upgrade.
#[derive(Debug, Clone, Deserialize)]
pub struct WsQuery {
    #[serde(default)]
    pub mode: SubscriptionMode,
    pub token: Option<String>,
}

impl From<SessionStatus> for ServerMessage {
    fn from(st: SessionStatus) -> Self {
        ServerMessage::Status {
            state: st.state,
            pid: st.pid,
            uptime_secs: st.uptime_secs,
            exit_code: st.exit_code,
            screen_seq: st.screen_seq,
            bytes_read: st.bytes_read,
            bytes_written: st.bytes_written,
            ws_clients: st.ws_clients,
        }
    }
}

impl From<NudgeOutcome> for ServerMessage {
    fn from(o: NudgeOutcome) -> Self {
        ServerMessage::NudgeResult {
            delivered: o.delivered,
            state_before: o.state_before,
            reason: o.reason,
        }
    }
}

impl From<RespondOutcome> for ServerMessage {
    fn from(o: RespondOutcome) -> Self {
        ServerMessage::RespondResult {
            delivered: o.delivered,
            prompt_type: o.prompt_type,
            reason: o.reason,
        }
    }
}

/// Build a `ServerMessage::Screen` from a screen snapshot.
fn snapshot_to_msg(snap: ScreenSnapshot, seq: u64) -> ServerMessage {
    ServerMessage::Screen {
        lines: snap.lines,
        cols: snap.cols,
        rows: snap.rows,
        alt_screen: snap.alt_screen,
        cursor: Some(snap.cursor),
        seq,
    }
}

/// Short-circuit: return an auth error if the client has not authenticated.
macro_rules! require_auth {
    ($authed:expr) => {
        if !*$authed {
            return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
        }
    };
}

/// WebSocket upgrade handler. Validates auth from query params if configured.
pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate auth token from query param if one is required.
    if state.config.auth_token.is_some() {
        if let Some(ref token) = query.token {
            if let Err(_code) = auth::validate_ws_auth(token, state.config.auth_token.as_deref()) {
                return axum::http::Response::builder()
                    .status(401)
                    .body(axum::body::Body::from("unauthorized"))
                    .unwrap_or_default()
                    .into_response();
            }
        }
        // If no token provided in query, the client can still auth via Auth message.
        // We'll track auth state per-connection.
    }

    let mode = query.mode;
    let needs_auth = state.config.auth_token.is_some() && query.token.is_none();

    ws.on_upgrade(move |socket| {
        let client_id = format!("ws-{}", next_client_id());
        handle_connection(state, mode, socket, client_id, needs_auth)
    })
    .into_response()
}

/// Per-connection event loop.
async fn handle_connection(
    state: Arc<AppState>,
    mode: SubscriptionMode,
    socket: WebSocket,
    client_id: String,
    needs_auth: bool,
) {
    state.lifecycle.ws_client_count.fetch_add(1, Ordering::Relaxed);

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut output_rx = state.channels.output_tx.subscribe();
    let mut state_rx = state.channels.state_tx.subscribe();
    let mut prompt_rx = state.channels.prompt_tx.subscribe();
    let mut stop_rx = state.stop.stop_tx.subscribe();
    let mut start_rx = state.start.start_tx.subscribe();
    let mut authed = !needs_auth;

    loop {
        tokio::select! {
            event = prompt_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if matches!(mode, SubscriptionMode::State | SubscriptionMode::All) {
                    let msg = ServerMessage::PromptAction {
                        source: event.source,
                        prompt_type: event.r#type,
                        subtype: event.subtype,
                        option: event.option,
                    };
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = stop_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if matches!(mode, SubscriptionMode::State | SubscriptionMode::All) {
                    let msg = stop_event_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = start_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if matches!(mode, SubscriptionMode::State | SubscriptionMode::All) {
                    let msg = start_event_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            event = output_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match (&event, mode) {
                    (OutputEvent::Raw(data), SubscriptionMode::Raw | SubscriptionMode::All) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                        let ring = state.terminal.ring.read().await;
                        let offset = ring.total_written().saturating_sub(data.len() as u64);
                        let msg = ServerMessage::Output { data: encoded, offset };
                        if send_json(&mut ws_tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    (OutputEvent::ScreenUpdate { seq }, SubscriptionMode::Screen | SubscriptionMode::All) => {
                        let snap = state.terminal.screen.read().await.snapshot();
                        if send_json(&mut ws_tx, &snapshot_to_msg(snap, *seq)).await.is_err() {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            event = state_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                if matches!(mode, SubscriptionMode::State | SubscriptionMode::All) {
                    let msg = state_change_to_msg(&event);
                    if send_json(&mut ws_tx, &msg).await.is_err() {
                        break;
                    }
                }
            }
            msg = ws_rx.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break,
                };

                match msg {
                    Message::Text(text) => {
                        let client_msg: ClientMessage = match serde_json::from_str(&text) {
                            Ok(m) => m,
                            Err(_) => {
                                let err = ServerMessage::Error {
                                    code: ErrorCode::BadRequest.as_str().to_owned(),
                                    message: "invalid message".to_owned(),
                                };
                                if send_json(&mut ws_tx, &err).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };

                        if let Some(reply) = handle_client_message(&state, client_msg, &client_id, &mut authed).await {
                            if send_json(&mut ws_tx, &reply).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        }
    }

    // Cleanup
    state.lifecycle.ws_client_count.fetch_sub(1, Ordering::Relaxed);
}

/// Handle a single client message and optionally return a reply.
async fn handle_client_message(
    state: &AppState,
    msg: ClientMessage,
    _client_id: &str,
    authed: &mut bool,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Ping {} => Some(ServerMessage::Pong {}),

        ClientMessage::Auth { token } => {
            match auth::validate_ws_auth(&token, state.config.auth_token.as_deref()) {
                Ok(()) => {
                    *authed = true;
                    None
                }
                Err(code) => Some(ServerMessage::Error {
                    code: code.as_str().to_owned(),
                    message: "authentication failed".to_owned(),
                }),
            }
        }

        ClientMessage::ScreenRequest {} => {
            let snap = state.terminal.screen.read().await.snapshot();
            let seq = snap.sequence;
            Some(snapshot_to_msg(snap, seq))
        }

        ClientMessage::StateRequest {} => {
            let agent = state.driver.agent_state.read().await;
            let screen = state.terminal.screen.read().await;
            let cause = state.driver.detection_cause.read().await.clone();
            let (error_detail, error_category) = extract_error_fields(&agent);
            let last_message = state.driver.last_message.read().await.clone();
            Some(ServerMessage::StateChange {
                prev: agent.as_str().to_owned(),
                next: agent.as_str().to_owned(),
                seq: screen.seq(),
                prompt: Box::new(agent.prompt().cloned()),
                error_detail,
                error_category,
                cause,
                last_message,
            })
        }

        ClientMessage::StatusRequest {} => Some(compute_status(state).await.into()),

        ClientMessage::Replay { offset } => {
            let ring = state.terminal.ring.read().await;
            let combined = read_ring_combined(&ring, offset);
            let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);
            Some(ServerMessage::Output { data: encoded, offset })
        }

        // Write operations require auth
        ClientMessage::Input { text, enter } => {
            require_auth!(authed);
            let _ = handle_input(state, text, enter).await;
            None
        }

        ClientMessage::InputRaw { data } => {
            require_auth!(authed);
            let decoded = match base64::engine::general_purpose::STANDARD.decode(&data) {
                Ok(d) => d,
                Err(_) => return Some(ws_error(ErrorCode::BadRequest, "invalid base64 data")),
            };
            let _ = handle_input_raw(state, decoded).await;
            None
        }

        ClientMessage::Keys { keys } => {
            require_auth!(authed);
            match handle_keys(state, &keys).await {
                Ok(_) => None,
                Err(bad_key) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("unknown key: {bad_key}")))
                }
            }
        }

        ClientMessage::Resize { cols, rows } => {
            require_auth!(authed);
            match handle_resize(state, cols, rows).await {
                Ok(()) => None,
                Err(_) => Some(ws_error(ErrorCode::BadRequest, "cols and rows must be positive")),
            }
        }

        ClientMessage::Nudge { message } => {
            require_auth!(authed);
            match handle_nudge(state, &message).await {
                Ok(outcome) => Some(outcome.into()),
                Err(code) => Some(ws_error(code, error_message(code))),
            }
        }

        ClientMessage::Respond { accept, text, answers, option } => {
            require_auth!(authed);
            match handle_respond(state, accept, option, text.as_deref(), &answers).await {
                Ok(outcome) => Some(outcome.into()),
                Err(code) => Some(ws_error(code, error_message(code))),
            }
        }

        ClientMessage::Signal { signal } => {
            require_auth!(authed);
            match handle_signal(state, &signal).await {
                Ok(()) => None,
                Err(bad_signal) => {
                    Some(ws_error(ErrorCode::BadRequest, &format!("unknown signal: {bad_signal}")))
                }
            }
        }

        ClientMessage::Shutdown {} => {
            require_auth!(authed);
            state.lifecycle.shutdown.cancel();
            None // Connection will close as servers shut down
        }
    }
}

/// Build a WebSocket error message.
fn ws_error(code: ErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Error { code: code.as_str().to_owned(), message: message.to_owned() }
}

/// Convert a `StateChangeEvent` to a `ServerMessage`.
fn state_change_to_msg(event: &StateChangeEvent) -> ServerMessage {
    if let AgentState::Exited { status } = &event.next {
        return ServerMessage::Exit { code: status.code, signal: status.signal };
    }
    let (error_detail, error_category) = extract_error_fields(&event.next);
    ServerMessage::StateChange {
        prev: event.prev.as_str().to_owned(),
        next: event.next.as_str().to_owned(),
        seq: event.seq,
        prompt: Box::new(event.next.prompt().cloned()),
        error_detail,
        error_category,
        cause: event.cause.clone(),
        last_message: event.last_message.clone(),
    }
}

/// Convert a `StartEvent` to a `ServerMessage`.
fn start_event_to_msg(event: &StartEvent) -> ServerMessage {
    ServerMessage::Start {
        source: event.source.clone(),
        session_id: event.session_id.clone(),
        injected: event.injected,
        seq: event.seq,
    }
}

/// Convert a `StopEvent` to a `ServerMessage`.
fn stop_event_to_msg(event: &StopEvent) -> ServerMessage {
    ServerMessage::Stop {
        stop_type: event.stop_type.as_str().to_owned(),
        signal: event.signal.clone(),
        error_detail: event.error_detail.clone(),
        seq: event.seq,
    }
}

/// Send a JSON-serialized message over the WebSocket.
async fn send_json<S>(tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
{
    let text = match serde_json::to_string(msg) {
        Ok(t) => t,
        Err(_) => return Err(()),
    };
    tx.send(Message::Text(text.into())).await.map_err(|_| ())
}

/// Generate a simple unique ID (not cryptographic, just for client tracking).
fn next_client_id() -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{ts:x}-{n}")
}

#[cfg(test)]
#[path = "ws_tests.rs"]
mod tests;
