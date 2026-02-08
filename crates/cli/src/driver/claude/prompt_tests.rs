// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use crate::screen::{CursorPosition, ScreenSnapshot};

use super::{extract_ask_user_context, extract_permission_context, extract_plan_context};

#[test]
fn permission_context_extracts_tool_and_preview() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "npm install express" }
            }]
        }
    });
    let ctx = extract_permission_context(&entry);
    assert_eq!(ctx.prompt_type, "permission");
    assert_eq!(ctx.tool.as_deref(), Some("Bash"));
    assert!(ctx.input_preview.is_some());
    let preview = ctx.input_preview.as_deref().unwrap_or("");
    assert!(preview.contains("npm install express"));
}

#[test]
fn permission_context_truncates_large_input() {
    let long_text = "x".repeat(500);
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Write",
                "input": { "content": long_text }
            }]
        }
    });
    let ctx = extract_permission_context(&entry);
    let preview = ctx.input_preview.unwrap_or_default();
    assert!(preview.len() <= 210); // 200 + "..."
    assert!(preview.ends_with("..."));
}

#[test]
fn ask_user_context_extracts_question_and_options() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {
            "questions": [{
                "question": "Which database should we use?",
                "options": [
                    { "label": "PostgreSQL", "description": "Relational" },
                    { "label": "SQLite", "description": "Embedded" },
                    { "label": "MySQL", "description": "Popular" }
                ]
            }]
        }
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.prompt_type, "question");
    assert_eq!(
        ctx.question.as_deref(),
        Some("Which database should we use?")
    );
    assert_eq!(ctx.options, vec!["PostgreSQL", "SQLite", "MySQL"]);
}

#[test]
fn ask_user_context_with_no_options() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {
            "questions": [{
                "question": "What do you want to do?"
            }]
        }
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.question.as_deref(), Some("What do you want to do?"));
    assert!(ctx.options.is_empty());
}

#[test]
fn ask_user_context_with_empty_input() {
    let block = json!({
        "type": "tool_use",
        "name": "AskUserQuestion",
        "input": {}
    });
    let ctx = extract_ask_user_context(&block);
    assert_eq!(ctx.prompt_type, "question");
    assert!(ctx.question.is_none());
    assert!(ctx.options.is_empty());
}

#[test]
fn plan_context_captures_screen_lines() {
    let screen = ScreenSnapshot {
        lines: vec![
            "Plan: Implement auth system".to_string(),
            "Step 1: Add middleware".to_string(),
            "[y] Accept  [n] Reject".to_string(),
        ],
        cols: 80,
        rows: 24,
        alt_screen: false,
        cursor: CursorPosition { row: 2, col: 0 },
        sequence: 42,
    };
    let ctx = extract_plan_context(&screen);
    assert_eq!(ctx.prompt_type, "plan");
    assert_eq!(ctx.screen_lines.len(), 3);
    assert_eq!(ctx.screen_lines[0], "Plan: Implement auth system");
}

#[test]
fn missing_fields_produce_sensible_defaults() {
    // Permission context with no message field
    let entry = json!({});
    let ctx = extract_permission_context(&entry);
    assert_eq!(ctx.prompt_type, "permission");
    assert!(ctx.tool.is_none());
    assert!(ctx.input_preview.is_none());

    // AskUser context with no input field
    let block = json!({ "type": "tool_use", "name": "AskUserQuestion" });
    let ctx = extract_ask_user_context(&block);
    assert!(ctx.question.is_none());
    assert!(ctx.options.is_empty());
}
