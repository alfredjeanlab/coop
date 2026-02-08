// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use crate::driver::AgentState;

use super::parse_claude_state;

#[yare::parameterized(
    error_string = {
        json!({ "error": "rate_limit_exceeded" }),
        Some(AgentState::Error { detail: "rate_limit_exceeded".to_string() })
    },
    error_non_string = {
        json!({ "error": 42 }),
        Some(AgentState::Error { detail: "unknown".to_string() })
    },
    system_message = {
        json!({ "type": "system", "message": { "content": [] } }),
        Some(AgentState::Working)
    },
    user_message = {
        json!({ "type": "user", "message": { "content": [{ "type": "text", "text": "hello" }] } }),
        Some(AgentState::Working)
    },
    assistant_tool_use = {
        json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "name": "Bash", "input": { "command": "ls" } }] } }),
        Some(AgentState::Working)
    },
    assistant_thinking = {
        json!({ "type": "assistant", "message": { "content": [{ "type": "thinking", "thinking": "Let me consider..." }] } }),
        Some(AgentState::Working)
    },
    assistant_text_only = {
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Here is the result." }] } }),
        Some(AgentState::WaitingForInput)
    },
    empty_content = {
        json!({ "type": "assistant", "message": { "content": [] } }),
        Some(AgentState::WaitingForInput)
    },
    missing_message = {
        json!({ "type": "assistant" }),
        None
    },
    missing_content = {
        json!({ "type": "assistant", "message": {} }),
        None
    },
)]
fn state_from_jsonl(entry: serde_json::Value, expected: Option<AgentState>) {
    assert_eq!(parse_claude_state(&entry), expected);
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
