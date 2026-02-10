// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use crate::driver::{AgentState, ExitStatus};
use crate::event::InputEvent;
use crate::test_support::{AppStateBuilder, StubNudgeEncoder, StubRespondEncoder};
use crate::transport::handler::{
    compute_health, compute_status, handle_input, handle_input_raw, handle_keys, handle_nudge,
    handle_resize, handle_respond, handle_signal, session_state_str, to_domain_answers,
    TransportQuestionAnswer,
};

#[test]
fn session_state_exited() {
    let state = AgentState::Exited { status: ExitStatus { code: Some(0), signal: None } };
    assert_eq!(session_state_str(&state, 1234), "exited");
}

#[test]
fn session_state_starting_when_pid_zero() {
    assert_eq!(session_state_str(&AgentState::Starting, 0), "starting");
    assert_eq!(session_state_str(&AgentState::Working, 0), "starting");
}

#[test]
fn session_state_running_when_pid_nonzero() {
    assert_eq!(session_state_str(&AgentState::Starting, 1), "running");
    assert_eq!(session_state_str(&AgentState::Working, 42), "running");
    assert_eq!(session_state_str(&AgentState::Idle, 100), "running");
}

#[test]
fn to_domain_answers_empty() {
    let result = to_domain_answers(&[]);
    assert!(result.is_empty());
}

#[test]
fn to_domain_answers_converts_fields() {
    let input = vec![
        TransportQuestionAnswer { option: Some(1), text: None },
        TransportQuestionAnswer { option: None, text: Some("custom".to_owned()) },
        TransportQuestionAnswer { option: Some(3), text: Some("both".to_owned()) },
    ];
    let result = to_domain_answers(&input);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].option, Some(1));
    assert!(result[0].text.is_none());
    assert!(result[1].option.is_none());
    assert_eq!(result[1].text.as_deref(), Some("custom"));
    assert_eq!(result[2].option, Some(3));
    assert_eq!(result[2].text.as_deref(), Some("both"));
}

#[tokio::test]
async fn compute_health_fields() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().child_pid(1234).build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);

    let h = compute_health(&state).await;
    assert_eq!(h.status, "running");
    assert_eq!(h.pid, Some(1234));
    assert!(h.uptime_secs >= 0);
    assert_eq!(h.terminal_cols, 80);
    assert_eq!(h.terminal_rows, 24);
    assert_eq!(h.ws_clients, 0);
    assert!(h.ready);
    Ok(())
}

#[tokio::test]
async fn compute_health_pid_zero_is_none() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    let h = compute_health(&state).await;
    assert!(h.pid.is_none());
    assert!(!h.ready);
    Ok(())
}

#[tokio::test]
async fn compute_status_running() -> anyhow::Result<()> {
    let (state, _rx) =
        AppStateBuilder::new().child_pid(5678).agent_state(AgentState::Working).build();
    let st = compute_status(&state).await;
    assert_eq!(st.state, "running");
    assert_eq!(st.pid, Some(5678));
    assert!(st.uptime_secs >= 0);
    assert!(st.exit_code.is_none());
    Ok(())
}

#[tokio::test]
async fn compute_status_exited() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .child_pid(100)
        .agent_state(AgentState::Exited { status: ExitStatus { code: Some(1), signal: None } })
        .build();
    *state.driver.exit_status.write().await = Some(ExitStatus { code: Some(1), signal: None });
    let st = compute_status(&state).await;
    assert_eq!(st.state, "exited");
    assert_eq!(st.exit_code, Some(1));
    Ok(())
}

#[tokio::test]
async fn nudge_not_ready_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().nudge_encoder(Arc::new(StubNudgeEncoder)).build();
    // ready defaults to false
    let result = handle_nudge(&state, "hello").await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), crate::error::ErrorCode::NotReady);
    Ok(())
}

#[tokio::test]
async fn nudge_no_driver_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);
    let result = handle_nudge(&state, "hello").await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), crate::error::ErrorCode::NoDriver);
    Ok(())
}

#[tokio::test]
async fn nudge_busy_returns_soft_failure() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .agent_state(AgentState::Working)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);

    let result = handle_nudge(&state, "hello").await.map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(!result.delivered);
    assert_eq!(result.state_before.as_deref(), Some("working"));
    assert!(result.reason.as_deref().unwrap_or("").contains("agent is working"));
    Ok(())
}

#[tokio::test]
async fn nudge_waiting_delivers() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new()
        .agent_state(AgentState::Idle)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);

    let result = handle_nudge(&state, "hello").await.map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(result.delivered);
    assert_eq!(result.state_before.as_deref(), Some("idle"));
    assert!(result.reason.is_none());

    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(_))));
    Ok(())
}

#[tokio::test]
async fn respond_not_ready_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().respond_encoder(Arc::new(StubRespondEncoder)).build();
    let result = handle_respond(&state, None, None, None, &[]).await;
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), crate::error::ErrorCode::NotReady);
    Ok(())
}

#[tokio::test]
async fn respond_setup_prompt_delivers_option_to_pty() -> anyhow::Result<()> {
    let setup_state = AgentState::Prompt {
        prompt: crate::driver::PromptContext::new(crate::driver::PromptKind::Setup)
            .with_subtype("theme_picker")
            .with_options(vec!["Dark mode".to_owned(), "Light mode".to_owned()])
            .with_ready(),
    };
    let (state, mut rx) = AppStateBuilder::new()
        .agent_state(setup_state)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);

    let result = handle_respond(&state, None, Some(2), None, &[])
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(result.delivered);
    assert_eq!(result.prompt_type.as_deref(), Some("setup"));

    // Verify PTY received the encoded bytes ("2\r" from StubRespondEncoder).
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(data)) if data == &b"2\r"[..]));
    Ok(())
}

#[tokio::test]
async fn respond_no_prompt_returns_soft_failure() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new()
        .agent_state(AgentState::Working)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build();
    state.driver.ready.store(true, std::sync::atomic::Ordering::Release);

    let result = handle_respond(&state, Some(true), None, None, &[])
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(!result.delivered);
    assert!(result.prompt_type.is_none());
    assert_eq!(result.reason.as_deref(), Some("no prompt active"));
    Ok(())
}

#[tokio::test]
async fn input_writes_text() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    let len = handle_input(&state, "hello".to_owned(), false).await;
    assert_eq!(len, 5);
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(data)) if data == &b"hello"[..]));
    Ok(())
}

#[tokio::test]
async fn input_with_enter_appends_cr() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    let len = handle_input(&state, "hi".to_owned(), true).await;
    assert_eq!(len, 3); // "hi\r"
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(data)) if data == &b"hi\r"[..]));
    Ok(())
}

#[tokio::test]
async fn input_raw_writes_bytes() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    let len = handle_input_raw(&state, vec![0x1b, 0x5b, 0x41]).await;
    assert_eq!(len, 3);
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(data)) if data == &[0x1b, 0x5b, 0x41][..]));
    Ok(())
}

#[tokio::test]
async fn keys_valid() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    let len = handle_keys(&state, &["Enter".to_owned(), "Tab".to_owned()])
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(len, 2); // \r + \t
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Write(_))));
    Ok(())
}

#[tokio::test]
async fn keys_invalid_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    let result = handle_keys(&state, &["SuperKey".to_owned()]).await;
    assert_eq!(result.unwrap_err(), "SuperKey");
    Ok(())
}

#[tokio::test]
async fn resize_valid() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    handle_resize(&state, 120, 40).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Resize { cols: 120, rows: 40 })));
    Ok(())
}

#[tokio::test]
async fn resize_zero_cols_rejected() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    let result = handle_resize(&state, 0, 24).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn resize_zero_rows_rejected() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    let result = handle_resize(&state, 80, 0).await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn signal_valid() -> anyhow::Result<()> {
    let (state, mut rx) = AppStateBuilder::new().build();
    handle_signal(&state, "SIGINT").await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let event = rx.recv().await;
    assert!(matches!(event, Some(InputEvent::Signal(crate::event::PtySignal::Int))));
    Ok(())
}

#[tokio::test]
async fn signal_unknown_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = AppStateBuilder::new().build();
    let result = handle_signal(&state, "SIGFOO").await;
    assert_eq!(result.unwrap_err(), "SIGFOO");
    Ok(())
}
