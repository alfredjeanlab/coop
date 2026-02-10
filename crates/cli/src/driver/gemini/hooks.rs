// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::config::AgentSettings;

/// Generate the Gemini CLI hook configuration.
///
/// Gemini hooks receive JSON on stdin and must output JSON on stdout.
/// The hooks read stdin, wrap it, and write to the named pipe at `$COOP_HOOK_PIPE`:
/// - `BeforeAgent`: fires at the start of each turn (after user prompt)
/// - `BeforeTool`: fires before each tool call
/// - `AfterTool`: fires after each tool call, includes tool name and result
/// - `AfterAgent`: fires after each turn; curls gating endpoint
/// - `SessionEnd`: fires when the session ends
/// - `Notification`: fires on system notifications (e.g. `ToolPermission`)
pub fn generate_hook_config(pipe_path: &Path) -> AgentSettings {
    // Use $COOP_HOOK_PIPE so the config is portable across processes.
    // The actual path is passed via environment variable.
    let _ = pipe_path; // validated by caller; config uses env var

    // SessionStart hook: write start event to pipe, then curl start endpoint.
    // The response is a shell script that gets eval'd.
    let session_start_command = concat!(
        "input=$(cat); ",
        "event=$(printf '{\"event\":\"start\",\"data\":%s}' \"$input\"); ",
        "printf '%s\\n' \"$event\" > \"$COOP_HOOK_PIPE\"; ",
        "response=$(printf '%s' \"$event\" | curl -sf -X POST ",
        "-H 'Content-Type: application/json' ",
        "-d @- \"$COOP_URL/api/v1/hooks/start\" 2>/dev/null); ",
        "[ -n \"$response\" ] && eval \"$response\""
    );

    // AfterAgent hook: write stop event to pipe, then curl gating endpoint.
    // Gemini uses {"continue":true} to prevent stopping (vs Claude's {"decision":"block"}).
    // If curl fails (coop not ready), the hook outputs nothing → agent proceeds.
    // Builds the event envelope once and sends it to both the pipe and the endpoint.
    let after_agent_command = concat!(
        "input=$(cat); ",
        "event=$(printf '{\"event\":\"stop\",\"data\":%s}' \"$input\"); ",
        "printf '%s\\n' \"$event\" > \"$COOP_HOOK_PIPE\"; ",
        "response=$(printf '%s' \"$event\" | curl -sf -X POST ",
        "-H 'Content-Type: application/json' ",
        "-d @- \"$COOP_URL/api/v1/hooks/stop\" 2>/dev/null); ",
        "if printf '%s' \"$response\" | grep -q '\"block\"'; then printf '{\"continue\":true}'; fi"
    );

    let mut hooks = BTreeMap::new();
    hooks.insert(
        "SessionStart".to_string(),
        vec![json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": session_start_command}]
        })],
    );
    hooks.insert("BeforeAgent".to_string(), vec![json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"before_agent\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert("BeforeTool".to_string(), vec![json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"pre_tool_use\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert("AfterTool".to_string(), vec![json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"after_tool\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert(
        "AfterAgent".to_string(),
        vec![json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": after_agent_command}]
        })],
    );
    hooks.insert("SessionEnd".to_string(), vec![json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "cat > /dev/null; echo '{\"event\":\"session_end\"}' > \"$COOP_HOOK_PIPE\""}]
    })]);
    hooks.insert("Notification".to_string(), vec![json!({
        "matcher": "*",
        "hooks": [{"type": "command", "command": "input=$(cat); printf '{\"event\":\"notification\",\"data\":%s}\\n' \"$input\" > \"$COOP_HOOK_PIPE\""}]
    })]);

    AgentSettings { hooks: Some(hooks), ..Default::default() }
}

/// Return environment variables to set on the Gemini child process.
pub fn hook_env_vars(pipe_path: &Path, coop_url: &str) -> Vec<(String, String)> {
    vec![
        ("COOP_HOOK_PIPE".to_string(), pipe_path.display().to_string()),
        ("COOP_URL".to_string(), coop_url.to_string()),
    ]
}

/// Write the hook config to a settings file and return its path.
///
/// The config file is written into `config_dir` so Gemini can load it
/// via `GEMINI_CLI_SYSTEM_SETTINGS_PATH`.
pub fn write_hook_config(
    config_dir: &Path,
    pipe_path: &Path,
) -> anyhow::Result<std::path::PathBuf> {
    let config = generate_hook_config(pipe_path);
    let config_path = config_dir.join("coop-gemini-settings.json");
    let contents = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, contents)?;
    Ok(config_path)
}

#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
