// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::transport::ws::{ClientMessage, LockAction, ServerMessage, SubscriptionMode};

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
