// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::AgentState;
use crate::pty::spawn::NativePty;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::session::{Session, SessionConfig};
use crate::transport::state::WriteLock;
use crate::transport::AppState;

fn make_app_state(input_tx: mpsc::Sender<crate::event::InputEvent>) -> Arc<AppState> {
    let (output_tx, _) = broadcast::channel(256);
    let (state_tx, _) = broadcast::channel(64);

    Arc::new(AppState {
        started_at: Instant::now(),
        agent_type: "unknown".to_owned(),
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(65536))),
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        input_tx,
        output_tx,
        state_tx,
        child_pid: Arc::new(AtomicU32::new(0)),
        exit_status: Arc::new(RwLock::new(None)),
        write_lock: Arc::new(WriteLock::new()),
        ws_client_count: Arc::new(AtomicI32::new(0)),
        bytes_written: AtomicU64::new(0),
        auth_token: None,
        nudge_encoder: None,
        respond_encoder: None,
        shutdown: CancellationToken::new(),
    })
}

#[tokio::test]
async fn echo_exits_with_zero() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["echo".into(), "hello".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state,
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    let status = session.run().await?;
    assert_eq!(status.code, Some(0));
    Ok(())
}

#[tokio::test]
async fn output_captured_in_ring_and_screen() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    let backend = NativePty::spawn(&["echo".into(), "hello-ring".into()], 80, 24)?;
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state: Arc::clone(&app_state),
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown,
    });

    let _ = session.run().await?;

    // Check ring buffer
    let ring = app_state.ring.read().await;
    assert!(ring.total_written() > 0);
    let (a, b) = ring.read_from(0).ok_or(anyhow::anyhow!("no data"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);
    let text = String::from_utf8_lossy(&data);
    assert!(text.contains("hello-ring"), "ring: {text:?}");

    // Check screen
    let screen = app_state.screen.read().await;
    let snap = screen.snapshot();
    let lines = snap.lines.join("\n");
    assert!(lines.contains("hello-ring"), "screen: {lines:?}");

    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_session() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = make_app_state(input_tx);
    let shutdown = CancellationToken::new();

    // Long-running command
    let backend = NativePty::spawn(&["/bin/sh".into(), "-c".into(), "sleep 60".into()], 80, 24)?;
    let sd = shutdown.clone();
    let session = Session::new(SessionConfig {
        backend: Box::new(backend),
        detectors: vec![],
        app_state,
        consumer_input_rx,
        cols: 80,
        rows: 24,
        shutdown: sd,
    });

    // Cancel after a short delay
    let cancel_shutdown = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancel_shutdown.cancel();
    });

    let status = session.run().await?;
    // Should have exited (signal or timeout)
    assert!(
        status.code.is_some() || status.signal.is_some(),
        "expected exit: {status:?}"
    );
    Ok(())
}
