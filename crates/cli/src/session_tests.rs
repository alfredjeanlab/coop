// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::{Config, GroomLevel};
use crate::driver::{AgentState, PromptContext, PromptKind};
use crate::pty::spawn::NativePty;
use crate::session::{Session, SessionConfig};
use crate::test_support::{AppStateBuilder, MockDetector, MockPty, StubRespondEncoder};

#[tokio::test]
async fn echo_exits_with_zero() -> anyhow::Result<()> {
    let config = Config::test();
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);

    let backend = NativePty::spawn(
        &["echo".into(), "hello".into()],
        crate::DEFAULT_TERM_COLS,
        crate::DEFAULT_TERM_ROWS,
        &[],
    )?;
    let session = Session::new(&config, SessionConfig::new(app_state, backend, consumer_input_rx));

    let status = session.run(&config).await?;
    assert_eq!(status.code, Some(0));
    Ok(())
}

#[tokio::test]
async fn output_captured_in_ring_and_screen() -> anyhow::Result<()> {
    let config = Config::test();
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);

    let backend = NativePty::spawn(
        &["echo".into(), "hello-ring".into()],
        crate::DEFAULT_TERM_COLS,
        crate::DEFAULT_TERM_ROWS,
        &[],
    )?;
    let session = Session::new(
        &config,
        SessionConfig::new(Arc::clone(&app_state), backend, consumer_input_rx),
    );

    let _ = session.run(&config).await?;

    // Check ring buffer
    let ring = app_state.terminal.ring.read().await;
    assert!(ring.total_written() > 0);
    let (a, b) = ring.read_from(0).ok_or(anyhow::anyhow!("no data"))?;
    let mut data = a.to_vec();
    data.extend_from_slice(b);
    let text = String::from_utf8_lossy(&data);
    assert!(text.contains("hello-ring"), "ring: {text:?}");

    // Check screen
    let screen = app_state.terminal.screen.read().await;
    let snap = screen.snapshot();
    let lines = snap.lines.join("\n");
    assert!(lines.contains("hello-ring"), "screen: {lines:?}");

    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_session() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(0);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);
    let shutdown = CancellationToken::new();

    // Long-running command
    let backend = NativePty::spawn(
        &["/bin/sh".into(), "-c".into(), "sleep 60".into()],
        crate::DEFAULT_TERM_COLS,
        crate::DEFAULT_TERM_ROWS,
        &[],
    )?;
    let session = Session::new(
        &config,
        SessionConfig::new(app_state, backend, consumer_input_rx).with_shutdown(shutdown.clone()),
    );

    // Cancel after a short delay
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        shutdown.cancel();
    });

    let status = session.run(&config).await?;
    // Should have exited (signal or timeout)
    assert!(status.code.is_some() || status.signal.is_some(), "expected exit: {status:?}");
    Ok(())
}

/// Agent is already idle when shutdown fires → immediate SIGHUP (no drain wait).
#[tokio::test]
async fn graceful_drain_kills_when_already_idle() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(2000);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);
    let shutdown = CancellationToken::new();

    let backend = MockPty::new().drain_input();
    // Detector emits Idle almost immediately.
    let detector = MockDetector::new(1, vec![(Duration::from_millis(10), AgentState::Idle)]);

    let session = Session::new(
        &config,
        SessionConfig::new(app_state, backend, consumer_input_rx)
            .with_shutdown(shutdown.clone())
            .with_detectors(vec![Box::new(detector)]),
    );

    // Cancel after the detector has fired (10ms) + margin.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        sd.cancel();
    });

    let start = std::time::Instant::now();
    let _ = session.run(&config).await?;
    let elapsed = start.elapsed();

    // Should exit promptly (not wait for the 2s graceful timeout).
    assert!(elapsed < Duration::from_secs(1), "expected quick exit, took {elapsed:?}");
    Ok(())
}

/// Agent never reaches idle → drain deadline force-kills after timeout.
#[tokio::test]
async fn graceful_drain_timeout_force_kills() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(500);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);
    let shutdown = CancellationToken::new();

    let backend = MockPty::new().drain_input();
    let captured = backend.captured_input();

    let session = Session::new(
        &config,
        SessionConfig::new(app_state, backend, consumer_input_rx).with_shutdown(shutdown.clone()),
    );

    // Cancel after a short delay; no detector → state stays Starting → drain entered.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        sd.cancel();
    });

    let start = std::time::Instant::now();
    let _ = session.run(&config).await?;
    let elapsed = start.elapsed();

    // Should wait for the ~500ms drain deadline, not exit immediately.
    assert!(elapsed >= Duration::from_millis(300), "exited too fast: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(3), "took too long: {elapsed:?}");

    // Verify Escape bytes were sent during drain.
    let input = captured.lock();
    let has_escape = input.iter().any(|b| b.as_ref() == b"\x1b");
    assert!(has_escape, "expected Escape bytes in captured input: {input:?}");

    Ok(())
}

/// drain_timeout_ms=0 disables drain → immediate SIGHUP like pre-feature.
#[tokio::test]
async fn graceful_drain_disabled_when_zero() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(0);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .build_with_sender(input_tx);
    let shutdown = CancellationToken::new();

    let backend = MockPty::new().drain_input();
    let captured = backend.captured_input();

    let session = Session::new(
        &config,
        SessionConfig::new(app_state, backend, consumer_input_rx).with_shutdown(shutdown.clone()),
    );

    let sd = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        sd.cancel();
    });

    let start = std::time::Instant::now();
    let _ = session.run(&config).await?;
    let elapsed = start.elapsed();

    // Should exit immediately (no drain mode).
    assert!(elapsed < Duration::from_secs(2), "expected quick exit, took {elapsed:?}");

    // No Escape bytes should have been sent.
    let input = captured.lock();
    let has_escape = input.iter().any(|b| b.as_ref() == b"\x1b");
    assert!(!has_escape, "unexpected Escape bytes in captured input: {input:?}");

    Ok(())
}

fn disruption_prompt() -> AgentState {
    AgentState::Prompt {
        prompt: PromptContext::new(PromptKind::Setup)
            .with_subtype("theme_picker")
            .with_options(vec!["Dark mode".to_owned(), "Light mode".to_owned()])
            .with_ready(),
    }
}

/// groom=Auto auto-dismisses disruption prompts.
#[tokio::test]
async fn groom_auto_dismisses_disruption() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(0);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .groom(GroomLevel::Auto)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build_with_sender(input_tx);

    let backend = MockPty::new().drain_input();
    let captured = backend.captured_input();

    // Detector emits a disruption prompt, then after a delay triggers shutdown.
    let detector = MockDetector::new(1, vec![(Duration::from_millis(10), disruption_prompt())]);

    let shutdown = CancellationToken::new();
    let session = Session::new(
        &config,
        SessionConfig::new(Arc::clone(&app_state), backend, consumer_input_rx)
            .with_shutdown(shutdown.clone())
            .with_detectors(vec![Box::new(detector)]),
    );

    // Give enough time for the 500ms auto-dismiss delay + delivery, then shut down.
    let sd = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        sd.cancel();
    });

    let _ = session.run(&config).await?;

    // The auto-dismiss should have delivered keystrokes to the backend.
    let input = captured.lock();
    assert!(!input.is_empty(), "expected auto-dismiss keystrokes, got none");
    // StubRespondEncoder.encode_setup(1) → "1\r"
    let has_setup_response = input.iter().any(|b| b.as_ref() == b"1\r");
    assert!(has_setup_response, "expected '1\\r' in captured input: {input:?}");

    Ok(())
}

/// groom=Manual does NOT auto-dismiss disruption prompts.
#[tokio::test]
async fn groom_manual_does_not_dismiss() -> anyhow::Result<()> {
    let mut config = Config::test();
    config.drain_timeout_ms = Some(0);
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(crate::test_support::TEST_RING_SIZE)
        .groom(GroomLevel::Manual)
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build_with_sender(input_tx);

    let backend = MockPty::new().drain_input();
    let captured = backend.captured_input();

    let detector = MockDetector::new(1, vec![(Duration::from_millis(10), disruption_prompt())]);

    let shutdown = CancellationToken::new();
    let session = Session::new(
        &config,
        SessionConfig::new(Arc::clone(&app_state), backend, consumer_input_rx)
            .with_shutdown(shutdown.clone())
            .with_detectors(vec![Box::new(detector)]),
    );

    let sd = shutdown.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        sd.cancel();
    });

    let _ = session.run(&config).await?;

    // No auto-dismiss keystrokes should have been sent.
    let input = captured.lock();
    let has_setup_response = input.iter().any(|b| b.as_ref() == b"1\r");
    assert!(!has_setup_response, "unexpected auto-dismiss in manual mode: {input:?}");

    Ok(())
}
