// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared handler functions for HTTP, WebSocket, and gRPC transports.
//!
//! Each transport is a thin wire-format adapter: parse → call shared fn → serialize.
//! Business logic lives here to prevent behavioral divergence.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::driver::AgentType;
use crate::driver::{classify_error_detail, AgentState, QuestionAnswer};
use crate::error::ErrorCode;
use crate::event::InputEvent;
use crate::event::PtySignal;
use crate::switch::SwitchRequest;
use crate::transport::state::Store;
use crate::transport::{
    deliver_steps, encode_response, keys_to_bytes, spawn_enter_retry, update_question_current,
};

/// Health check result.
pub struct HealthInfo {
    pub status: String,
    pub session_id: String,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub agent: String,
    pub terminal_cols: u16,
    pub terminal_rows: u16,
    pub ws_clients: i32,
    pub ready: bool,
}

/// Session status result.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStatus {
    pub session_id: String,
    pub state: String,
    pub pid: Option<i32>,
    pub uptime_secs: i64,
    pub exit_code: Option<i32>,
    pub screen_seq: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub ws_clients: i32,
}

/// Nudge delivery result.
#[derive(Debug, Serialize)]
pub struct NudgeOutcome {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Respond delivery result.
#[derive(Debug, Serialize)]
pub struct RespondOutcome {
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Transport-agnostic question answer (shared across HTTP, WS, gRPC).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportQuestionAnswer {
    pub option: Option<i32>,
    pub text: Option<String>,
}

/// Convert transport question answers to domain [`QuestionAnswer`] values.
pub fn to_domain_answers(answers: &[TransportQuestionAnswer]) -> Vec<QuestionAnswer> {
    answers
        .iter()
        .map(|a| QuestionAnswer { option: a.option.map(|o| o as u32), text: a.text.clone() })
        .collect()
}

/// Determine session state string from agent state and child PID.
///
/// Returns the agent state wire string (e.g. `"working"`, `"idle"`, `"prompt"`).
/// When the child process hasn't started yet (pid == 0), returns `"starting"`
/// regardless of the agent's internal state.
pub fn session_state_str(agent: &AgentState, child_pid: u32) -> &'static str {
    if child_pid == 0 && !matches!(agent, AgentState::Exited { .. }) {
        "starting"
    } else {
        agent.as_str()
    }
}

/// Map an error code to a human-readable message for error responses.
pub fn error_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NotReady => "agent is still starting",
        ErrorCode::NoDriver => "no agent driver configured",
        _ => "request failed",
    }
}

/// Extract error detail and category fields from an agent state.
///
/// Returns `(error_detail, error_category)` — both `None` for non-error states.
pub fn extract_error_fields(agent: &AgentState) -> (Option<String>, Option<String>) {
    match agent {
        AgentState::Error { detail } => {
            let category = classify_error_detail(detail);
            (Some(detail.clone()), Some(category.as_str().to_owned()))
        }
        _ => (None, None),
    }
}

/// Extract parked fields from an agent state.
///
/// Returns `(parked_reason, resume_at_epoch_ms)` — both `None` for non-parked states.
pub fn extract_parked_fields(agent: &AgentState) -> (Option<String>, Option<u64>) {
    match agent {
        AgentState::Parked { reason, resume_at_epoch_ms } => {
            (Some(reason.clone()), Some(*resume_at_epoch_ms))
        }
        _ => (None, None),
    }
}

/// Resolve profile credentials on a [`SwitchRequest`].
///
/// If `profile` is set but `credentials` is `None`, look up the named profile.
pub async fn resolve_switch_profile(
    store: &Store,
    req: &mut SwitchRequest,
) -> Result<(), ErrorCode> {
    if req.profile.is_some() && req.credentials.is_none() {
        let name = req.profile.as_deref().unwrap_or_default();
        match store.profile.resolve_credentials(name).await {
            Some(creds) => req.credentials = Some(creds),
            None => return Err(ErrorCode::BadRequest),
        }
    }
    Ok(())
}

/// Compute health info.
///
/// Uses `try_read` for RwLock-guarded fields to avoid blocking when locks are
/// contended under heavy terminal I/O. K8s liveness probes must not hang.
pub async fn compute_health(state: &Store) -> HealthInfo {
    let (cols, rows) = match state.terminal.screen.try_read() {
        Ok(screen) => {
            let s = screen.snapshot();
            (s.cols, s.rows)
        }
        Err(_) => (0, 0),
    };
    let pid = state.terminal.child_pid.load(Ordering::Relaxed);
    let uptime = state.config.started_at.elapsed().as_secs() as i64;
    let ready = state.ready.load(Ordering::Acquire);
    let session_id = state.session_id.try_read().map(|s| s.clone()).unwrap_or_default();

    HealthInfo {
        status: "running".to_owned(),
        session_id,
        pid: if pid == 0 { None } else { Some(pid as i32) },
        uptime_secs: uptime,
        agent: state.config.agent.to_string(),
        terminal_cols: cols,
        terminal_rows: rows,
        ws_clients: state.lifecycle.ws_client_count.load(Ordering::Relaxed),
        ready,
    }
}

/// Compute session status.
pub async fn compute_status(state: &Store) -> SessionStatus {
    let agent = state.driver.agent_state.read().await;
    let ring = state.terminal.ring.read().await;
    let screen = state.terminal.screen.read().await;
    let pid = state.terminal.child_pid.load(Ordering::Relaxed);
    let exit = state.terminal.exit_status.read().await;
    let bw = state.lifecycle.bytes_written.load(Ordering::Relaxed);
    let session_id = state.session_id.read().await.clone();

    SessionStatus {
        session_id,
        state: session_state_str(&agent, pid).to_owned(),
        pid: if pid == 0 { None } else { Some(pid as i32) },
        uptime_secs: state.config.started_at.elapsed().as_secs() as i64,
        exit_code: exit.as_ref().and_then(|e| e.code),
        screen_seq: screen.seq(),
        bytes_read: ring.total_written(),
        bytes_written: bw,
        ws_clients: state.lifecycle.ws_client_count.load(Ordering::Relaxed),
    }
}

/// Send a nudge to the agent.
///
/// Returns `Err` only for genuine errors (not ready, no driver).
/// Agent-busy is a soft failure returned as `Ok(NudgeOutcome { delivered: false })`.
pub async fn handle_nudge(state: &Store, message: &str) -> Result<NudgeOutcome, ErrorCode> {
    if !state.ready.load(Ordering::Acquire) {
        return Err(ErrorCode::NotReady);
    }

    let encoder = match &state.config.nudge_encoder {
        Some(enc) => Arc::clone(enc),
        None => return Err(ErrorCode::NoDriver),
    };

    // Subscribe to state changes BEFORE delivering so we don't miss
    // the Working transition that confirms Enter was processed.
    let state_rx = state.channels.state_tx.subscribe();

    let mut _delivery = state.input_gate.acquire().await;

    let agent = state.driver.agent_state.read().await;
    let state_before = agent.as_str().to_owned();

    match &*agent {
        AgentState::Idle => {}
        // Allow nudge on oauth_login prompts (user pastes a login code + enter).
        AgentState::Prompt { prompt }
            if prompt.kind == crate::driver::PromptKind::Setup
                && prompt.subtype.as_deref() == Some("oauth_login") => {}
        // Delegate to respond for prompts that accept freetext.
        AgentState::Prompt { prompt }
            if prompt.kind == crate::driver::PromptKind::Plan
                || prompt.kind == crate::driver::PromptKind::Question =>
        {
            let kind = prompt.kind;
            drop(agent);
            // Release the gate before delegating — handle_respond acquires its own.
            drop(_delivery);
            let respond = match kind {
                // Plan: option 4 (feedback) with nudge message as text.
                crate::driver::PromptKind::Plan => {
                    handle_respond(state, None, None, Some(message), &[]).await
                }
                // Question: single freetext answer for current question.
                crate::driver::PromptKind::Question => {
                    let answer =
                        TransportQuestionAnswer { option: None, text: Some(message.to_owned()) };
                    handle_respond(state, None, None, None, &[answer]).await
                }
                crate::driver::PromptKind::Permission | crate::driver::PromptKind::Setup => {
                    unreachable!("guard filters to Plan | Question")
                }
            };
            return match respond {
                Ok(r) => Ok(NudgeOutcome {
                    delivered: r.delivered,
                    state_before: Some(state_before),
                    reason: r.reason,
                }),
                Err(code) => Err(code),
            };
        }
        _ => {
            return Ok(NudgeOutcome {
                delivered: false,
                state_before: Some(state_before.clone()),
                reason: Some(format!("agent is {state_before}")),
            });
        }
    }
    drop(agent);

    let steps = encoder.encode(message);
    let _ = deliver_steps(&state.channels.input_tx, steps).await;

    // Safety net: if Enter didn't register, retry once after timeout.
    // Only for Claude — other agents don't have the ink TUI rendering bug.
    let nudge_timeout = state.config.nudge_timeout;
    if state.config.agent == AgentType::Claude && nudge_timeout > std::time::Duration::ZERO {
        let cancel = spawn_enter_retry(
            state.channels.input_tx.clone(),
            state_rx,
            Arc::clone(&state.input_activity),
            nudge_timeout,
        );
        _delivery.set_retry_cancel(cancel);
    }

    Ok(NudgeOutcome { delivered: true, state_before: Some(state_before), reason: None })
}

/// Respond to an active prompt.
///
/// Returns `Err` only for genuine errors (not ready, no driver).
/// No-prompt is a soft failure returned as `Ok(RespondOutcome { delivered: false })`.
pub async fn handle_respond(
    state: &Store,
    accept: Option<bool>,
    option: Option<i32>,
    text: Option<&str>,
    answers: &[TransportQuestionAnswer],
) -> Result<RespondOutcome, ErrorCode> {
    if !state.ready.load(Ordering::Acquire) {
        return Err(ErrorCode::NotReady);
    }

    let encoder = match &state.config.respond_encoder {
        Some(enc) => Arc::clone(enc),
        None => return Err(ErrorCode::NoDriver),
    };

    let domain_answers = to_domain_answers(answers);
    let resolved_option = option.map(|o| o as u32);

    let _delivery = state.input_gate.acquire().await;

    let agent = state.driver.agent_state.read().await;
    let prompt_type = agent.prompt().map(|p| p.kind.as_str().to_owned());
    let prompt_subtype = agent.prompt().and_then(|p| p.subtype.clone());

    let (steps, answers_delivered) = match encode_response(
        &agent,
        encoder.as_ref(),
        accept,
        resolved_option,
        text,
        &domain_answers,
    ) {
        Ok(r) => r,
        Err(_code) => {
            return Ok(RespondOutcome {
                delivered: false,
                prompt_type: None,
                reason: Some("no prompt active".to_owned()),
            });
        }
    };

    drop(agent);
    let _ = deliver_steps(&state.channels.input_tx, steps).await;

    if answers_delivered > 0 {
        update_question_current(state, answers_delivered).await;
    }

    // Broadcast prompt event so WebSocket/event stream shows the response.
    let _ = state.channels.prompt_tx.send(crate::event::PromptOutcome {
        source: "api".to_owned(),
        r#type: prompt_type.clone().unwrap_or_default(),
        subtype: prompt_subtype,
        option: resolved_option,
    });

    Ok(RespondOutcome { delivered: true, prompt_type, reason: None })
}

/// Write text to the PTY, optionally followed by a carriage return.
pub async fn handle_input(state: &Store, text: String, enter: bool) -> i32 {
    let mut data = text.into_bytes();
    if enter {
        data.push(b'\r');
    }
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    len
}

/// Write raw bytes to the PTY.
pub async fn handle_input_raw(state: &Store, data: Vec<u8>) -> i32 {
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    len
}

/// Send named key sequences to the PTY.
///
/// Returns the byte count on success, or the unrecognised key name on failure.
pub async fn handle_keys(state: &Store, keys: &[String]) -> Result<i32, String> {
    let data = keys_to_bytes(keys)?;
    let len = data.len() as i32;
    let _ = state.channels.input_tx.send(InputEvent::Write(Bytes::from(data))).await;
    Ok(len)
}

/// Resize the PTY.
pub async fn handle_resize(state: &Store, cols: u16, rows: u16) -> Result<(), ErrorCode> {
    if cols == 0 || rows == 0 {
        return Err(ErrorCode::BadRequest);
    }
    let _ = state.channels.input_tx.send(InputEvent::Resize { cols, rows }).await;
    Ok(())
}

/// Send a signal to the child process.
///
/// Returns `Ok(())` on success, or the unknown signal name on failure.
pub async fn handle_signal(state: &Store, signal: &str) -> Result<(), String> {
    let sig = PtySignal::from_name(signal).ok_or_else(|| signal.to_owned())?;
    let _ = state.channels.input_tx.send(InputEvent::Signal(sig)).await;
    Ok(())
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
