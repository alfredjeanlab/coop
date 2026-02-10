// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::config::AgentSettings;

/// Generate the Claude Code hook configuration.
///
/// The hooks write JSON events to the named pipe at `$COOP_HOOK_PIPE`:
/// - `PostToolUse`: fires after each tool call, writes tool name
/// - `Stop`: curls `$COOP_URL/api/v1/hooks/stop` for a gating verdict
/// - `Notification`: fires on `idle_prompt` and `permission_prompt`
/// - `PreToolUse`: fires before `AskUserQuestion`, `ExitPlanMode`, `EnterPlanMode`
pub fn generate_hook_config(pipe_path: &Path) -> AgentSettings {
    // Use $COOP_HOOK_PIPE so the config is portable across processes.
    // The actual path is passed via environment variable.
    let _ = pipe_path; // validated by caller; config uses env var

    // Start hook uses curl to call coop's start endpoint. The response is
    // a shell script that gets eval'd. If curl fails or returns empty, nothing happens.
    let start_command = concat!(
        "input=$(cat); ",
        "event=$(printf '{\"event\":\"start\",\"data\":%s}' \"$input\"); ",
        "printf '%s\\n' \"$event\" > \"$COOP_HOOK_PIPE\"; ",
        "response=$(printf '%s' \"$event\" | curl -sf -X POST ",
        "-H 'Content-Type: application/json' ",
        "-d @- \"$COOP_URL/api/v1/hooks/start\" 2>/dev/null); ",
        "[ -n \"$response\" ] && eval \"$response\""
    );

    // Stop hook uses curl to call coop's gating endpoint. If curl fails
    // (coop not ready), the hook outputs nothing and exits 0 → agent proceeds.
    // The -f flag makes curl return non-zero on HTTP errors.
    // Builds the event envelope once and sends it to both the pipe and the endpoint.
    let stop_command = concat!(
        "input=$(cat); ",
        "event=$(printf '{\"event\":\"stop\",\"data\":%s}' \"$input\"); ",
        "printf '%s\\n' \"$event\" > \"$COOP_HOOK_PIPE\"; ",
        "response=$(printf '%s' \"$event\" | curl -sf -X POST ",
        "-H 'Content-Type: application/json' ",
        "-d @- \"$COOP_URL/api/v1/hooks/stop\" 2>/dev/null); ",
        "[ -n \"$response\" ] && printf '%s' \"$response\""
    );

    let mut hooks = BTreeMap::new();
    hooks.insert(
        "SessionStart".to_string(),
        vec![json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": start_command}]
        })],
    );
    hooks.insert("PostToolUse".to_string(), vec![json!({
        "matcher": "",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"post_tool_use\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert(
        "Stop".to_string(),
        vec![json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": stop_command}]
        })],
    );
    hooks.insert("Notification".to_string(), vec![json!({
        "matcher": "idle_prompt|permission_prompt",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"notification\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert("PreToolUse".to_string(), vec![json!({
        "matcher": "ExitPlanMode|AskUserQuestion|EnterPlanMode",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"pre_tool_use\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert("UserPromptSubmit".to_string(), vec![json!({
        "matcher": "",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"user_prompt_submit\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);

    AgentSettings { hooks: Some(hooks), ..Default::default() }
}

/// Return environment variables to set on the Claude child process.
pub fn hook_env_vars(pipe_path: &Path, coop_url: &str) -> Vec<(String, String)> {
    vec![
        ("COOP_HOOK_PIPE".to_string(), pipe_path.display().to_string()),
        ("COOP_URL".to_string(), coop_url.to_string()),
    ]
}

/// Write the hook config to a file and return its path.
///
/// The config file is written into `config_dir` so Claude can load it
/// via `--hook-config`.
pub fn write_hook_config(
    config_dir: &Path,
    pipe_path: &Path,
) -> anyhow::Result<std::path::PathBuf> {
    let config = generate_hook_config(pipe_path);
    let config_path = config_dir.join("coop-hooks.json");
    let contents = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, contents)?;
    Ok(config_path)
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
