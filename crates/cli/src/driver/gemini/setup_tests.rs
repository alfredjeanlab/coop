// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde_json::json;

use super::prepare_gemini_session;
use crate::config::{AgentSettings, McpConfig};

#[test]
fn prepare_session_creates_settings_file() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", None, None)?;

    // Settings file should exist in the temp dir (pointed to by env var)
    let settings_path = setup
        .env_vars
        .iter()
        .find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH")
        .map(|(_, v)| std::path::PathBuf::from(v))
        .ok_or_else(|| anyhow::anyhow!("no GEMINI_CLI_SYSTEM_SETTINGS_PATH env var"))?;
    assert!(settings_path.exists());

    // Settings should contain hook config
    let content = std::fs::read_to_string(&settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}

#[test]
fn prepare_session_has_env_vars() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", None, None)?;

    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_URL"));
    assert!(setup.env_vars.iter().any(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH"));
    Ok(())
}

#[test]
fn prepare_session_pipe_path_in_temp_dir() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", None, None)?;

    assert!(setup.hook_pipe_path.file_name().is_some());
    assert_eq!(setup.hook_pipe_path.file_name().and_then(|n| n.to_str()), Some("hook.pipe"));
    Ok(())
}

#[test]
fn prepare_session_has_no_extra_args() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", None, None)?;

    // Gemini doesn't need extra CLI args (no --session-id, etc.)
    assert!(setup.extra_args.is_empty());
    Ok(())
}

#[test]
fn prepare_session_with_base_settings_merges_hooks() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let orchestrator: AgentSettings = serde_json::from_value(json!({
        "hooks": {
            "SessionStart": [{"matcher": "*", "hooks": [{"type": "command", "command": "gt-prime"}]}],
            "BeforeTool": [{"matcher": "*", "hooks": [{"type": "command", "command": "gt-guard"}]}]
        },
        "permissions": { "allow": ["shell"] }
    }))?;
    let setup =
        prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", Some(&orchestrator), None)?;

    let settings_path = setup
        .env_vars
        .iter()
        .find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH")
        .map(|(_, v)| std::path::PathBuf::from(v))
        .ok_or_else(|| anyhow::anyhow!("no GEMINI_CLI_SYSTEM_SETTINGS_PATH env var"))?;
    let content = std::fs::read_to_string(&settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    // SessionStart: orchestrator entry first, coop entry second
    let session_start = parsed["hooks"]["SessionStart"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no SessionStart hooks"))?;
    assert!(session_start.len() >= 2);
    assert_eq!(session_start[0]["hooks"][0]["command"], "gt-prime");

    // Permissions pass through from orchestrator
    assert_eq!(parsed["permissions"]["allow"][0], "shell");

    // Coop-only hook types present
    assert!(parsed["hooks"]["AfterAgent"].as_array().is_some());
    assert!(parsed["hooks"]["BeforeAgent"].as_array().is_some());
    Ok(())
}

#[test]
fn prepare_session_with_mcp_config_includes_servers() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let mcp: McpConfig = serde_json::from_value(json!({
        "my-server": { "command": "node", "args": ["server.js"] }
    }))?;
    let setup = prepare_gemini_session(work_dir.path(), "http://127.0.0.1:0", None, Some(&mcp))?;

    let settings_path = setup
        .env_vars
        .iter()
        .find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH")
        .map(|(_, v)| std::path::PathBuf::from(v))
        .ok_or_else(|| anyhow::anyhow!("no GEMINI_CLI_SYSTEM_SETTINGS_PATH env var"))?;
    let content = std::fs::read_to_string(&settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    // mcpServers merged into settings
    assert_eq!(parsed["mcpServers"]["my-server"]["command"], "node");
    // hooks still present
    assert!(parsed.get("hooks").is_some());
    Ok(())
}
