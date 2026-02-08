// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::Value;

use crate::driver::AgentState;

use super::prompt::extract_ask_user_context;

/// Parse a Claude session log JSONL entry into an [`AgentState`].
///
/// Returns `None` if the entry cannot be meaningfully classified (e.g.
/// missing required fields on an assistant message).
pub fn parse_claude_state(json: &Value) -> Option<AgentState> {
    // Error field takes priority
    if let Some(error) = json.get("error") {
        return Some(AgentState::Error {
            detail: error.as_str().unwrap_or("unknown").to_string(),
        });
    }

    // Only assistant messages carry meaningful state transitions
    if json.get("type").and_then(|v| v.as_str()) != Some("assistant") {
        return Some(AgentState::Working);
    }

    let content = json.get("message")?.get("content")?.as_array()?;

    for block in content {
        let block_type = block.get("type").and_then(|v| v.as_str());
        match block_type {
            Some("tool_use") => {
                let tool = block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                return match tool {
                    "AskUserQuestion" => Some(AgentState::AskUser {
                        prompt: extract_ask_user_context(block),
                    }),
                    _ => Some(AgentState::Working),
                };
            }
            Some("thinking") => return Some(AgentState::Working),
            _ => {}
        }
    }

    // Assistant message with no tool_use or thinking blocks — idle
    Some(AgentState::WaitingForInput)
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
