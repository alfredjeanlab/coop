// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! gRPC integration tests using a tonic client against an in-process server.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio_stream::StreamExt;

use coop::driver::AgentState;
use coop::event::{OutputEvent, StateChangeEvent};
use coop::test_support::{spawn_grpc_server, AppStateBuilder, StubNudgeEncoder};
use coop::transport::grpc::proto;

// ---------------------------------------------------------------------------
// Helper: create a gRPC client connected to the test server
// ---------------------------------------------------------------------------

async fn grpc_client(
    app_state: Arc<coop::transport::AppState>,
) -> anyhow::Result<(
    proto::coop_client::CoopClient<tonic::transport::Channel>,
    Arc<coop::transport::AppState>,
)> {
    let (addr, _handle) = spawn_grpc_server(Arc::clone(&app_state)).await?;
    // Brief pause for the server to start accepting
    tokio::time::sleep(Duration::from_millis(50)).await;
    let channel = tonic::transport::Channel::from_shared(format!("http://{addr}"))
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .connect()
        .await
        .map_err(|e| anyhow::anyhow!("grpc connect: {e}"))?;
    Ok((proto::coop_client::CoopClient::new(channel), app_state))
}

// ---------------------------------------------------------------------------
// grpc_get_health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_get_health() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().child_pid(42).build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .get_health(proto::GetHealthRequest {})
        .await?
        .into_inner();
    assert_eq!(resp.status, "ok");
    assert_eq!(resp.pid, Some(42));
    assert_eq!(resp.agent, "unknown");
    assert_eq!(resp.ws_clients, 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_get_screen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_get_screen() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    // TEXT format
    let resp = client
        .get_screen(proto::GetScreenRequest {
            format: 0, // TEXT
            include_cursor: true,
        })
        .await?
        .into_inner();
    assert_eq!(resp.cols, 80);
    assert_eq!(resp.rows, 24);
    assert!(!resp.alt_screen);
    assert!(resp.cursor.is_some());

    // Without cursor
    let resp2 = client
        .get_screen(proto::GetScreenRequest {
            format: 0,
            include_cursor: false,
        })
        .await?
        .into_inner();
    assert!(resp2.cursor.is_none());

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_get_status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_get_status() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .get_status(proto::GetStatusRequest {})
        .await?
        .into_inner();
    assert_eq!(resp.state, "starting");
    assert_eq!(resp.ws_clients, 0);
    assert_eq!(resp.bytes_written, 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_send_input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_send_input() -> anyhow::Result<()> {
    let (app_state, mut input_rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .send_input(proto::SendInputRequest {
            text: "hello".to_owned(),
            enter: true,
        })
        .await?
        .into_inner();
    assert_eq!(resp.bytes_written, 6); // "hello" + "\r"

    // Verify input event received
    let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await?;
    match event {
        Some(coop::event::InputEvent::Write(data)) => {
            assert_eq!(&data[..], b"hello\r");
        }
        other => anyhow::bail!("expected Write event, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_send_keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_send_keys() -> anyhow::Result<()> {
    let (app_state, mut input_rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .send_keys(proto::SendKeysRequest {
            keys: vec!["Enter".to_owned(), "Tab".to_owned()],
        })
        .await?
        .into_inner();
    assert_eq!(resp.bytes_written, 2); // \r + \t

    let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await?;
    match event {
        Some(coop::event::InputEvent::Write(data)) => {
            assert_eq!(&data[..], b"\r\t");
        }
        other => anyhow::bail!("expected Write event with keys, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_resize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_resize() -> anyhow::Result<()> {
    let (app_state, mut input_rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .resize(proto::ResizeRequest {
            cols: 120,
            rows: 40,
        })
        .await?
        .into_inner();
    assert_eq!(resp.cols, 120);
    assert_eq!(resp.rows, 40);

    let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await?;
    match event {
        Some(coop::event::InputEvent::Resize { cols, rows }) => {
            assert_eq!(cols, 120);
            assert_eq!(rows, 40);
        }
        other => anyhow::bail!("expected Resize event, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_send_signal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_send_signal() -> anyhow::Result<()> {
    let (app_state, mut input_rx) = AppStateBuilder::new().build();
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .send_signal(proto::SendSignalRequest {
            signal: "SIGINT".to_owned(),
        })
        .await?
        .into_inner();
    assert!(resp.delivered);

    let event = tokio::time::timeout(Duration::from_secs(2), input_rx.recv()).await?;
    assert!(
        matches!(
            event,
            Some(coop::event::InputEvent::Signal(coop::event::PtySignal::Int))
        ),
        "expected Signal(Int), got {event:?}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_stream_output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_stream_output() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().ring_size(65536).build();
    let (mut client, state) = grpc_client(app_state).await?;

    let mut stream = client
        .stream_output(proto::StreamOutputRequest { from_offset: 0 })
        .await?
        .into_inner();

    // Write data to ring + broadcast
    let data = bytes::Bytes::from("stream-test-data");
    {
        let mut ring = state.terminal.ring.write().await;
        ring.write(&data);
    }
    let _ = state
        .channels
        .output_tx
        .send(OutputEvent::Raw(data.clone()));

    // Read from stream
    let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("stream ended"))?
        .map_err(|e| anyhow::anyhow!("stream error: {e}"))?;

    assert_eq!(&chunk.data, b"stream-test-data");

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_stream_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_stream_state() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (mut client, state) = grpc_client(app_state).await?;

    let mut stream = client
        .stream_state(proto::StreamStateRequest {})
        .await?
        .into_inner();

    // Send state change
    let _ = state.channels.state_tx.send(StateChangeEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 1,
    });

    let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("stream ended"))?
        .map_err(|e| anyhow::anyhow!("stream error: {e}"))?;

    assert_eq!(event.prev, "starting");
    assert_eq!(event.next, "working");
    assert_eq!(event.seq, 1);

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_stream_screen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_stream_screen() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    let (mut client, state) = grpc_client(app_state).await?;

    let mut stream = client
        .stream_screen(proto::StreamScreenRequest {})
        .await?
        .into_inner();

    // Push screen update
    let _ = state
        .channels
        .output_tx
        .send(OutputEvent::ScreenUpdate { seq: 7 });

    let snap = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await?
        .ok_or_else(|| anyhow::anyhow!("stream ended"))?
        .map_err(|e| anyhow::anyhow!("stream error: {e}"))?;

    assert_eq!(snap.cols, 80);
    assert_eq!(snap.rows, 24);

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_error_codes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_nudge_not_ready() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new()
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .build();
    // NOT marking ready
    let (mut client, _state) = grpc_client(app_state).await?;

    let result = client
        .nudge(proto::NudgeRequest {
            message: "hello".to_owned(),
        })
        .await;
    let err = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected error"))?;
    assert_eq!(err.code(), tonic::Code::Unavailable);

    Ok(())
}

#[tokio::test]
async fn grpc_nudge_no_driver() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new().build();
    app_state.ready.store(true, Ordering::Release);
    let (mut client, _state) = grpc_client(app_state).await?;

    let result = client
        .nudge(proto::NudgeRequest {
            message: "hello".to_owned(),
        })
        .await;
    let err = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected error"))?;
    assert_eq!(err.code(), tonic::Code::Unimplemented);

    Ok(())
}

#[tokio::test]
async fn grpc_nudge_agent_busy() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new()
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .agent_state(AgentState::Working)
        .build();
    app_state.ready.store(true, Ordering::Release);
    let (mut client, _state) = grpc_client(app_state).await?;

    let resp = client
        .nudge(proto::NudgeRequest {
            message: "hello".to_owned(),
        })
        .await?
        .into_inner();
    assert!(!resp.delivered);
    assert!(resp.reason.is_some());

    Ok(())
}

// ---------------------------------------------------------------------------
// grpc_nudge_write_lock_conflict
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grpc_nudge_write_lock_conflict() -> anyhow::Result<()> {
    let (app_state, _rx) = AppStateBuilder::new()
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .agent_state(AgentState::WaitingForInput)
        .build();
    app_state.ready.store(true, Ordering::Release);

    // Simulate WS client holding the write lock
    app_state
        .lifecycle
        .write_lock
        .acquire_ws("ws-holder")
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let (mut client, _state) = grpc_client(app_state).await?;

    let result = client
        .nudge(proto::NudgeRequest {
            message: "hello".to_owned(),
        })
        .await;
    let err = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected error"))?;
    assert_eq!(err.code(), tonic::Code::ResourceExhausted);

    Ok(())
}
