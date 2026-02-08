// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use coop::pty::spawn::NativePty;
use coop::pty::Backend;
use coop::ring::RingBuffer;
use coop::screen::Screen;
use tokio::sync::mpsc;

#[tokio::test]
async fn spawn_and_capture() {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);

    let mut pty = NativePty::spawn(&["echo".into(), "hello".into()], 80, 24).expect("spawn failed");

    assert!(pty.child_pid().is_some());

    let status = pty.run(output_tx, input_rx).await.expect("run failed");
    assert_eq!(status.code, Some(0));
    assert_eq!(status.signal, None);

    let mut output = Vec::new();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&output);
    assert!(
        text.contains("hello"),
        "expected 'hello' in output: {text:?}"
    );
}

#[tokio::test]
async fn input_delivery() {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (input_tx, input_rx) = mpsc::channel(64);

    let mut pty = NativePty::spawn(&["/bin/cat".into()], 80, 24).expect("spawn failed");

    let handle = tokio::spawn(async move { pty.run(output_tx, input_rx).await });

    // Write data with newline, then Ctrl-D on empty line to signal EOF
    input_tx
        .send(Bytes::from_static(b"ping\n"))
        .await
        .expect("send failed");
    // Short delay so cat processes the line before we send EOF
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    input_tx
        .send(Bytes::from_static(b"\x04"))
        .await
        .expect("send eof failed");
    drop(input_tx);

    let status = handle.await.expect("join").expect("run");
    assert_eq!(status.code, Some(0));

    let mut output = Vec::new();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend_from_slice(&chunk);
    }
    let text = String::from_utf8_lossy(&output);
    assert!(text.contains("ping"), "expected 'ping' in output: {text:?}");
}

#[tokio::test]
async fn resize_no_error() {
    let pty = NativePty::spawn(&["/bin/sh".into(), "-c".into(), "sleep 0.1".into()], 80, 24)
        .expect("spawn failed");

    pty.resize(40, 10).expect("resize failed");
}

#[tokio::test]
async fn screen_integration() {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);

    let mut pty = NativePty::spawn(&["echo".into(), "hello".into()], 80, 24).expect("spawn failed");
    let _ = pty.run(output_tx, input_rx).await;

    let mut screen = Screen::new(80, 24);
    while let Ok(chunk) = output_rx.try_recv() {
        screen.feed(&chunk);
    }

    let snap = screen.snapshot();
    let joined = snap.lines.join("\n");
    assert!(
        joined.contains("hello"),
        "expected 'hello' in screen: {joined:?}"
    );
}

#[tokio::test]
async fn ring_buffer_integration() {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let (_input_tx, input_rx) = mpsc::channel(64);

    let mut pty = NativePty::spawn(&["echo".into(), "hello".into()], 80, 24).expect("spawn failed");
    let _ = pty.run(output_tx, input_rx).await;

    let mut ring = RingBuffer::new(4096);
    while let Ok(chunk) = output_rx.try_recv() {
        ring.write(&chunk);
    }

    assert!(ring.total_written() > 0);
    let (a, b) = ring.read_from(0).expect("should be readable");
    let mut data = a.to_vec();
    data.extend_from_slice(b);
    let text = String::from_utf8_lossy(&data);
    assert!(
        text.contains("hello"),
        "expected 'hello' in ring buffer: {text:?}"
    );
}
