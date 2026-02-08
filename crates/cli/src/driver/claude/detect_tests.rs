// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, Detector};

use super::{LogDetector, StdoutDetector};

#[tokio::test]
async fn log_detector_parses_lines_and_emits_states() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(
        &log_path,
        concat!(
            "{\"type\":\"system\",\"message\":{\"content\":[]}}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n",
        ),
    )?;

    let detector = Box::new(LogDetector {
        log_path: log_path.clone(),
        start_offset: 0,
    });
    assert_eq!(detector.tier(), 2);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // Wait for states to arrive
    let mut states = Vec::new();
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(state) = state_rx.recv().await {
            states.push(state.clone());
            if matches!(state, AgentState::WaitingForInput) {
                break;
            }
        }
    })
    .await;

    shutdown.cancel();
    let _ = handle.await;

    // Should have received at least Working (system) and WaitingForInput (assistant text-only)
    assert!(timeout.is_ok(), "timed out waiting for states");
    assert!(states.iter().any(|s| matches!(s, AgentState::Working)));
    assert!(states
        .iter()
        .any(|s| matches!(s, AgentState::WaitingForInput)));
    Ok(())
}

#[tokio::test]
async fn log_detector_skips_non_assistant_lines() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(
        &log_path,
        "{\"type\":\"user\",\"message\":{\"content\":[]}}\n",
    )?;

    let detector = Box::new(LogDetector {
        log_path: log_path.clone(),
        start_offset: 0,
    });
    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // User messages produce Working (not WaitingForInput)
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        state_rx.recv().await
    })
    .await;

    shutdown.cancel();
    let _ = handle.await;

    if let Ok(Some(state)) = timeout {
        assert!(matches!(state, AgentState::Working));
    }
    Ok(())
}

#[tokio::test]
async fn stdout_detector_parses_jsonl_bytes() -> anyhow::Result<()> {
    let (bytes_tx, bytes_rx) = mpsc::channel(32);
    let detector = Box::new(StdoutDetector {
        stdout_rx: bytes_rx,
    });
    assert_eq!(detector.tier(), 3);

    let (state_tx, mut state_rx) = mpsc::channel(32);
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    let handle = tokio::spawn(async move {
        detector.run(state_tx, shutdown_clone).await;
    });

    // Send a JSONL line as raw bytes
    bytes_tx
        .send(Bytes::from(
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"tool_use\",\"name\":\"Bash\",\"input\":{}}]}}\n",
        ))
        .await?;

    let state = tokio::time::timeout(std::time::Duration::from_secs(5), state_rx.recv()).await;

    shutdown.cancel();
    let _ = handle.await;

    match state {
        Ok(Some(AgentState::Working)) => {} // tool_use → Working
        other => anyhow::bail!("expected Working, got {other:?}"),
    }
    Ok(())
}
