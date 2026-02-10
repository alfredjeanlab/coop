// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::Path;

use serde_json::json;

use super::{prepare_claude_session, project_dir_name};
use crate::config::AgentSettings;

#[test]
fn project_dir_name_strips_leading_separator() {
    let name = project_dir_name(Path::new("/Users/alice/projects/myapp"));
    assert!(!name.starts_with('-'));
    assert!(name.contains("Users-alice-projects-myapp"));
}

#[test]
fn project_dir_name_replaces_slashes() {
    let name = project_dir_name(Path::new("/a/b/c"));
    assert!(!name.contains('/'));
}

#[test]
fn prepare_session_creates_settings_file() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", None)?;

    // Settings file should exist in the temp dir
    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    assert!(settings_path.exists());

    // Settings should contain hook config
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;
    assert!(parsed.get("hooks").is_some());
    Ok(())
}

#[test]
fn prepare_session_has_session_id_arg() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", None)?;

    assert!(setup.extra_args.contains(&"--session-id".to_owned()));
    // Session ID should be a UUID (36 chars with hyphens)
    let id_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--session-id")
        .ok_or_else(|| anyhow::anyhow!("no --session-id arg"))?;
    let id = &setup.extra_args[id_idx + 1];
    assert_eq!(id.len(), 36);
    Ok(())
}

#[test]
fn prepare_session_has_env_vars() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", None)?;

    assert!(setup.env_vars.iter().any(|(k, _)| k == "COOP_HOOK_PIPE"));
    Ok(())
}

#[test]
fn prepare_session_pipe_path_in_temp_dir() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", None)?;

    assert!(setup.hook_pipe_path.file_name().is_some());
    assert_eq!(setup.hook_pipe_path.file_name().and_then(|n| n.to_str()), Some("hook.pipe"));
    Ok(())
}

#[test]
fn prepare_session_with_base_settings_merges_hooks() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let orchestrator: AgentSettings = serde_json::from_value(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "gt-prime"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "gt-guard"}]}]
        },
        "permissions": { "allow": ["Bash", "Read"] }
    }))?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", Some(&orchestrator))?;

    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    // SessionStart: orchestrator entry first, coop entry second
    let session_start = parsed["hooks"]["SessionStart"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("no SessionStart hooks"))?;
    assert!(session_start.len() >= 2);
    assert_eq!(session_start[0]["hooks"][0]["command"], "gt-prime");

    // Orchestrator permissions pass through, coop send rule appended
    let allow = parsed["permissions"]["allow"].as_array().unwrap();
    assert_eq!(allow[0], "Bash");
    assert_eq!(allow[1], "Read");
    assert!(allow.contains(&json!("Bash(coop send:*)")));

    // Coop-only hook types present
    assert!(parsed["hooks"]["PostToolUse"].as_array().is_some());
    assert!(parsed["hooks"]["Stop"].as_array().is_some());
    Ok(())
}

#[test]
fn prepare_session_injects_coop_send_permission() -> anyhow::Result<()> {
    let work_dir = tempfile::tempdir()?;
    let setup = prepare_claude_session(work_dir.path(), "http://127.0.0.1:0", None)?;

    let settings_arg_idx = setup
        .extra_args
        .iter()
        .position(|a| a == "--settings")
        .ok_or_else(|| anyhow::anyhow!("no --settings arg"))?;
    let settings_path = Path::new(&setup.extra_args[settings_arg_idx + 1]);
    let content = std::fs::read_to_string(settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&content)?;

    let allow = parsed["permissions"]["allow"].as_array().unwrap();
    assert!(allow.contains(&json!("Bash(coop send:*)")));
    Ok(())
}
