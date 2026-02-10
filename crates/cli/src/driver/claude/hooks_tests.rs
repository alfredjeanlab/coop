// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use super::{generate_hook_config, hook_env_vars, write_hook_config};

#[test]
fn generated_config_has_required_hooks() {
    let config = generate_hook_config(Path::new("/tmp/coop.pipe"));

    assert!(config.hooks.contains_key("SessionStart"));
    assert!(config.hooks.contains_key("PostToolUse"));
    assert!(config.hooks.contains_key("Stop"));
    assert!(config.hooks.contains_key("Notification"));
    assert!(config.hooks.contains_key("PreToolUse"));
    assert!(config.hooks.contains_key("UserPromptSubmit"));

    // Verify nested matcher + hooks structure
    let post_tool = &config.hooks["PostToolUse"];
    assert_eq!(post_tool[0].matcher, "");
    assert_eq!(post_tool[0].hooks[0].hook_type, "command");

    let stop = &config.hooks["Stop"];
    assert_eq!(stop[0].matcher, "");
    assert_eq!(stop[0].hooks[0].hook_type, "command");

    let notification = &config.hooks["Notification"];
    assert_eq!(notification[0].matcher, "idle_prompt|permission_prompt");
    assert_eq!(notification[0].hooks[0].hook_type, "command");

    let pre_tool = &config.hooks["PreToolUse"];
    assert_eq!(pre_tool[0].matcher, "ExitPlanMode|AskUserQuestion|EnterPlanMode");
    assert_eq!(pre_tool[0].hooks[0].hook_type, "command");
}

#[test]
fn config_references_env_var() {
    let config = generate_hook_config(Path::new("/tmp/coop.pipe"));
    let config_str = serde_json::to_string(&config).unwrap_or_default();

    // Config should use $COOP_HOOK_PIPE, not a hardcoded path
    assert!(config_str.contains("COOP_HOOK_PIPE"));
    // Stop hook should reference $COOP_URL for the gating endpoint
    assert!(config_str.contains("COOP_URL"));
}

#[test]
fn env_vars_include_pipe_path_and_url() {
    let vars = hook_env_vars(Path::new("/tmp/coop.pipe"), "http://127.0.0.1:8080");
    assert_eq!(vars.len(), 2);
    assert_eq!(vars[0].0, "COOP_HOOK_PIPE");
    assert_eq!(vars[0].1, "/tmp/coop.pipe");
    assert_eq!(vars[1].0, "COOP_URL");
    assert_eq!(vars[1].1, "http://127.0.0.1:8080");
}

#[test]
fn generated_json_is_valid() {
    let config = generate_hook_config(Path::new("/tmp/coop.pipe"));
    // Round-trip through string to verify valid JSON
    let s = serde_json::to_string(&config).unwrap_or_default();
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
    assert!(parsed.get("hooks").is_some());
}

#[test]
fn write_hook_config_creates_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let pipe_path = Path::new("/tmp/test-coop.pipe");

    let config_path = write_hook_config(dir.path(), pipe_path)?;
    assert!(config_path.exists());

    let content = std::fs::read_to_string(&config_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}
