// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for `TmuxBackend`.
//!
//! Each test spins up an isolated tmux server via `-S <tmpdir>/tmux.sock`
//! so tests run in parallel without colliding with each other or the
//! user's default tmux.

use bytes::Bytes;
use coop::driver::ExitStatus;
use coop::pty::adapter::TmuxBackend;
use coop::pty::{Backend, BackendInput};
use coop::screen;
use std::path::PathBuf;
use std::process::Command;
use tokio::sync::mpsc;

/// RAII guard that manages an isolated tmux server + session in a temp dir.
///
/// On drop, kills the tmux server (which destroys all sessions) and
/// cleans up the temp directory.
struct TmuxSession {
    name: String,
    socket: PathBuf,
    _tmpdir: tempfile::TempDir,
}

impl TmuxSession {
    fn new(name: &str) -> anyhow::Result<Self> {
        let tmpdir = tempfile::tempdir()?;
        let socket = tmpdir.path().join("tmux.sock");

        let status = Command::new("tmux")
            .args(["-S"])
            .arg(&socket)
            .args([
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                &screen::DEFAULT_COLS.to_string(),
                "-y",
                &screen::DEFAULT_ROWS.to_string(),
            ])
            .status()?;
        anyhow::ensure!(status.success(), "failed to create tmux session");

        Ok(Self { name: name.to_string(), socket, _tmpdir: tmpdir })
    }

    fn backend(&self) -> anyhow::Result<TmuxBackend> {
        TmuxBackend::with_socket(self.name.clone(), Some(self.socket.clone()))
    }
}

impl Drop for TmuxSession {
    fn drop(&mut self) {
        // Kill the entire server — cleans up all sessions on this socket
        let _ = Command::new("tmux")
            .args(["-S"])
            .arg(&self.socket)
            .args(["kill-server"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[tokio::test]
async fn send_command_and_capture_output() -> anyhow::Result<()> {
    let session = TmuxSession::new("test")?;
    let mut backend = session.backend()?;

    let (output_tx, mut output_rx) = mpsc::channel::<Bytes>(16);
    let (input_tx, input_rx) = mpsc::channel::<BackendInput>(16);

    let (_resize_tx, resize_rx) = mpsc::channel(4);
    let run_handle = tokio::spawn(async move { backend.run(output_tx, input_rx, resize_rx).await });

    // Send a command
    input_tx.send(BackendInput::Write(Bytes::from("echo hello\r"))).await?;

    // Wait for output containing "hello"
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(std::time::Duration::from_secs(2), output_rx.recv()).await {
            Ok(Some(data)) => {
                let text = String::from_utf8_lossy(&data);
                if text.contains("hello") {
                    found = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(found, "expected output containing 'hello'");

    drop(input_tx);
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), run_handle).await;
    assert!(result.is_ok(), "run future should resolve after input closes");

    Ok(())
}

#[tokio::test]
async fn resize_succeeds() -> anyhow::Result<()> {
    let session = TmuxSession::new("test")?;
    let backend = session.backend()?;

    backend.resize(100, 30)?;
    Ok(())
}

#[tokio::test]
async fn child_pid_returns_valid_pid() -> anyhow::Result<()> {
    let session = TmuxSession::new("test")?;
    let backend = session.backend()?;

    let pid = backend.child_pid();
    assert!(pid.is_some(), "child_pid should return Some");
    assert!(pid.is_some_and(|p| p > 0), "pid should be > 0");
    Ok(())
}

#[tokio::test]
async fn session_kill_resolves_run() -> anyhow::Result<()> {
    let session = TmuxSession::new("test")?;
    let mut backend = session.backend()?;

    let (output_tx, _output_rx) = mpsc::channel::<Bytes>(16);
    let (_input_tx, input_rx) = mpsc::channel::<BackendInput>(16);

    let (_resize_tx, resize_rx) = mpsc::channel(4);
    let run_handle = tokio::spawn(async move { backend.run(output_tx, input_rx, resize_rx).await });

    // Kill the server (simulates session going away)
    drop(session);

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle).await;
    assert!(result.is_ok(), "run future should resolve after session kill");

    if let Ok(Ok(Ok(exit_status))) = result {
        assert_eq!(exit_status, ExitStatus { code: None, signal: None });
    }
    Ok(())
}
