// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, RwLock};

use super::*;
use crate::driver::AgentState;
use crate::ring::RingBuffer;
use crate::screen::{CursorPosition, Screen, ScreenSnapshot};

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
    let event = StateChangeEvent {
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
    let event = StateChangeEvent {
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

fn mock_app_state() -> Arc<AppState> {
    let (input_tx, _input_rx) = mpsc::channel(16);
    let (output_tx, _) = broadcast::channel(16);
    let (state_tx, _) = broadcast::channel(16);

    Arc::new(AppState {
        screen: Arc::new(RwLock::new(Screen::new(80, 24))),
        ring: Arc::new(RwLock::new(RingBuffer::new(4096))),
        input_tx,
        output_tx,
        state_tx,
        agent_state: Arc::new(RwLock::new(AgentState::Starting)),
        agent_type: "unknown".to_owned(),
        pid: Arc::new(RwLock::new(None)),
        start_time: Instant::now(),
        nudge_encoder: None,
        respond_encoder: None,
    })
}

#[test]
fn service_instantiation_compiles() {
    let state = mock_app_state();
    let service = CoopGrpc::new(state);
    // Verify we can construct a tonic server from the service
    let _router = service.into_router();
}
