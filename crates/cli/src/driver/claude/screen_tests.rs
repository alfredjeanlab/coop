// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::driver::{AgentState, PromptKind};
use crate::screen::{CursorPosition, ScreenSnapshot, DEFAULT_COLS, DEFAULT_ROWS};

use super::classify_claude_screen;

fn snapshot(lines: &[&str]) -> ScreenSnapshot {
    ScreenSnapshot {
        lines: lines.iter().map(|s| s.to_string()).collect(),
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    }
}

/// Extract just the state from the classify result for simple assertions.
fn state(snap: &ScreenSnapshot) -> Option<AgentState> {
    classify_claude_screen(snap).map(|(s, _)| s)
}

/// Extract the cause string from the classify result.
fn cause(snap: &ScreenSnapshot) -> Option<String> {
    classify_claude_screen(snap).map(|(_, c)| c)
}

#[test]
fn detects_idle_prompt() {
    let snap = snapshot(&["Claude Code v2.1.37", "", "\u{276f} Try \"fix lint errors\"", ""]);
    assert_eq!(state(&snap), Some(AgentState::Idle));
    assert_eq!(cause(&snap).as_deref(), Some("screen:idle"));
}

#[test]
fn no_idle_on_empty_screen() {
    let snap = snapshot(&["", "", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn no_idle_on_working_output() {
    let snap = snapshot(&["Reading file src/main.rs...", "Analyzing code...", ""]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn startup_trust_prompt_emits_setup() {
    let snap = snapshot(&["Do you trust the files in this folder?", "(y/n)", ""]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("startup_trust"));
    assert!(prompt.options.is_empty(), "text prompts have no parsed options");
    assert_eq!(c, "screen:setup");
}

#[test]
fn startup_bypass_prompt_emits_setup() {
    let snap = snapshot(&["Allow tool use without prompting?", "dangerously-skip-permissions", ""]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("startup_bypass"));
    assert_eq!(c, "screen:setup");
}

#[test]
fn startup_login_prompt_emits_setup() {
    let snap = snapshot(&["Please sign in to continue", ""]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("startup_login"));
    assert_eq!(c, "screen:setup");
}

#[test]
fn settings_error_emits_setup() {
    let lines: Vec<String> =
        include_str!("fixtures/bad_settings.screen.txt").lines().map(String::from).collect();
    let snap = ScreenSnapshot {
        lines,
        cols: 200,
        rows: 50,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    };
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("settings_error"));
    assert!(!prompt.options.is_empty(), "should parse numbered options");
    assert_eq!(c, "screen:setup");
}

#[test]
fn detects_bare_prompt() {
    let snap = snapshot(&["\u{276f} ", ""]);
    assert_eq!(state(&snap), Some(AgentState::Idle));
}

#[test]
fn workspace_trust_dialog_emits_permission() {
    let snap = snapshot(&[
        " Accessing workspace:",
        " /Users/kestred/Developer/foo",
        "",
        " \u{276f} 1. Yes, I trust this folder",
        "   2. No, exit",
        "",
        " Enter to confirm \u{00b7} Esc to cancel",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Permission);
    assert_eq!(prompt.subtype.as_deref(), Some("trust"));
    assert!(!prompt.options.is_empty(), "should parse options");
    assert_eq!(c, "screen:permission");
}

#[test]
fn theme_picker_emits_setup() {
    let snap = snapshot(&[
        " Choose the text style that looks best with your terminal",
        "",
        " \u{276f} 1. Dark mode \u{2714}",
        "   2. Light mode",
        "",
        " Enter to confirm",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("theme_picker"));
    assert!(!prompt.options.is_empty(), "should parse options");
    assert_eq!(c, "screen:setup");
}

#[test]
fn tool_permission_dialog_still_suppressed() {
    let snap = snapshot(&[
        " Bash command",
        "",
        "   coop send '{\"status\":\"done\",\"message\":\"Said goodbye as requested.\"}'",
        "   Signal done to coop stop hook",
        "",
        " Do you want to proceed?",
        " \u{276f} 1. Yes",
        "   2. Yes, and don't ask again for coop send commands in /Users/kestred/Developer/coop",
        "   3. No",
        "",
        " Esc to cancel \u{00b7} Tab to amend \u{00b7} ctrl+e to explain",
    ]);
    assert_eq!(classify_claude_screen(&snap), None);
}

#[test]
fn security_notes_emits_setup() {
    let snap = snapshot(&[
        " Security notes:",
        "",
        " Claude can make mistakes. Review tool use requests carefully.",
        "",
        " Press Enter to continue...",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("security_notes"));
    assert_eq!(c, "screen:setup");
}

#[test]
fn login_success_emits_setup() {
    let snap = snapshot(&[
        " Login successful. Press Enter to continue...",
        "",
        " Logged in as user@example.com",
    ]);
    let (s, _) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("login_success"));
}

#[test]
fn oauth_login_extracts_auth_url_single_line() {
    let snap = snapshot(&[
        "",
        " Paste code here if prompted",
        "",
        "https://claude.ai/oauth/authorize?client_id=abc&state=xyz",
        "",
    ]);
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("oauth_login"));
    assert_eq!(
        prompt.auth_url.as_deref(),
        Some("https://claude.ai/oauth/authorize?client_id=abc&state=xyz")
    );
    assert_eq!(c, "screen:setup");
}

#[test]
fn oauth_login_extracts_wrapped_auth_url() {
    // Real Claude wraps the URL across multiple terminal lines.
    let snap = snapshot(&[
        " Browser didn't open? Use the url below to sign in (c to copy)",
        "",
        "https://claude.ai/oauth/authorize?code=true&client_id=9d1c&redirect_uri=",
        "https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user",
        "%3Asessions&state=BwPX",
        "",
        " Paste code here if prompted >",
    ]);
    let (s, _) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.subtype.as_deref(), Some("oauth_login"));
    assert_eq!(
        prompt.auth_url.as_deref(),
        Some("https://claude.ai/oauth/authorize?code=true&client_id=9d1c&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Asessions&state=BwPX")
    );
}

#[test]
fn oauth_login_extracts_platform_domain_auth_url() {
    let snap = snapshot(&[
        " Browser didn't open? Use the url below to sign in (c to copy)",
        "",
        "https://platform.claude.com/oauth/authorize?code=true&client_id=9d1c",
        "",
        " Paste code here if prompted >",
    ]);
    let (s, _) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.subtype.as_deref(), Some("oauth_login"));
    assert_eq!(
        prompt.auth_url.as_deref(),
        Some("https://platform.claude.com/oauth/authorize?code=true&client_id=9d1c")
    );
}

#[test]
fn oauth_login_no_url_has_none_auth_url() {
    let snap =
        snapshot(&[" Paste code here if prompted", " Please visit this URL: oauth/authorize"]);
    let (s, _) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.subtype.as_deref(), Some("oauth_login"));
    assert!(prompt.auth_url.is_none());
}

#[test]
fn detects_prompt_with_status_text_below() {
    let snap = snapshot(&[
        "Claude Code v2.1.37",
        "",
        "\u{276f}\u{00a0}Try \"create a util logging.py that...\"",
        "────────────────────────────────",
        "  ctrl+t to hide tasks",
    ]);
    assert_eq!(state(&snap), Some(AgentState::Idle));
}

#[test]
fn plan_context_returns_plan_kind() {
    use super::extract_plan_context;

    let screen = ScreenSnapshot {
        lines: vec![
            "Plan: Implement auth system".to_string(),
            "Step 1: Add middleware".to_string(),
            "[y] Accept  [n] Reject".to_string(),
        ],
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        alt_screen: false,
        cursor: CursorPosition { row: 2, col: 0 },
        sequence: 42,
    };
    let ctx = extract_plan_context(&screen);
    assert_eq!(ctx.kind, crate::driver::PromptKind::Plan);
    assert!(ctx.auth_url.is_none());
}

use super::parse_options_from_screen;

/// Helper: load a fixture file and split into screen lines.
fn fixture_lines(text: &str) -> Vec<String> {
    text.lines().map(String::from).collect()
}

/// Bash permission dialog (from bash_permission_dialog.tui.txt)
#[test]
fn parse_options_bash_permission() {
    let lines = fixture_lines(include_str!("fixtures/bash_permission.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, and always allow access to tmp/ from this project", "No"]);
}

/// Edit permission dialog (from edit_permission_dialog.tui.txt)
#[test]
fn parse_options_edit_permission() {
    let lines = fixture_lines(include_str!("fixtures/edit_permission.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, allow all edits during this session (shift+tab)", "No"]);
}

/// Trust folder / Bash permission dialog (from trust_folder_dialog.tui.txt)
#[test]
fn parse_options_trust_folder() {
    let lines = fixture_lines(include_str!("fixtures/trust_folder.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "Yes, allow reading from Downloads/ from this project", "No"]);
}

/// Thinking dialog (from thinking_dialog_disabled_selected.tui.txt)
#[test]
fn parse_options_thinking_dialog() {
    let lines = fixture_lines(include_str!("fixtures/thinking_dialog.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(
        opts,
        vec![
            "Enabled ✔  Claude will think before responding",
            "Disabled   Claude will respond without extended thinking",
        ]
    );
}

/// Multi-question dialog Q1 (from multi_question_dialog_q1.tui.txt)
/// Options are split across a separator line, with description lines under each.
#[test]
fn parse_options_multi_question_dialog() {
    let lines = fixture_lines(include_str!("fixtures/multi_question_q1.screen.txt"));
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Rust", "Python", "Type something.", "Chat about this"]);
}

/// Non-breaking space after ❯ (Claude uses U+00A0 in practice)
#[test]
fn parse_options_nbsp_after_selector() {
    let lines =
        vec![" Do you want to proceed?".into(), " ❯\u{00A0}1. Yes".into(), "   2. No".into()];
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Yes", "No"]);
}

#[test]
fn parse_options_empty_screen() {
    let opts = parse_options_from_screen(&[]);
    assert!(opts.is_empty());
}

/// Theme picker with trailing checkmark on selected option
#[test]
fn parse_options_strips_trailing_checkmark() {
    let lines = vec![
        " Choose the text style".into(),
        " ❯ 1. Dark mode ✔".into(),
        "   2. Light mode".into(),
        "   3. Light mode (high contrast)".into(),
        " Enter to confirm · Esc to exit".into(),
    ];
    let opts = parse_options_from_screen(&lines);
    assert_eq!(opts, vec!["Dark mode", "Light mode", "Light mode (high contrast)"]);
}

#[test]
fn parse_options_no_match() {
    let lines = vec!["Working on your task...".into(), "Reading files".into()];
    let opts = parse_options_from_screen(&lines);
    assert!(opts.is_empty());
}

/// Login success fixture (real TUI screen with ASCII art)
#[test]
fn login_success_fixture_emits_setup() {
    let lines = fixture_lines(include_str!("fixtures/login_success.screen.txt"));
    let snap = ScreenSnapshot {
        lines,
        cols: 200,
        rows: 50,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    };
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("login_success"));
    assert!(prompt.options.is_empty(), "press-Enter screens have no numbered options");
    assert_eq!(c, "screen:setup");
}

/// Security notes fixture (real TUI screen with ASCII art)
#[test]
fn security_notes_fixture_emits_setup() {
    let lines = fixture_lines(include_str!("fixtures/security_notes.screen.txt"));
    let snap = ScreenSnapshot {
        lines,
        cols: 200,
        rows: 50,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    };
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Setup);
    assert_eq!(prompt.subtype.as_deref(), Some("security_notes"));
    assert!(prompt.options.is_empty(), "press-Enter screens have no numbered options");
    assert_eq!(c, "screen:setup");
}

/// Accessing workspace fixture (real TUI trust dialog with numbered options)
#[test]
fn accessing_workspace_fixture_emits_permission() {
    let lines = fixture_lines(include_str!("fixtures/accessing_workspace.screen.txt"));
    let snap = ScreenSnapshot {
        lines,
        cols: 200,
        rows: 50,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 1,
    };
    let (s, c) = classify_claude_screen(&snap).expect("should emit state");
    let prompt = s.prompt().expect("should be Prompt");
    assert_eq!(prompt.kind, PromptKind::Permission);
    assert_eq!(prompt.subtype.as_deref(), Some("trust"));
    assert_eq!(prompt.options, vec!["Yes, I trust this folder", "No, exit"]);
    assert_eq!(c, "screen:permission");
}
