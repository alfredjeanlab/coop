// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for LogWatcher file reading and HookReceiver pipe I/O.

use std::io::Write;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use coop::driver::log_watch::LogWatcher;

// ---------------------------------------------------------------------------
// log_watcher_reads_appended_lines
// ---------------------------------------------------------------------------

#[tokio::test]
async fn log_watcher_reads_appended_lines() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.log");

    // Write initial 3 lines
    {
        let mut f = std::fs::File::create(&path)?;
        writeln!(f, "line1")?;
        writeln!(f, "line2")?;
        writeln!(f, "line3")?;
    }

    let mut watcher = LogWatcher::new(path.clone());
    let lines = watcher.read_new_lines()?;
    assert_eq!(lines, vec!["line1", "line2", "line3"]);

    // Append 2 more lines
    {
        let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
        writeln!(f, "line4")?;
        writeln!(f, "line5")?;
    }

    let lines2 = watcher.read_new_lines()?;
    assert_eq!(lines2, vec!["line4", "line5"]);

    Ok(())
}

// ---------------------------------------------------------------------------
// log_watcher_run_receives_batches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn log_watcher_run_receives_batches() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.log");

    // Create the file
    std::fs::File::create(&path)?;

    let watcher = LogWatcher::new(path.clone());
    let (line_tx, mut line_rx) = mpsc::channel(16);
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();

    tokio::spawn(async move {
        watcher.run(line_tx, sd).await;
    });

    // Append lines after a short delay
    let p = path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).ok();
        if let Some(ref mut f) = f {
            let _ = writeln!(f, "batch_line1");
            let _ = writeln!(f, "batch_line2");
        }
    });

    // Wait for the batch to arrive
    let batch = tokio::time::timeout(Duration::from_secs(10), line_rx.recv()).await?;
    let batch = batch.ok_or_else(|| anyhow::anyhow!("expected batch"))?;
    assert!(
        batch.contains(&"batch_line1".to_string()),
        "batch: {batch:?}"
    );

    shutdown.cancel();
    Ok(())
}

// ---------------------------------------------------------------------------
// log_watcher_handles_file_created_late
// ---------------------------------------------------------------------------

#[tokio::test]
async fn log_watcher_handles_file_created_late() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("late.log");

    // File doesn't exist yet
    let mut watcher = LogWatcher::new(path.clone());
    let lines = watcher.read_new_lines()?;
    assert!(lines.is_empty(), "should be empty for nonexistent file");

    // Create file and write
    {
        let mut f = std::fs::File::create(&path)?;
        writeln!(f, "appeared")?;
    }

    let lines2 = watcher.read_new_lines()?;
    assert_eq!(lines2, vec!["appeared"]);

    Ok(())
}

// ---------------------------------------------------------------------------
// log_watcher_offset_survives_reopen
// ---------------------------------------------------------------------------

#[tokio::test]
async fn log_watcher_offset_survives_reopen() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("offset.log");

    // Write initial lines
    {
        let mut f = std::fs::File::create(&path)?;
        writeln!(f, "first")?;
        writeln!(f, "second")?;
    }

    let mut watcher = LogWatcher::new(path.clone());
    let lines = watcher.read_new_lines()?;
    assert_eq!(lines, vec!["first", "second"]);

    let offset = watcher.offset();

    // Create new watcher with the saved offset
    let mut watcher2 = LogWatcher::with_offset(path.clone(), offset);

    // Append more lines
    {
        let mut f = std::fs::OpenOptions::new().append(true).open(&path)?;
        writeln!(f, "third")?;
    }

    let lines2 = watcher2.read_new_lines()?;
    assert_eq!(lines2, vec!["third"]);

    Ok(())
}

// ---------------------------------------------------------------------------
// hook_receiver_reads_events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_receiver_reads_events() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook.pipe");

    let mut receiver = coop::driver::hook_recv::HookReceiver::new(&pipe_path)?;

    // Write events from a spawned task
    let pp = pipe_path.clone();
    tokio::spawn(async move {
        // Open the FIFO for writing (this will block until reader opens)
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&pp) {
            let _ = writeln!(f, r#"{{"event":"post_tool_use","tool":"bash"}}"#);
            let _ = writeln!(f, r#"{{"event":"stop"}}"#);
        }
    });

    // Read events
    let event1 = tokio::time::timeout(Duration::from_secs(5), receiver.next_event()).await?;
    match event1 {
        Some(coop::event::HookEvent::ToolComplete { tool }) => {
            assert_eq!(tool, "bash");
        }
        other => anyhow::bail!("expected ToolComplete, got {other:?}"),
    }

    let event2 = tokio::time::timeout(Duration::from_secs(5), receiver.next_event()).await?;
    match event2 {
        Some(coop::event::HookEvent::AgentStop) => {}
        other => anyhow::bail!("expected AgentStop, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// hook_receiver_skips_malformed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_receiver_skips_malformed() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook_malformed.pipe");

    let mut receiver = coop::driver::hook_recv::HookReceiver::new(&pipe_path)?;

    let pp = pipe_path.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&pp) {
            let _ = writeln!(f, "not json at all");
            let _ = writeln!(f, r#"{{"invalid":"object"}}"#);
            let _ = writeln!(f, r#"{{"event":"stop"}}"#);
        }
    });

    // Should skip malformed lines and return the valid event
    let event = tokio::time::timeout(Duration::from_secs(5), receiver.next_event()).await?;
    match event {
        Some(coop::event::HookEvent::AgentStop) => {}
        other => anyhow::bail!("expected AgentStop after skipping malformed, got {other:?}"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// hook_receiver_cleanup_removes_pipe
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hook_receiver_cleanup_removes_pipe() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook_cleanup.pipe");

    {
        let _receiver = coop::driver::hook_recv::HookReceiver::new(&pipe_path)?;
        assert!(pipe_path.exists(), "pipe should exist while receiver lives");
    }

    // After drop, the pipe should be removed
    assert!(
        !pipe_path.exists(),
        "pipe should be removed after receiver drop"
    );

    Ok(())
}
