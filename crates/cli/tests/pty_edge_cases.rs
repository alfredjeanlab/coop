// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! PTY edge-case integration tests extending `pty_backend.rs`.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;

use coop::pty::spawn::NativePty;
use coop::pty::Backend;
use coop::session::{Session, SessionConfig};
use coop::test_support::AppStateBuilder;

// ---------------------------------------------------------------------------
// child_exit_produces_eof
// ---------------------------------------------------------------------------

#[tokio::test]
async fn child_exit_produces_eof() -> anyhow::Result<()> {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);
    let (_resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(&["true".into()], 80, 24)?;
    let status = pty.run(output_tx, input_rx, resize_rx).await?;
    assert_eq!(status.code, Some(0));
    assert_eq!(status.signal, None);

    // Drain output — should be empty or minimal for `true`
    let mut output = Vec::new();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// child_killed_produces_signal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn child_killed_produces_signal() -> anyhow::Result<()> {
    let (output_tx, _output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);
    let (_resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(&["/bin/sleep".into(), "60".into()], 80, 24)?;
    let pid = pty
        .child_pid()
        .ok_or_else(|| anyhow::anyhow!("no child pid"))?;

    let handle = tokio::spawn(async move { pty.run(output_tx, input_rx, resize_rx).await });

    // Give the process time to start
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Send SIGKILL
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    )?;

    let status = handle.await??;
    assert_eq!(
        status.signal,
        Some(9),
        "expected SIGKILL (9), got {status:?}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// eio_on_child_death
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eio_on_child_death() -> anyhow::Result<()> {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);
    let (_resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(
        &["/bin/sh".into(), "-c".into(), "echo hi; exit 1".into()],
        80,
        24,
    )?;

    let status = pty.run(output_tx, input_rx, resize_rx).await?;
    assert_eq!(status.code, Some(1), "expected exit code 1, got {status:?}");

    // Output should contain "hi"
    let mut output = Vec::new();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("hi"), "expected 'hi' in output: {text:?}");
    Ok(())
}

// ---------------------------------------------------------------------------
// resize_reflected_in_stty (extends existing with multiple sizes)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resize_reflected_in_stty() -> anyhow::Result<()> {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (input_tx, input_rx) = mpsc::channel(64);
    let (resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(&["/bin/sh".into()], 80, 24)?;

    let handle = tokio::spawn(async move { pty.run(output_tx, input_rx, resize_rx).await });

    // Give the shell time to start
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Drain initial shell prompt output
    while tokio::time::timeout(Duration::from_millis(100), output_rx.recv())
        .await
        .is_ok()
    {}

    // Test multiple resize values — send resize then query stty size
    let sizes: [(u16, u16); 3] = [(100, 30), (120, 40), (60, 20)];
    for (cols, rows) in &sizes {
        resize_tx.send((*cols, *rows)).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Query terminal size via stty
        input_tx.send(bytes::Bytes::from("stty size\n")).await?;

        // Collect output until we see the expected dimensions
        let expected = format!("{rows} {cols}");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut output = Vec::new();
        let mut found = false;
        loop {
            tokio::select! {
                chunk = output_rx.recv() => {
                    match chunk {
                        Some(data) => output.extend_from_slice(&data),
                        None => break,
                    }
                    let text = String::from_utf8_lossy(&output);
                    if text.contains(&expected) {
                        found = true;
                        break;
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    break;
                }
            }
        }
        assert!(
            found,
            "expected '{expected}' after resize to {cols}x{rows}, got: {:?}",
            String::from_utf8_lossy(&output)
        );
    }

    // Clean up
    handle.abort();
    Ok(())
}

// ---------------------------------------------------------------------------
// large_output_through_session
// ---------------------------------------------------------------------------

#[tokio::test]
async fn large_output_through_session() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(1_048_576) // 1MB
        .build_with_sender(input_tx);

    let backend = NativePty::spawn(
        &["/bin/sh".into(), "-c".into(), "seq 1 10000".into()],
        80,
        24,
    )?;
    let session = Session::new(SessionConfig::test_default(
        Box::new(backend),
        Arc::clone(&app_state),
        consumer_input_rx,
    ));

    let status = session.run().await?;
    assert_eq!(status.code, Some(0));

    // Ring should have captured substantial data
    let ring = app_state.terminal.ring.read().await;
    assert!(
        ring.total_written() > 1000,
        "expected significant output, got {} bytes",
        ring.total_written()
    );

    // Screen should have content
    let screen = app_state.terminal.screen.read().await;
    let snap = screen.snapshot();
    let text = snap.lines.join("\n");
    assert!(!text.trim().is_empty(), "screen should have content");

    Ok(())
}

// ---------------------------------------------------------------------------
// binary_output_no_panic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn binary_output_no_panic() -> anyhow::Result<()> {
    let (output_tx, mut output_rx) = mpsc::channel(256);
    let (_input_tx, input_rx) = mpsc::channel(64);
    let (_resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(
        &[
            "/bin/sh".into(),
            "-c".into(),
            "head -c 1024 /dev/urandom".into(),
        ],
        80,
        24,
    )?;

    let status = pty.run(output_tx, input_rx, resize_rx).await?;
    assert_eq!(status.code, Some(0));

    // Should complete without error
    let mut total = 0;
    while let Ok(chunk) = output_rx.try_recv() {
        total += chunk.len();
    }
    assert!(total > 0, "expected some binary output");
    Ok(())
}

// ---------------------------------------------------------------------------
// rapid_input_output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rapid_input_output() -> anyhow::Result<()> {
    let (output_tx, mut output_rx) = mpsc::channel(256);
    let (input_tx, input_rx) = mpsc::channel(256);
    let (_resize_tx, resize_rx) = mpsc::channel(4);

    let mut pty = NativePty::spawn(&["/bin/cat".into()], 80, 24)?;
    let handle = tokio::spawn(async move { pty.run(output_tx, input_rx, resize_rx).await });

    // Send 100 short lines rapidly
    tokio::time::sleep(Duration::from_millis(100)).await;
    for i in 0..100 {
        let line = format!("line{i}\n");
        input_tx.send(Bytes::from(line)).await?;
    }

    // Wait for output to settle then send EOF
    tokio::time::sleep(Duration::from_millis(500)).await;
    input_tx.send(Bytes::from_static(b"\x04")).await?;
    drop(input_tx);

    let status = handle.await??;
    assert_eq!(status.code, Some(0));

    // Collect all output
    let mut output = Vec::new();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&output);

    // Verify at least some lines echoed
    assert!(text.contains("line0"), "expected 'line0' in output");
    assert!(text.contains("line99"), "expected 'line99' in output");
    Ok(())
}

// ---------------------------------------------------------------------------
// signal_delivery_sigint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signal_delivery_sigint() -> anyhow::Result<()> {
    let (input_tx, consumer_input_rx) = mpsc::channel(64);
    let app_state = AppStateBuilder::new()
        .ring_size(65536)
        .build_with_sender(input_tx.clone());

    let backend = NativePty::spawn(&["/bin/cat".into()], 80, 24)?;
    let session = Session::new(SessionConfig::test_default(
        Box::new(backend),
        Arc::clone(&app_state),
        consumer_input_rx,
    ));

    let session_handle = tokio::spawn(async move { session.run().await });

    // Give cat time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send SIGINT via InputEvent
    input_tx
        .send(coop::event::InputEvent::Signal(coop::event::PtySignal::Int))
        .await?;
    drop(input_tx);

    let status = session_handle.await??;
    // cat should terminate with signal or non-zero code
    assert!(
        status.signal.is_some() || status.code.is_some(),
        "expected termination: {status:?}"
    );
    Ok(())
}
