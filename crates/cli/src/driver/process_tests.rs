// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{is_process_alive, AgentState, Detector, ProcessMonitor};

#[test]
fn tier_returns_4() {
    let monitor = ProcessMonitor::new(Arc::new(|| None), Arc::new(|| 0));
    assert_eq!(monitor.tier(), 4);
}

#[test]
fn is_process_alive_returns_true_for_self() {
    let pid = std::process::id();
    assert!(is_process_alive(pid));
}

#[test]
fn is_process_alive_returns_false_for_bogus_pid() {
    assert!(!is_process_alive(999_999_999));
}

#[tokio::test]
async fn emits_exited_when_pid_dead() -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();

    let monitor = ProcessMonitor::new(Arc::new(|| Some(999_999_999)), Arc::new(|| 0))
        .with_poll_interval(Duration::from_millis(10));

    let handle = tokio::spawn(Box::new(monitor).run(tx, shutdown.clone()));

    let state = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await?;
    assert!(matches!(state, Some(AgentState::Exited { .. })));

    handle.await?;
    Ok(())
}

#[tokio::test]
async fn shutdown_cancels_monitor() -> anyhow::Result<()> {
    let (tx, _rx) = mpsc::channel(8);
    let shutdown = CancellationToken::new();

    let monitor = ProcessMonitor::new(
        Arc::new(|| {
            let pid = std::process::id();
            Some(pid)
        }),
        Arc::new(|| 0),
    )
    .with_poll_interval(Duration::from_millis(10));

    let shutdown_clone = shutdown.clone();
    let handle = tokio::spawn(Box::new(monitor).run(tx, shutdown_clone));

    // Give the monitor a tick then cancel
    tokio::time::sleep(Duration::from_millis(50)).await;
    shutdown.cancel();

    tokio::time::timeout(Duration::from_secs(2), handle).await??;
    Ok(())
}
