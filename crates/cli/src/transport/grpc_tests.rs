// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use super::*;
use crate::driver::AgentState;
use crate::screen::{CursorPosition, ScreenSnapshot};
use crate::test_support::{AnyhowExt, TestAppStateBuilder};
use crate::transport::encode_key;

// ---------------------------------------------------------------------------
// Type conversion tests
// ---------------------------------------------------------------------------

#[test]
fn cursor_to_proto_converts_u16_to_i32() {
    let cursor = CursorPosition { row: 5, col: 42 };
    let p = cursor_to_proto(&cursor);
    assert_eq!(p.row, 5);
    assert_eq!(p.col, 42);
}

#[test]
fn cursor_to_proto_handles_max_u16() {
    let cursor = CursorPosition {
        row: u16::MAX,
        col: u16::MAX,
    };
    let p = cursor_to_proto(&cursor);
    assert_eq!(p.row, u16::MAX as i32);
    assert_eq!(p.col, u16::MAX as i32);
}

#[test]
fn screen_snapshot_to_proto_converts_all_fields() {
    let snap = ScreenSnapshot {
        lines: vec!["hello".to_owned(), "world".to_owned()],
        cols: 80,
        rows: 24,
        alt_screen: true,
        cursor: CursorPosition { row: 1, col: 5 },
        sequence: 42,
    };
    let p = screen_snapshot_to_proto(&snap);
    assert_eq!(p.lines, vec!["hello", "world"]);
    assert_eq!(p.cols, 80);
    assert_eq!(p.rows, 24);
    assert!(p.alt_screen);
    assert_eq!(p.sequence, 42);
    let cursor = p.cursor.as_ref();
    assert!(cursor.is_some());
    let c = cursor.ok_or("missing cursor").map_err(|e| e.to_string());
    assert!(c.is_ok());
}

#[test]
fn screen_snapshot_to_response_omits_cursor() {
    let snap = ScreenSnapshot {
        lines: vec![],
        cols: 40,
        rows: 10,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    };
    let resp = screen_snapshot_to_response(&snap, false);
    assert!(resp.cursor.is_none());
    assert_eq!(resp.cols, 40);
}

#[test]
fn screen_snapshot_to_response_includes_cursor() {
    let snap = ScreenSnapshot {
        lines: vec![],
        cols: 40,
        rows: 10,
        alt_screen: false,
        cursor: CursorPosition { row: 3, col: 7 },
        sequence: 1,
    };
    let resp = screen_snapshot_to_response(&snap, true);
    assert!(resp.cursor.is_some());
    let c = resp.cursor.as_ref();
    assert!(c.is_some());
}

#[test]
fn prompt_to_proto_converts_all_fields() {
    let prompt = crate::driver::PromptContext {
        prompt_type: "permission".to_owned(),
        tool: Some("bash".to_owned()),
        input_preview: Some("rm -rf /".to_owned()),
        question: Some("Allow?".to_owned()),
        options: vec!["yes".to_owned(), "no".to_owned()],
        summary: Some("dangerous command".to_owned()),
        screen_lines: vec!["$ rm -rf /".to_owned()],
    };
    let p = prompt_to_proto(&prompt);
    assert_eq!(p.r#type, "permission");
    assert_eq!(p.tool.as_deref(), Some("bash"));
    assert_eq!(p.input_preview.as_deref(), Some("rm -rf /"));
    assert_eq!(p.question.as_deref(), Some("Allow?"));
    assert_eq!(p.options, vec!["yes", "no"]);
    assert_eq!(p.summary.as_deref(), Some("dangerous command"));
    assert_eq!(p.screen_lines, vec!["$ rm -rf /"]);
}

#[test]
fn prompt_to_proto_handles_none_fields() {
    let prompt = crate::driver::PromptContext {
        prompt_type: "ask_user".to_owned(),
        tool: None,
        input_preview: None,
        question: None,
        options: vec![],
        summary: None,
        screen_lines: vec![],
    };
    let p = prompt_to_proto(&prompt);
    assert_eq!(p.r#type, "ask_user");
    assert!(p.tool.is_none());
    assert!(p.input_preview.is_none());
    assert!(p.question.is_none());
    assert!(p.options.is_empty());
    assert!(p.summary.is_none());
    assert!(p.screen_lines.is_empty());
}

#[test]
fn state_change_to_proto_converts_simple_transition() {
    let event = crate::event::StateChangeEvent {
        prev: AgentState::Starting,
        next: AgentState::Working,
        seq: 7,
    };
    let p = state_change_to_proto(&event);
    assert_eq!(p.prev, "starting");
    assert_eq!(p.next, "working");
    assert_eq!(p.seq, 7);
    assert!(p.prompt.is_none());
}

#[test]
fn state_change_to_proto_includes_prompt() {
    let prompt = crate::driver::PromptContext {
        prompt_type: "permission".to_owned(),
        tool: Some("write".to_owned()),
        input_preview: None,
        question: None,
        options: vec![],
        summary: None,
        screen_lines: vec![],
    };
    let event = crate::event::StateChangeEvent {
        prev: AgentState::Working,
        next: AgentState::PermissionPrompt {
            prompt: prompt.clone(),
        },
        seq: 10,
    };
    let p = state_change_to_proto(&event);
    assert_eq!(p.next, "permission_prompt");
    assert!(p.prompt.is_some());
    let pp = p.prompt.as_ref();
    assert!(pp.is_some());
}

// ---------------------------------------------------------------------------
// Key encoding tests
// ---------------------------------------------------------------------------

#[yare::parameterized(
    enter = { "Enter", b"\r" as &[u8] },
    return_key = { "Return", b"\r" },
    tab = { "Tab", b"\t" },
    escape = { "Escape", b"\x1b" },
    esc = { "Esc", b"\x1b" },
    backspace = { "Backspace", b"\x7f" },
    delete = { "Delete", b"\x1b[3~" },
    up = { "Up", b"\x1b[A" },
    down = { "Down", b"\x1b[B" },
    right = { "Right", b"\x1b[C" },
    left = { "Left", b"\x1b[D" },
    home = { "Home", b"\x1b[H" },
    end = { "End", b"\x1b[F" },
    pageup = { "PageUp", b"\x1b[5~" },
    pagedown = { "PageDown", b"\x1b[6~" },
    space = { "Space", b" " },
    ctrl_c = { "Ctrl-C", b"\x03" },
    ctrl_d = { "Ctrl-D", b"\x04" },
    ctrl_z = { "Ctrl-Z", b"\x1a" },
    f1 = { "F1", b"\x1bOP" },
    f12 = { "F12", b"\x1b[24~" },
)]
fn encode_key_known(name: &str, expected: &[u8]) {
    let result = encode_key(name);
    assert!(result.is_some(), "encode_key({name:?}) returned None");
    assert_eq!(result.as_deref(), Some(expected));
}

#[test]
fn encode_key_unknown_returns_none() {
    assert!(encode_key("SuperKey").is_none());
    assert!(encode_key("").is_none());
    assert!(encode_key("Ctrl-?").is_none());
}

#[test]
fn encode_key_case_insensitive() {
    assert_eq!(encode_key("enter"), encode_key("ENTER"));
    assert_eq!(encode_key("ctrl-c"), encode_key("Ctrl-C"));
}

// ---------------------------------------------------------------------------
// Signal parsing tests
// ---------------------------------------------------------------------------

#[yare::parameterized(
    sigint = { "SIGINT", 2 },
    int = { "INT", 2 },
    bare_2 = { "2", 2 },
    sigterm = { "SIGTERM", 15 },
    term = { "TERM", 15 },
    bare_15 = { "15", 15 },
    sighup = { "SIGHUP", 1 },
    sigkill = { "SIGKILL", 9 },
    sigusr1 = { "SIGUSR1", 10 },
    sigusr2 = { "SIGUSR2", 12 },
    sigcont = { "SIGCONT", 18 },
    sigstop = { "SIGSTOP", 19 },
    sigtstp = { "SIGTSTP", 20 },
    sigwinch = { "SIGWINCH", 28 },
)]
fn parse_signal_known(name: &str, expected: i32) {
    let result = parse_signal(name);
    assert_eq!(result, Some(expected), "parse_signal({name:?})");
}

#[test]
fn parse_signal_unknown_returns_none() {
    assert!(parse_signal("SIGFOO").is_none());
    assert!(parse_signal("").is_none());
    assert!(parse_signal("99").is_none());
}

#[test]
fn parse_signal_case_insensitive() {
    assert_eq!(parse_signal("sigint"), Some(2));
    assert_eq!(parse_signal("int"), Some(2));
}

// ---------------------------------------------------------------------------
// Service instantiation (compile-time type chain test)
// ---------------------------------------------------------------------------

#[test]
fn service_instantiation_compiles() {
    let state = TestAppStateBuilder::new().build().0;
    let service = CoopGrpc::new(state);
    // Verify we can construct a tonic server from the service
    let _router = service.into_router();
}

// ---------------------------------------------------------------------------
// Write lock tests for nudge/respond
// ---------------------------------------------------------------------------

use crate::driver::{NudgeEncoder, NudgeStep, RespondEncoder};
use tonic::Code;

struct StubNudgeEncoder;
impl NudgeEncoder for StubNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: message.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
}

struct StubRespondEncoder;
impl RespondEncoder for StubRespondEncoder {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep> {
        let text = if accept { "y" } else { "n" };
        vec![NudgeStep {
            bytes: text.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
    fn encode_plan(&self, accept: bool, _text: Option<&str>) -> Vec<NudgeStep> {
        self.encode_permission(accept)
    }
    fn encode_question(&self, _option: Option<u32>, text: Option<&str>) -> Vec<NudgeStep> {
        let t = text.unwrap_or("1");
        vec![NudgeStep {
            bytes: t.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
}

fn mock_app_state_with_encoders(agent: AgentState) -> Arc<AppState> {
    let (state, _rx) = TestAppStateBuilder::new()
        .agent_state(agent)
        .nudge_encoder(Arc::new(StubNudgeEncoder))
        .respond_encoder(Arc::new(StubRespondEncoder))
        .build();
    state
        .ready
        .store(true, std::sync::atomic::Ordering::Release);
    state
}

#[tokio::test]
async fn nudge_fails_when_write_lock_held() -> anyhow::Result<()> {
    use proto::coop_server::Coop;

    let state = mock_app_state_with_encoders(AgentState::WaitingForInput);
    // Simulate another client holding the write lock
    state.write_lock.acquire_ws("other-client").anyhow()?;

    let service = CoopGrpc::new(state);
    let req = Request::new(proto::NudgeRequest {
        message: "hello".to_owned(),
    });
    let result = service.nudge(req).await;
    let err = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected error"))?;
    assert_eq!(err.code(), Code::ResourceExhausted);
    Ok(())
}

#[tokio::test]
async fn respond_fails_when_write_lock_held() -> anyhow::Result<()> {
    use proto::coop_server::Coop;

    let prompt = crate::driver::PromptContext {
        prompt_type: "permission".to_owned(),
        tool: None,
        input_preview: None,
        question: None,
        options: vec![],
        summary: None,
        screen_lines: vec![],
    };
    let state = mock_app_state_with_encoders(AgentState::PermissionPrompt { prompt });
    state.write_lock.acquire_ws("other-client").anyhow()?;

    let service = CoopGrpc::new(state);
    let req = Request::new(proto::RespondRequest {
        accept: Some(true),
        option: None,
        text: None,
    });
    let result = service.respond(req).await;
    let err = result
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected error"))?;
    assert_eq!(err.code(), Code::ResourceExhausted);
    Ok(())
}
