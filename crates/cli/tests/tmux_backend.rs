// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for `TmuxBackend`.
//!
//! These tests require a working tmux installation and are `#[ignore]`-gated
//! so they don't run in CI environments without tmux.

use bytes::Bytes;
use coop::driver::ExitStatus;
use coop::pty::attach::TmuxBackend;
use coop::pty::Backend;
use std::process::Command;
use tokio::sync::mpsc;

const TEST_SESSION: &str = "coop-test-tmux-backend";

/// RAII guard that kills the tmux session on drop.
struct TmuxSession {
    name: String,
}

impl TmuxSession {
    fn new(name: &str) -> anyhow::Result<Self> {
        // Kill any leftover session from a previous failed run
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        let status = Command::new("tmux")
            .args(["new-session", "-d", "-s", name, "-x", "80", "-y", "24"])
            .status()?;
        anyhow::ensure!(status.success(), "failed to create tmux session");
        Ok(Self {
            name: name.to_string(),
        })
    }
}

impl Drop for TmuxSession {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &self.name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[ignore]
#[tokio::test]
async fn send_command_and_capture_output() -> anyhow::Result<()> {
    let _session = TmuxSession::new(TEST_SESSION)?;
    let mut backend = TmuxBackend::new(TEST_SESSION.to_string())?;

    let (output_tx, mut output_rx) = mpsc::channel::<Bytes>(16);
    let (input_tx, input_rx) = mpsc::channel::<Bytes>(16);

    let (_resize_tx, resize_rx) = mpsc::channel(4);
    let run_handle = tokio::spawn(async move { backend.run(output_tx, input_rx, resize_rx).await });

    // Send a command
    input_tx.send(Bytes::from("echo hello\r")).await?;

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
    assert!(
        result.is_ok(),
        "run future should resolve after input closes"
    );

    Ok(())
}

#[ignore]
#[tokio::test]
async fn resize_succeeds() -> anyhow::Result<()> {
    let _session = TmuxSession::new(TEST_SESSION)?;
    let backend = TmuxBackend::new(TEST_SESSION.to_string())?;

    backend.resize(100, 30)?;
    Ok(())
}

#[ignore]
#[tokio::test]
async fn child_pid_returns_valid_pid() -> anyhow::Result<()> {
    let _session = TmuxSession::new(TEST_SESSION)?;
    let backend = TmuxBackend::new(TEST_SESSION.to_string())?;

    let pid = backend.child_pid();
    assert!(pid.is_some(), "child_pid should return Some");
    assert!(pid.is_some_and(|p| p > 0), "pid should be > 0");
    Ok(())
}

#[ignore]
#[tokio::test]
async fn session_kill_resolves_run() -> anyhow::Result<()> {
    let session = TmuxSession::new(TEST_SESSION)?;
    let mut backend = TmuxBackend::new(TEST_SESSION.to_string())?;

    let (output_tx, _output_rx) = mpsc::channel::<Bytes>(16);
    let (_input_tx, input_rx) = mpsc::channel::<Bytes>(16);

    let (_resize_tx, resize_rx) = mpsc::channel(4);
    let run_handle = tokio::spawn(async move { backend.run(output_tx, input_rx, resize_rx).await });

    // Kill the session
    drop(session);

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle).await;
    assert!(
        result.is_ok(),
        "run future should resolve after session kill"
    );

    if let Ok(Ok(Ok(exit_status))) = result {
        assert_eq!(
            exit_status,
            ExitStatus {
                code: None,
                signal: None,
            }
        );
    }
    Ok(())
}
