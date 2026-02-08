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
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use crate::driver::{AgentState, PromptContext};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::screen::CursorPosition;
use crate::transport::auth;
use crate::transport::state::AppState;
use crate::transport::{
    deliver_steps, encode_response, keys_to_bytes, parse_signal, read_ring_combined,
};

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

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
        prompt: Option<PromptContext>,
    },
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },
    Error {
        code: String,
        message: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    Pong {},
}

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Input {
        text: String,
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
    Nudge {
        message: String,
    },
    Respond {
        accept: Option<bool>,
        option: Option<i32>,
        text: Option<String>,
    },
    Replay {
        offset: u64,
    },
    Lock {
        action: LockAction,
    },
    Auth {
        token: String,
    },
    Signal {
        name: String,
    },
    Ping {},
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockAction {
    Acquire,
    Release,
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

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

/// WebSocket upgrade handler. Validates auth from query params if configured.
pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    // Validate auth token from query param if one is required.
    if state.auth_token.is_some() {
        if let Some(ref token) = query.token {
            if let Err(_code) = auth::validate_ws_auth(token, state.auth_token.as_deref()) {
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
    let needs_auth = state.auth_token.is_some() && query.token.is_none();

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
    state.ws_client_count.fetch_add(1, Ordering::Relaxed);

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut output_rx = state.output_tx.subscribe();
    let mut state_rx = state.state_tx.subscribe();
    let mut authed = !needs_auth;

    loop {
        tokio::select! {
            event = output_rx.recv() => {
                let event = match event {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                match (&event, mode) {
                    (OutputEvent::Raw(data), SubscriptionMode::Raw | SubscriptionMode::All) => {
                        let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                        let ring = state.ring.read().await;
                        let offset = ring.total_written().saturating_sub(data.len() as u64);
                        let msg = ServerMessage::Output { data: encoded, offset };
                        if send_json(&mut ws_tx, &msg).await.is_err() {
                            break;
                        }
                    }
                    (OutputEvent::ScreenUpdate { seq }, SubscriptionMode::Screen | SubscriptionMode::All) => {
                        let snap = state.screen.read().await.snapshot();
                        let msg = ServerMessage::Screen {
                            lines: snap.lines,
                            cols: snap.cols,
                            rows: snap.rows,
                            alt_screen: snap.alt_screen,
                            cursor: Some(snap.cursor),
                            seq: *seq,
                        };
                        if send_json(&mut ws_tx, &msg).await.is_err() {
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
    state.ws_client_count.fetch_sub(1, Ordering::Relaxed);
    state.write_lock.force_release_ws(&client_id);
}

/// Handle a single client message and optionally return a reply.
async fn handle_client_message(
    state: &AppState,
    msg: ClientMessage,
    client_id: &str,
    authed: &mut bool,
) -> Option<ServerMessage> {
    match msg {
        ClientMessage::Ping {} => Some(ServerMessage::Pong {}),

        ClientMessage::Auth { token } => {
            match auth::validate_ws_auth(&token, state.auth_token.as_deref()) {
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
            let snap = state.screen.read().await.snapshot();
            Some(ServerMessage::Screen {
                lines: snap.lines,
                cols: snap.cols,
                rows: snap.rows,
                alt_screen: snap.alt_screen,
                cursor: Some(snap.cursor),
                seq: snap.sequence,
            })
        }

        ClientMessage::StateRequest {} => {
            let agent = state.agent_state.read().await;
            let screen = state.screen.read().await;
            Some(ServerMessage::StateChange {
                prev: agent.as_str().to_owned(),
                next: agent.as_str().to_owned(),
                seq: screen.seq(),
                prompt: agent.prompt().cloned(),
            })
        }

        ClientMessage::Replay { offset } => {
            let ring = state.ring.read().await;
            let combined = read_ring_combined(&ring, offset);
            let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);
            Some(ServerMessage::Output {
                data: encoded,
                offset,
            })
        }

        // Write operations require auth and write lock
        ClientMessage::Input { text } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            let data = Bytes::from(text.into_bytes());
            let _ = state.input_tx.send(InputEvent::Write(data)).await;
            None
        }

        ClientMessage::InputRaw { data } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&data)
                .unwrap_or_default();
            let _ = state
                .input_tx
                .send(InputEvent::Write(Bytes::from(decoded)))
                .await;
            None
        }

        ClientMessage::Keys { keys } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            let data = match keys_to_bytes(&keys) {
                Ok(d) => d,
                Err(bad_key) => {
                    return Some(ws_error(
                        ErrorCode::BadRequest,
                        &format!("unknown key: {bad_key}"),
                    ));
                }
            };
            let _ = state
                .input_tx
                .send(InputEvent::Write(Bytes::from(data)))
                .await;
            None
        }

        ClientMessage::Resize { cols, rows } => {
            if cols == 0 || rows == 0 {
                return Some(ws_error(
                    ErrorCode::BadRequest,
                    "cols and rows must be positive",
                ));
            }
            let _ = state.input_tx.send(InputEvent::Resize { cols, rows }).await;
            None
        }

        ClientMessage::Lock { action } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            match action {
                LockAction::Acquire => match state.write_lock.acquire_ws(client_id) {
                    Ok(()) => None,
                    Err(code) => Some(ws_error(code, "write lock held by another client")),
                },
                LockAction::Release => match state.write_lock.release_ws(client_id) {
                    Ok(()) => None,
                    Err(code) => Some(ws_error(code, "not the lock owner")),
                },
            }
        }

        ClientMessage::Nudge { message } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            let encoder = match &state.nudge_encoder {
                Some(enc) => Arc::clone(enc),
                None => return Some(ws_error(ErrorCode::NoDriver, "no agent driver configured")),
            };
            let agent = state.agent_state.read().await;
            if !matches!(&*agent, AgentState::WaitingForInput) {
                return Some(ws_error(
                    ErrorCode::AgentBusy,
                    "agent is not waiting for input",
                ));
            }
            drop(agent);
            let steps = encoder.encode(&message);
            let _ = deliver_steps(&state.input_tx, steps).await;
            None
        }

        ClientMessage::Respond {
            accept,
            option,
            text,
        } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            let encoder = match &state.respond_encoder {
                Some(enc) => Arc::clone(enc),
                None => return Some(ws_error(ErrorCode::NoDriver, "no agent driver configured")),
            };
            let agent = state.agent_state.read().await;
            let steps =
                match encode_response(&agent, encoder.as_ref(), accept, option, text.as_deref()) {
                    Ok(s) => s,
                    Err(code) => {
                        return Some(ws_error(code, "no prompt active"));
                    }
                };
            drop(agent);
            let _ = deliver_steps(&state.input_tx, steps).await;
            None
        }

        ClientMessage::Signal { name } => {
            if !*authed {
                return Some(ws_error(ErrorCode::Unauthorized, "not authenticated"));
            }
            if let Err(code) = state.write_lock.check_ws(client_id) {
                return Some(ws_error(code, "write lock not held"));
            }
            match parse_signal(&name) {
                Some(signum) => {
                    let _ = state.input_tx.send(InputEvent::Signal(signum)).await;
                    None
                }
                None => Some(ws_error(
                    ErrorCode::BadRequest,
                    &format!("unknown signal: {name}"),
                )),
            }
        }
    }
}

/// Build a WebSocket error message.
fn ws_error(code: ErrorCode, message: &str) -> ServerMessage {
    ServerMessage::Error {
        code: code.as_str().to_owned(),
        message: message.to_owned(),
    }
}

/// Convert a `StateChangeEvent` to a `ServerMessage`.
fn state_change_to_msg(event: &StateChangeEvent) -> ServerMessage {
    let prompt = event.next.prompt().cloned();
    match &event.next {
        AgentState::Exited { status } => ServerMessage::Exit {
            code: status.code,
            signal: status.signal,
        },
        _ => ServerMessage::StateChange {
            prev: event.prev.as_str().to_owned(),
            next: event.next.as_str().to_owned(),
            seq: event.seq,
            prompt,
        },
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
