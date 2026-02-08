// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use super::{generate_hook_config, hook_env_vars, write_hook_config};

#[test]
fn generated_config_has_required_hooks() {
    let config = generate_hook_config(Path::new("/tmp/coop.pipe"));
    let hooks = &config["hooks"];

    assert!(hooks.get("PostToolUse").is_some());
    assert!(hooks.get("Stop").is_some());

    let post_tool = &hooks["PostToolUse"];
    assert!(post_tool.is_array());

    let stop = &hooks["Stop"];
    assert!(stop.is_array());
}

#[test]
fn config_references_env_var() {
    let config = generate_hook_config(Path::new("/tmp/coop.pipe"));
    let config_str = serde_json::to_string(&config).unwrap_or_default();

    // Config should use $COOP_HOOK_PIPE, not a hardcoded path
    assert!(config_str.contains("COOP_HOOK_PIPE"));
}

#[test]
fn env_vars_include_pipe_path() {
    let vars = hook_env_vars(Path::new("/tmp/coop.pipe"));
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0].0, "COOP_HOOK_PIPE");
    assert_eq!(vars[0].1, "/tmp/coop.pipe");
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
