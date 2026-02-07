// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! WebSocket transport with subscription modes.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use base64::Engine;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use super::{encode_key, AppState};
use crate::driver::{AgentState, PromptContext};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::screen::CursorPosition;

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

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct WsQuery {
    #[serde(default)]
    pub mode: SubscriptionMode,
    pub token: Option<String>,
}

// ---------------------------------------------------------------------------
// Upgrade handler
// ---------------------------------------------------------------------------

pub async fn ws_upgrade(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state, query))
}

async fn handle_ws(socket: WebSocket, state: Arc<AppState>, query: WsQuery) {
    // Track WebSocket client count
    state.ws_clients.fetch_add(1, Ordering::Relaxed);

    let result = run_ws_loop(socket, &state, query).await;
    if let Err(e) = result {
        tracing::debug!("websocket session ended: {e}");
    }

    state.ws_clients.fetch_sub(1, Ordering::Relaxed);
}

async fn run_ws_loop(
    mut socket: WebSocket,
    state: &Arc<AppState>,
    query: WsQuery,
) -> anyhow::Result<()> {
    let mode = query.mode;

    // Subscribe to broadcast channels based on mode
    let mut output_rx = if matches!(mode, SubscriptionMode::Raw | SubscriptionMode::All) {
        Some(state.output_tx.subscribe())
    } else {
        None
    };

    let mut state_rx = if matches!(mode, SubscriptionMode::State | SubscriptionMode::All) {
        Some(state.state_tx.subscribe())
    } else {
        None
    };

    let mut screen_rx = if matches!(mode, SubscriptionMode::Screen | SubscriptionMode::All) {
        Some(state.output_tx.subscribe())
    } else {
        None
    };

    loop {
        tokio::select! {
            // Forward raw output to client
            Some(event) = async {
                if let Some(ref mut rx) = output_rx {
                    Some(rx.recv().await)
                } else {
                    std::future::pending::<Option<Result<OutputEvent, broadcast::error::RecvError>>>().await
                }
            } => {
                match event {
                    Ok(OutputEvent::Raw(data)) => {
                        let ring = state.ring.read().await;
                        let offset = ring.total_written() - data.len() as u64;
                        drop(ring);
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                        let msg = ServerMessage::Output { data: encoded, offset };
                        send_json(&mut socket, &msg).await?;
                    }
                    Ok(OutputEvent::ScreenUpdate { .. }) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Forward screen updates to client
            Some(event) = async {
                if let Some(ref mut rx) = screen_rx {
                    Some(rx.recv().await)
                } else {
                    std::future::pending::<Option<Result<OutputEvent, broadcast::error::RecvError>>>().await
                }
            } => {
                match event {
                    Ok(OutputEvent::ScreenUpdate { seq }) => {
                        let screen = state.screen.read().await;
                        let snap = screen.snapshot();
                        drop(screen);
                        let msg = ServerMessage::Screen {
                            lines: snap.lines,
                            cols: snap.cols,
                            rows: snap.rows,
                            alt_screen: snap.alt_screen,
                            cursor: Some(snap.cursor),
                            seq,
                        };
                        send_json(&mut socket, &msg).await?;
                    }
                    Ok(OutputEvent::Raw(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Forward state changes to client
            Some(event) = async {
                if let Some(ref mut rx) = state_rx {
                    Some(rx.recv().await)
                } else {
                    std::future::pending::<Option<Result<StateChangeEvent, broadcast::error::RecvError>>>().await
                }
            } => {
                match event {
                    Ok(change) => {
                        let msg = ServerMessage::StateChange {
                            prev: change.prev.as_str().to_owned(),
                            next: change.next.as_str().to_owned(),
                            seq: change.seq,
                            prompt: change.next.prompt().cloned(),
                        };
                        send_json(&mut socket, &msg).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }

            // Handle incoming client messages
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                            handle_client_message(&mut socket, state, client_msg).await?;
                        } else {
                            let err = ServerMessage::Error {
                                code: "BAD_REQUEST".to_owned(),
                                message: "invalid message format".to_owned(),
                            };
                            send_json(&mut socket, &err).await?;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // Binary, Ping, Pong handled by axum
                    Some(Err(_)) => break,
                }
            }
        }
    }

    Ok(())
}

async fn handle_client_message(
    socket: &mut WebSocket,
    state: &Arc<AppState>,
    msg: ClientMessage,
) -> anyhow::Result<()> {
    match msg {
        ClientMessage::Input { text } => {
            let _ = state
                .input_tx
                .send(InputEvent::Write(Bytes::from(text.into_bytes())))
                .await;
        }
        ClientMessage::InputRaw { data } => {
            if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&data) {
                let _ = state
                    .input_tx
                    .send(InputEvent::Write(Bytes::from(decoded)))
                    .await;
            }
        }
        ClientMessage::Keys { keys } => {
            for key in &keys {
                if let Some(encoded) = encode_key(key) {
                    let _ = state
                        .input_tx
                        .send(InputEvent::Write(Bytes::from(encoded)))
                        .await;
                }
            }
        }
        ClientMessage::Resize { cols, rows } => {
            let _ = state.input_tx.send(InputEvent::Resize { cols, rows }).await;
        }
        ClientMessage::ScreenRequest {} => {
            let screen = state.screen.read().await;
            let snap = screen.snapshot();
            drop(screen);
            let msg = ServerMessage::Screen {
                lines: snap.lines,
                cols: snap.cols,
                rows: snap.rows,
                alt_screen: snap.alt_screen,
                cursor: Some(snap.cursor),
                seq: snap.sequence,
            };
            send_json(socket, &msg).await?;
        }
        ClientMessage::StateRequest {} => {
            let agent = state.agent_state.read().await;
            let msg = ServerMessage::StateChange {
                prev: agent.as_str().to_owned(),
                next: agent.as_str().to_owned(),
                seq: 0,
                prompt: agent.prompt().cloned(),
            };
            send_json(socket, &msg).await?;
        }
        ClientMessage::Nudge { message } => {
            if let Some(encoder) = &state.nudge_encoder {
                let agent = state.agent_state.read().await;
                if matches!(&*agent, AgentState::WaitingForInput) {
                    let steps = encoder.encode(&message);
                    drop(agent);
                    for step in steps {
                        let _ = state
                            .input_tx
                            .send(InputEvent::Write(Bytes::from(step.bytes)))
                            .await;
                        if let Some(delay) = step.delay_after {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
        }
        ClientMessage::Respond {
            accept,
            option,
            text,
        } => {
            if let Some(encoder) = &state.respond_encoder {
                let agent = state.agent_state.read().await;
                let steps = match &*agent {
                    AgentState::PermissionPrompt { .. } => {
                        Some(encoder.encode_permission(accept.unwrap_or(false)))
                    }
                    AgentState::PlanPrompt { .. } => {
                        Some(encoder.encode_plan(accept.unwrap_or(false), text.as_deref()))
                    }
                    AgentState::AskUser { .. } => {
                        Some(encoder.encode_question(option.map(|o| o as u32), text.as_deref()))
                    }
                    _ => None,
                };
                drop(agent);
                if let Some(steps) = steps {
                    for step in steps {
                        let _ = state
                            .input_tx
                            .send(InputEvent::Write(Bytes::from(step.bytes)))
                            .await;
                        if let Some(delay) = step.delay_after {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
        }
        ClientMessage::Replay { offset } => {
            let ring = state.ring.read().await;
            if let Some((a, b)) = ring.read_from(offset) {
                let mut data = Vec::with_capacity(a.len() + b.len());
                data.extend_from_slice(a);
                data.extend_from_slice(b);
                drop(ring);
                let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                let msg = ServerMessage::Output {
                    data: encoded,
                    offset,
                };
                send_json(socket, &msg).await?;
            }
        }
        ClientMessage::Lock { .. } => {
            // Writer lock not yet implemented; acknowledge silently
        }
        ClientMessage::Auth { .. } => {
            // Auth handled via middleware/query param; acknowledge silently
        }
        ClientMessage::Ping {} => {
            send_json(socket, &ServerMessage::Pong {}).await?;
        }
    }
    Ok(())
}

async fn send_json(socket: &mut WebSocket, msg: &ServerMessage) -> anyhow::Result<()> {
    let text = serde_json::to_string(msg)?;
    socket
        .send(Message::Text(text))
        .await
        .map_err(|e| anyhow::anyhow!("websocket send failed: {e}"))?;
    Ok(())
}
