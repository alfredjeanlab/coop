// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, NudgeEncoder, NudgeStep};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::transport::state::{AppState, WriteLock};
use crate::transport::ws::{
    handle_client_message, ClientMessage, LockAction, ServerMessage, SubscriptionMode,
};

#[test]
fn ping_pong_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Ping {};
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"ping\""));

    let pong = ServerMessage::Pong {};
    let json = serde_json::to_string(&pong).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"pong\""));
    Ok(())
}

#[test]
fn screen_request_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::ScreenRequest {};
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"screen_request\""));
    Ok(())
}

#[test]
fn output_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Output {
        data: "aGVsbG8=".to_owned(),
        offset: 0,
    };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"output\""));
    assert!(json.contains("\"data\":\"aGVsbG8=\""));
    Ok(())
}

#[test]
fn state_change_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::StateChange {
        prev: "working".to_owned(),
        next: "waiting_for_input".to_owned(),
        seq: 42,
        prompt: None,
    };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"state_change\""));
    assert!(json.contains("\"prev\":\"working\""));
    assert!(json.contains("\"next\":\"waiting_for_input\""));
    Ok(())
}

#[test]
fn lock_acquire_release_serialization() -> anyhow::Result<()> {
    let acquire = ClientMessage::Lock {
        action: LockAction::Acquire,
    };
    let json = serde_json::to_string(&acquire).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"action\":\"acquire\""));

    let release = ClientMessage::Lock {
        action: LockAction::Release,
    };
    let json = serde_json::to_string(&release).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"action\":\"release\""));
    Ok(())
}

#[test]
fn subscription_mode_default_is_all() -> anyhow::Result<()> {
    let mode: SubscriptionMode =
        serde_json::from_str("\"all\"").map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(mode, SubscriptionMode::All);
    assert_eq!(SubscriptionMode::default(), SubscriptionMode::All);
    Ok(())
}

#[test]
fn subscription_modes_deserialize() -> anyhow::Result<()> {
    let raw: SubscriptionMode =
        serde_json::from_str("\"raw\"").map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(raw, SubscriptionMode::Raw);

    let screen: SubscriptionMode =
        serde_json::from_str("\"screen\"").map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(screen, SubscriptionMode::Screen);

    let state: SubscriptionMode =
        serde_json::from_str("\"state\"").map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(state, SubscriptionMode::State);
    Ok(())
}

#[test]
fn error_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Error {
        code: "WRITER_BUSY".to_owned(),
        message: "write lock held".to_owned(),
    };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"error\""));
    assert!(json.contains("\"code\":\"WRITER_BUSY\""));
    Ok(())
}

#[test]
fn exit_message_serialization() -> anyhow::Result<()> {
    let msg = ServerMessage::Exit {
        code: Some(0),
        signal: None,
    };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"exit\""));
    assert!(json.contains("\"code\":0"));
    Ok(())
}

#[test]
fn replay_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Replay { offset: 1024 };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"replay\""));
    assert!(json.contains("\"offset\":1024"));
    Ok(())
}

#[test]
fn auth_message_serialization() -> anyhow::Result<()> {
    let msg = ClientMessage::Auth {
        token: "secret123".to_owned(),
    };
    let json = serde_json::to_string(&msg).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(json.contains("\"type\":\"auth\""));
    assert!(json.contains("\"token\":\"secret123\""));
    Ok(())
}

#[test]
fn client_message_roundtrip() -> anyhow::Result<()> {
    let messages = vec![
        r#"{"type":"input","text":"hello"}"#,
        r#"{"type":"input_raw","data":"aGVsbG8="}"#,
        r#"{"type":"keys","keys":["Enter"]}"#,
        r#"{"type":"resize","cols":200,"rows":50}"#,
        r#"{"type":"screen_request"}"#,
        r#"{"type":"state_request"}"#,
        r#"{"type":"nudge","message":"fix bug"}"#,
        r#"{"type":"respond","accept":true}"#,
        r#"{"type":"replay","offset":0}"#,
        r#"{"type":"lock","action":"acquire"}"#,
        r#"{"type":"auth","token":"tok"}"#,
        r#"{"type":"ping"}"#,
    ];

    for json in messages {
        let _msg: ClientMessage = serde_json::from_str(json)
            .map_err(|e| anyhow::anyhow!("failed to parse '{json}': {e}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Integration tests using handle_client_message
// ---------------------------------------------------------------------------

struct StubNudgeEncoder;
impl NudgeEncoder for StubNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: message.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
}

fn ws_test_state(agent: AgentState) -> (AppState, mpsc::Receiver<InputEvent>) {
    let (input_tx, input_rx) = mpsc::channel(16);
    let (output_tx, _) = broadcast::channel::<OutputEvent>(16);
    let (state_tx, _) = broadcast::channel::<StateChangeEvent>(16);

    let state = AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(4096))),
        agent_state: Arc::new(RwLock::new(agent)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(1234)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        bytes_written: AtomicU64::new(0),
        auth_token: None,
        nudge_encoder: Some(Arc::new(StubNudgeEncoder)),
        respond_encoder: None,
        shutdown: CancellationToken::new(),
        state_seq: AtomicU64::new(0),
        detection_tier: std::sync::atomic::AtomicU8::new(u8::MAX),
        idle_grace_deadline: Arc::new(std::sync::Mutex::new(None)),
        idle_grace_duration: Duration::from_secs(60),
        ring_total_written: Arc::new(AtomicU64::new(0)),
    };

    (state, input_rx)
}

#[tokio::test]
async fn resize_zero_cols_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let msg = ClientMessage::Resize { cols: 0, rows: 24 };
    let reply = handle_client_message(&state, msg, "test-client", &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "BAD_REQUEST");
        }
        other => anyhow::bail!("expected Error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn resize_zero_rows_returns_error() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let msg = ClientMessage::Resize { cols: 80, rows: 0 };
    let reply = handle_client_message(&state, msg, "test-client", &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "BAD_REQUEST");
        }
        other => anyhow::bail!("expected Error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn nudge_rejected_when_agent_working() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::Working);
    let client_id = "test-ws";
    state
        .write_lock
        .acquire_ws(client_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let msg = ClientMessage::Nudge {
        message: "hello".to_owned(),
    };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    match reply {
        Some(ServerMessage::Error { code, .. }) => {
            assert_eq!(code, "AGENT_BUSY");
        }
        other => anyhow::bail!("expected AgentBusy error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn nudge_accepted_when_agent_waiting() -> anyhow::Result<()> {
    let (state, _rx) = ws_test_state(AgentState::WaitingForInput);
    let client_id = "test-ws";
    state
        .write_lock
        .acquire_ws(client_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let msg = ClientMessage::Nudge {
        message: "hello".to_owned(),
    };
    let reply = handle_client_message(&state, msg, client_id, &mut true).await;
    assert!(reply.is_none(), "expected None (success), got {reply:?}");
    Ok(())
}
