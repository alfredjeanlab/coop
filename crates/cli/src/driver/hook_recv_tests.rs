// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::event::HookEvent;

use super::{parse_hook_line, HookReceiver};

#[test]
fn parses_tool_complete_event() {
    let event = parse_hook_line(r#"{"event":"post_tool_use","tool":"Bash"}"#);
    assert_eq!(
        event,
        Some(HookEvent::ToolComplete {
            tool: "Bash".to_string()
        })
    );
}

#[test]
fn parses_stop_event() {
    let event = parse_hook_line(r#"{"event":"stop"}"#);
    assert_eq!(event, Some(HookEvent::AgentStop));
}

#[test]
fn parses_session_end_event() {
    let event = parse_hook_line(r#"{"event":"session_end"}"#);
    assert_eq!(event, Some(HookEvent::SessionEnd));
}

#[test]
fn ignores_malformed_lines() {
    assert_eq!(parse_hook_line("not json"), None);
    assert_eq!(parse_hook_line("{}"), None);
    assert_eq!(parse_hook_line(r#"{"event":"unknown_event"}"#), None);
    assert_eq!(parse_hook_line(""), None);
}

#[test]
fn creates_pipe_and_cleans_up() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("test.pipe");

    {
        let recv = HookReceiver::new(&pipe_path)?;
        assert!(pipe_path.exists());
        assert_eq!(recv.pipe_path(), pipe_path);
    }
    // Drop should remove the pipe
    assert!(!pipe_path.exists());
    Ok(())
}

#[tokio::test]
async fn reads_event_from_pipe() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = dir.path().join("hook.pipe");

    let mut recv = HookReceiver::new(&pipe_path)?;

    // Write to the pipe from another task
    let pipe = pipe_path.clone();
    tokio::spawn(async move {
        // Small delay to let the reader open first
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Open for write explicitly (tokio::fs::write uses create+truncate which
        // doesn't work on FIFOs)
        let mut file = match tokio::fs::OpenOptions::new().write(true).open(&pipe).await {
            Ok(f) => f,
            Err(_) => return,
        };
        use tokio::io::AsyncWriteExt;
        let _ = file.write_all(b"{\"event\":\"stop\"}\n").await;
    });

    let event = recv.next_event().await;
    assert_eq!(event, Some(HookEvent::AgentStop));
    Ok(())
}
