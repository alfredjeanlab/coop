// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use serde_json::json;

use super::prepare_pristine_extras;
use crate::config::{AgentSettings, McpConfig};
use crate::driver::AgentType;

// -- Claude pristine --

#[test]
fn pristine_claude_no_settings_returns_session_id_and_coop_url() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let (args, env, log_path) =
        prepare_pristine_extras(AgentType::Claude, dir, "http://127.0.0.1:8080", None, None)?;

    // --session-id <uuid>
    assert_eq!(args.len(), 2);
    assert_eq!(args[0], "--session-id");
    assert!(!args[1].is_empty());

    // Only COOP_URL (no COOP_HOOK_PIPE)
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].0, "COOP_URL");
    assert_eq!(env[0].1, "http://127.0.0.1:8080");

    // Session log path for Tier 2
    assert!(log_path.is_some());
    Ok(())
}

#[test]
fn pristine_claude_with_settings_writes_file_without_hooks() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let settings: AgentSettings = serde_json::from_value(json!({
        "permissions": { "allow": ["Bash"] },
        "env": { "FOO": "bar" }
    }))?;
    let (args, env, _) = prepare_pristine_extras(
        AgentType::Claude,
        dir,
        "http://127.0.0.1:8080",
        Some(&settings),
        None,
    )?;

    // --session-id <uuid> --settings <path>
    assert_eq!(args.len(), 4);
    assert_eq!(args[0], "--session-id");
    assert_eq!(args[2], "--settings");
    let settings_path = Path::new(&args[3]);
    assert!(settings_path.exists());

    // Settings file should NOT contain hooks (no coop hook merge)
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path)?)?;
    assert!(written.get("hooks").is_none());
    assert_eq!(written["permissions"]["allow"][0], "Bash");

    // No COOP_HOOK_PIPE
    assert!(!env.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    Ok(())
}

#[test]
fn pristine_claude_with_mcp_writes_mcp_config() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let mcp: McpConfig = serde_json::from_value(json!({
        "my-server": { "command": "node", "args": ["server.js"] }
    }))?;
    let (args, _, _) =
        prepare_pristine_extras(AgentType::Claude, dir, "http://127.0.0.1:8080", None, Some(&mcp))?;

    // --session-id <uuid> --mcp-config <path>
    assert_eq!(args.len(), 4);
    assert_eq!(args[2], "--mcp-config");
    let mcp_path = Path::new(&args[3]);
    assert!(mcp_path.exists());

    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(mcp_path)?)?;
    assert!(written.get("mcpServers").is_some());
    assert!(written["mcpServers"]["my-server"]["command"].as_str() == Some("node"));
    Ok(())
}

// -- Gemini pristine --

#[test]
fn pristine_gemini_with_settings_and_mcp() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let settings: AgentSettings = serde_json::from_value(json!({ "theme": "dark" }))?;
    let mcp: McpConfig = serde_json::from_value(json!({
        "tool-server": { "command": "python", "args": ["serve.py"] }
    }))?;
    let (args, env, log_path) = prepare_pristine_extras(
        AgentType::Gemini,
        dir,
        "http://127.0.0.1:8080",
        Some(&settings),
        Some(&mcp),
    )?;

    // No CLI args for Gemini
    assert!(args.is_empty());

    // COOP_URL + GEMINI_CLI_SYSTEM_SETTINGS_PATH
    assert_eq!(env.len(), 2);
    assert_eq!(env[0].0, "COOP_URL");
    let settings_env = env.iter().find(|(k, _)| k == "GEMINI_CLI_SYSTEM_SETTINGS_PATH");
    assert!(settings_env.is_some());

    // Settings file has MCP embedded and no hooks
    let settings_path = Path::new(&settings_env.unwrap().1);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(settings_path)?)?;
    assert_eq!(written["theme"], "dark");
    assert!(written["mcpServers"]["tool-server"]["command"].as_str() == Some("python"));
    assert!(written.get("hooks").is_none());

    // No session log path for Gemini
    assert!(log_path.is_none());
    Ok(())
}

// -- Unknown agent pristine --

#[test]
fn pristine_unknown_returns_only_coop_url() -> anyhow::Result<()> {
    let dir = Path::new("/tmp/test-pristine");
    let (args, env, log_path) =
        prepare_pristine_extras(AgentType::Unknown, dir, "http://127.0.0.1:9000", None, None)?;

    assert!(args.is_empty());
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].0, "COOP_URL");
    assert!(log_path.is_none());
    Ok(())
}
