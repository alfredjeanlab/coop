// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use serde_json::json;

use crate::driver::AgentState;

use super::parse_claude_state;

#[test]
fn error_field_produces_error_state() {
    let entry = json!({ "error": "rate_limit_exceeded" });
    let state = parse_claude_state(&entry);
    assert_eq!(
        state,
        Some(AgentState::Error {
            detail: "rate_limit_exceeded".to_string()
        })
    );
}

#[test]
fn error_field_non_string_uses_unknown() {
    let entry = json!({ "error": 42 });
    let state = parse_claude_state(&entry);
    assert_eq!(
        state,
        Some(AgentState::Error {
            detail: "unknown".to_string()
        })
    );
}

#[test]
fn system_message_produces_working() {
    let entry = json!({
        "type": "system",
        "message": { "content": [] }
    });
    assert_eq!(parse_claude_state(&entry), Some(AgentState::Working));
}

#[test]
fn user_message_produces_working() {
    let entry = json!({
        "type": "user",
        "message": { "content": [{ "type": "text", "text": "hello" }] }
    });
    assert_eq!(parse_claude_state(&entry), Some(AgentState::Working));
}

#[test]
fn assistant_with_tool_use_produces_working() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "Bash",
                "input": { "command": "ls" }
            }]
        }
    });
    assert_eq!(parse_claude_state(&entry), Some(AgentState::Working));
}

#[test]
fn assistant_with_thinking_produces_working() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "thinking",
                "thinking": "Let me consider..."
            }]
        }
    });
    assert_eq!(parse_claude_state(&entry), Some(AgentState::Working));
}

#[test]
fn assistant_with_ask_user_produces_ask_user() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "tool_use",
                "name": "AskUserQuestion",
                "input": {
                    "questions": [{
                        "question": "Which database?",
                        "options": [
                            { "label": "PostgreSQL" },
                            { "label": "SQLite" }
                        ]
                    }]
                }
            }]
        }
    });
    let state = parse_claude_state(&entry);
    match state {
        Some(AgentState::AskUser { prompt }) => {
            assert_eq!(prompt.prompt_type, "question");
            assert_eq!(prompt.question.as_deref(), Some("Which database?"));
            assert_eq!(prompt.options, vec!["PostgreSQL", "SQLite"]);
        }
        other => panic!("expected AskUser, got {other:?}"),
    }
}

#[test]
fn assistant_with_only_text_produces_waiting() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": [{
                "type": "text",
                "text": "Here is the result."
            }]
        }
    });
    assert_eq!(
        parse_claude_state(&entry),
        Some(AgentState::WaitingForInput)
    );
}

#[test]
fn malformed_json_missing_message_returns_none() {
    let entry = json!({
        "type": "assistant"
    });
    assert_eq!(parse_claude_state(&entry), None);
}

#[test]
fn malformed_json_missing_content_returns_none() {
    let entry = json!({
        "type": "assistant",
        "message": {}
    });
    assert_eq!(parse_claude_state(&entry), None);
}

#[test]
fn empty_content_array_produces_waiting() {
    let entry = json!({
        "type": "assistant",
        "message": {
            "content": []
        }
    });
    assert_eq!(
        parse_claude_state(&entry),
        Some(AgentState::WaitingForInput)
    );
}
