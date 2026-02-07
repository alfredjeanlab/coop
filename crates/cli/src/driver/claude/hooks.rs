// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::path::Path;

use serde_json::{json, Value};

/// Generate the Claude Code hook configuration JSON.
///
/// The hooks write JSON events to the named pipe at `$COOP_HOOK_PIPE`:
/// - `PostToolUse`: fires after each tool call, writes tool name
/// - `Stop`: fires when the agent stops
pub fn generate_hook_config(pipe_path: &Path) -> Value {
    // Use $COOP_HOOK_PIPE so the config is portable across processes.
    // The actual path is passed via environment variable.
    let _ = pipe_path; // validated by caller; config uses env var
    json!({
        "hooks": {
            "PostToolUse": [{
                "type": "command",
                "command": "echo '{\"event\":\"post_tool_use\",\"tool\":\"'\"$TOOL_NAME\"'\"}' > \"$COOP_HOOK_PIPE\""
            }],
            "Stop": [{
                "type": "command",
                "command": "echo '{\"event\":\"stop\"}' > \"$COOP_HOOK_PIPE\""
            }]
        }
    })
}

/// Return environment variables to set on the Claude child process.
pub fn hook_env_vars(pipe_path: &Path) -> Vec<(String, String)> {
    vec![(
        "COOP_HOOK_PIPE".to_string(),
        pipe_path.display().to_string(),
    )]
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
