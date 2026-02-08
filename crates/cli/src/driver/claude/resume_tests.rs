// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::io::Write;

use super::{discover_session_log, parse_resume_state, resume_args, ResumeState};
use crate::driver::AgentState;

#[test]
fn discover_returns_none_for_nonexistent() -> anyhow::Result<()> {
    let result = discover_session_log("/nonexistent/path")?;
    assert!(result.is_none());
    Ok(())
}

#[test]
fn discover_returns_direct_jsonl_path() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(&log_path, "{}\n")?;

    let result = discover_session_log(log_path.to_str().ok_or(anyhow::anyhow!("path"))?)?;
    assert_eq!(result, Some(log_path));
    Ok(())
}

#[test]
fn parse_empty_log_returns_starting() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    std::fs::write(&log_path, "")?;

    let state = parse_resume_state(&log_path)?;
    assert_eq!(state.last_state, AgentState::Starting);
    assert_eq!(state.log_offset, 0);
    assert!(state.conversation_id.is_none());
    Ok(())
}

#[test]
fn parse_log_with_assistant_message() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let log_path = dir.path().join("session.jsonl");
    let mut f = std::fs::File::create(&log_path)?;
    // Write a session entry with an assistant message (should yield WaitingForInput)
    writeln!(
        f,
        r#"{{"sessionId":"abc-123","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"Hello"}}]}}}}"#
    )?;

    let state = parse_resume_state(&log_path)?;
    assert_eq!(state.last_state, AgentState::WaitingForInput);
    assert_eq!(state.conversation_id, Some("abc-123".to_owned()));
    assert!(state.log_offset > 0);
    Ok(())
}

#[test]
fn resume_args_with_session_id() {
    let state = ResumeState {
        last_state: AgentState::WaitingForInput,
        log_offset: 1234,
        conversation_id: Some("sess-42".to_owned()),
    };
    let args = resume_args(&state);
    assert_eq!(args, vec!["--continue", "--session-id", "sess-42"]);
}

#[test]
fn resume_args_without_session_id() {
    let state = ResumeState {
        last_state: AgentState::Working,
        log_offset: 0,
        conversation_id: None,
    };
    let args = resume_args(&state);
    assert_eq!(args, vec!["--continue"]);
}
