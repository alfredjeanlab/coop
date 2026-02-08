// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::Value;

use crate::driver::PromptContext;
use crate::screen::ScreenSnapshot;

/// Extract permission prompt context from a session log entry.
///
/// Finds the last `tool_use` block in the message and extracts the tool
/// name and a truncated preview of its input.
pub fn extract_permission_context(json: &Value) -> PromptContext {
    let tool_use = find_last_tool_use(json);
    let (tool, input_preview) = match tool_use {
        Some(block) => {
            let tool = block.get("name").and_then(|v| v.as_str()).map(String::from);
            let preview = block.get("input").and_then(summarize_tool_input);
            (tool, preview)
        }
        None => (None, None),
    };

    PromptContext {
        prompt_type: "permission".to_string(),
        tool,
        input_preview,
        question: None,
        options: vec![],
        summary: None,
        screen_lines: vec![],
    }
}

/// Extract context from an `AskUserQuestion` tool_use block.
///
/// Handles Claude's tool input format where questions are in a
/// `questions` array with `question` text and `options[].label`.
pub fn extract_ask_user_context(block: &Value) -> PromptContext {
    let input = block.get("input");

    // Claude's format: input.questions[0]
    let first_q = input
        .and_then(|i| i.get("questions"))
        .and_then(|q| q.as_array())
        .and_then(|arr| arr.first());

    let question = first_q
        .and_then(|q| q.get("question"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let options: Vec<String> = first_q
        .and_then(|q| q.get("options"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.get("label")
                        .and_then(|l| l.as_str())
                        .or_else(|| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    PromptContext {
        prompt_type: "question".to_string(),
        tool: Some("AskUserQuestion".to_string()),
        input_preview: None,
        question,
        options,
        summary: None,
        screen_lines: vec![],
    }
}

/// Extract plan prompt context from the terminal screen.
///
/// Plan prompts are detected via the screen rather than the session log,
/// so context is built from the visible screen lines.
pub fn extract_plan_context(screen: &ScreenSnapshot) -> PromptContext {
    PromptContext {
        prompt_type: "plan".to_string(),
        tool: None,
        input_preview: None,
        question: None,
        options: vec![],
        summary: None,
        screen_lines: screen.lines.clone(),
    }
}

/// Truncate tool input JSON to a ~200 character preview string.
fn summarize_tool_input(input: &Value) -> Option<String> {
    let s = serde_json::to_string(input).ok()?;
    if s.len() <= 200 {
        return Some(s);
    }

    // Find a safe truncation point that doesn't split multi-byte chars
    let mut end = 200;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut result = s[..end].to_string();
    result.push_str("...");
    Some(result)
}

/// Find the last `tool_use` block in a message's content array.
fn find_last_tool_use(json: &Value) -> Option<&Value> {
    let content = json.get("message")?.get("content")?.as_array()?;
    content
        .iter()
        .rev()
        .find(|block| block.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
