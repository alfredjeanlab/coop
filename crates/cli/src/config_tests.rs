// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use clap::Parser;
use serde_json::json;

use super::{merge_settings, AgentFileConfig, AgentSettings, AgentType, Config, GroomLevel};

fn parse(args: &[&str]) -> Config {
    Config::parse_from(args)
}

#[test]
fn valid_config_with_port_and_command() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo", "hello"]);
    config.validate()?;
    assert_eq!(config.port, Some(8080));
    assert_eq!(config.command, vec!["echo", "hello"]);
    Ok(())
}

#[test]
fn valid_config_with_socket_and_command() -> anyhow::Result<()> {
    let config = parse(&["coop", "--socket", "/tmp/coop.sock", "--", "bash"]);
    config.validate()?;
    assert_eq!(config.socket.as_deref(), Some("/tmp/coop.sock"));
    Ok(())
}

#[test]
fn valid_config_with_attach() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--attach", "tmux:my-session"]);
    config.validate()?;
    assert_eq!(config.attach.as_deref(), Some("tmux:my-session"));
    Ok(())
}

#[yare::parameterized(
    no_transport        = { &["coop", "--", "echo"], "--port or --socket" },
    no_command          = { &["coop", "--port", "8080"], "command or --attach" },
    both_cmd_and_attach = { &["coop", "--port", "8080", "--attach", "tmux:sess", "--", "echo"],
                            "cannot specify both" },
)]
fn invalid_config(args: &[&str], expected_substr: &str) {
    let config = parse(args);
    crate::assert_err_contains!(config.validate(), expected_substr);
}

#[test]
fn agent_claude() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--agent", "claude", "--", "echo"]);
    assert_eq!(config.agent_enum()?, AgentType::Claude);
    Ok(())
}

#[test]
fn agent_unknown_default() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.agent_enum()?, AgentType::Unknown);
    Ok(())
}

#[test]
fn agent_invalid() {
    let config = parse(&["coop", "--port", "8080", "--agent", "gpt", "--", "echo"]);
    assert!(config.agent_enum().is_err());
}

// -- GroomLevel --

#[test]
fn groom_auto_default() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.groom_level()?, GroomLevel::Auto);
    Ok(())
}

#[test]
fn groom_manual() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--groom", "manual", "--", "echo"]);
    assert_eq!(config.groom_level()?, GroomLevel::Manual);
    Ok(())
}

#[test]
fn groom_case_insensitive() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--groom", "AUTO", "--", "echo"]);
    assert_eq!(config.groom_level()?, GroomLevel::Auto);
    Ok(())
}

#[test]
fn groom_pristine_validates() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--groom", "pristine", "--", "echo"]);
    config.validate()?;
    assert_eq!(config.groom_level()?, GroomLevel::Pristine);
    Ok(())
}

#[test]
fn groom_pristine_rejects_resume() {
    let config = parse(&[
        "coop", "--port", "8080", "--agent", "claude", "--groom", "pristine", "--resume",
        "some-id", "--", "claude",
    ]);
    crate::assert_err_contains!(
        config.validate(),
        "--resume cannot be combined with groom=pristine"
    );
}

#[test]
fn groom_invalid() {
    let config = parse(&["coop", "--port", "8080", "--groom", "nope", "--", "echo"]);
    assert!(config.groom_level().is_err());
}

#[test]
fn defaults_are_correct() {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.cols, 200);
    assert_eq!(config.rows, 50);
    assert_eq!(config.ring_size, 1048576);
    assert_eq!(config.log_format, "json");
    assert_eq!(config.log_level, "info");
}

#[test]
fn env_duration_defaults() {
    // These read env vars, so with no env set we get production defaults.
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.shutdown_timeout(), Duration::from_secs(10));
    assert_eq!(config.screen_debounce(), Duration::from_millis(50));
    assert_eq!(config.process_poll(), Duration::from_secs(10));
    assert_eq!(config.screen_poll(), Duration::from_secs(3));
    assert_eq!(config.log_poll(), Duration::from_secs(3));
    assert_eq!(config.tmux_poll(), Duration::from_secs(1));
    assert_eq!(config.reap_poll(), Duration::from_millis(50));
    assert_eq!(config.input_delay(), Duration::from_millis(200));
    assert_eq!(config.input_delay_per_byte(), Duration::from_millis(1));
    assert_eq!(config.input_delay_max(), Duration::from_millis(5000));
    assert_eq!(config.nudge_timeout(), Duration::from_millis(4000));
    assert_eq!(config.idle_timeout(), Duration::ZERO);
}

// -- Config::test() fast timings --

#[test]
fn test_config_uses_fast_timings() {
    let config = Config::test();
    assert_eq!(config.shutdown_timeout(), Duration::from_millis(100));
    assert_eq!(config.screen_debounce(), Duration::from_millis(10));
    assert_eq!(config.process_poll(), Duration::from_millis(50));
    assert_eq!(config.screen_poll(), Duration::from_millis(50));
    assert_eq!(config.log_poll(), Duration::from_millis(50));
    assert_eq!(config.tmux_poll(), Duration::from_millis(50));
    assert_eq!(config.reap_poll(), Duration::from_millis(10));
    assert_eq!(config.input_delay(), Duration::from_millis(10));
    assert_eq!(config.input_delay_per_byte(), Duration::ZERO);
    assert_eq!(config.input_delay_max(), Duration::from_millis(50));
    assert_eq!(config.nudge_timeout(), Duration::from_millis(100));
    assert_eq!(config.idle_timeout(), Duration::ZERO);
    assert_eq!(config.drain_timeout(), Duration::from_millis(100));
}

// -- AgentFileConfig deserialization --

#[test]
fn agent_file_config_deserializes_settings_and_mcp() -> anyhow::Result<()> {
    let input = json!({
        "settings": {
            "hooks": { "PostToolUse": [{"matcher": "", "hooks": []}] },
            "permissions": { "allow": ["Bash"] }
        },
        "mcp": {
            "my-server": { "command": "node", "args": ["server.js"] }
        }
    });
    let config: AgentFileConfig = serde_json::from_value(input)?;
    assert!(config.settings.is_some());
    let settings = config.settings.as_ref().unwrap();
    assert!(settings.hooks.contains_key("PostToolUse"));
    assert_eq!(settings.permissions.as_ref().unwrap().allow, vec![json!("Bash")]);
    assert!(config.mcp.is_some());
    let mcp = config.mcp.as_ref().unwrap();
    assert!(mcp.contains_key("my-server"));
    assert_eq!(mcp["my-server"].command, "node");
    assert_eq!(mcp["my-server"].args, vec!["server.js"]);
    Ok(())
}

#[test]
fn agent_file_config_missing_settings_and_mcp() -> anyhow::Result<()> {
    let input = json!({});
    let config: AgentFileConfig = serde_json::from_value(input)?;
    assert!(config.settings.is_none());
    assert!(config.mcp.is_none());
    Ok(())
}

// -- merge_settings --

/// Helper: parse JSON into AgentSettings for test readability.
fn settings(v: serde_json::Value) -> AgentSettings {
    serde_json::from_value(v).unwrap_or_default()
}

#[test]
fn merge_no_orchestrator_returns_coop_config() {
    let coop = settings(json!({
        "hooks": {
            "PostToolUse": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-detect"}]}],
            "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-gate"}]}]
        }
    }));
    // When orchestrator has no hooks, coop hooks should appear as-is.
    let orchestrator = settings(json!({}));
    let merged = merge_settings(&orchestrator, coop.clone());
    assert_eq!(merged.hooks["PostToolUse"], coop.hooks["PostToolUse"]);
    assert_eq!(merged.hooks["Stop"], coop.hooks["Stop"]);
}

#[test]
fn merge_concatenates_hook_arrays_orchestrator_first() {
    let orchestrator = settings(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "gt-prime"}]}]
        }
    }));
    let coop = settings(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-detect"}]}]
        }
    }));
    let merged = merge_settings(&orchestrator, coop);
    let arr = &merged.hooks["SessionStart"];
    assert_eq!(arr.len(), 2);
    // Orchestrator entry first
    assert_eq!(arr[0].hooks[0].command.as_deref(), Some("gt-prime"));
    // Coop entry second
    assert_eq!(arr[1].hooks[0].command.as_deref(), Some("coop-detect"));
}

#[test]
fn merge_preserves_non_hook_keys() {
    let orchestrator = settings(json!({
        "permissions": { "allow": ["Bash", "Read"] },
        "env": { "MY_VAR": "hello" },
        "hooks": {}
    }));
    let coop = settings(json!({
        "hooks": {
            "Stop": [{"matcher": "", "hooks": []}]
        }
    }));
    let merged = merge_settings(&orchestrator, coop);
    let perms = merged.permissions.as_ref().unwrap();
    assert_eq!(perms.allow[0], json!("Bash"));
    let env = merged.env.as_ref().unwrap();
    assert_eq!(env["MY_VAR"], "hello");
}

#[test]
fn merge_orchestrator_only_hook_types_pass_through() {
    let orchestrator = settings(json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "gt-guard"}]}]
        }
    }));
    let coop = settings(json!({
        "hooks": {
            "PostToolUse": [{"matcher": "", "hooks": []}]
        }
    }));
    let merged = merge_settings(&orchestrator, coop);
    // Orchestrator-only hook type preserved
    assert_eq!(merged.hooks["PreToolUse"][0].hooks[0].command.as_deref(), Some("gt-guard"));
    // Coop-only hook type added
    assert!(merged.hooks.contains_key("PostToolUse"));
}

#[test]
fn merge_coop_only_hook_types_appear_in_result() {
    let orchestrator = settings(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": []}]
        }
    }));
    let coop = settings(json!({
        "hooks": {
            "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-gate"}]}],
            "Notification": [{"matcher": "idle_prompt", "hooks": []}]
        }
    }));
    let merged = merge_settings(&orchestrator, coop);
    assert!(merged.hooks.contains_key("Stop"));
    assert!(merged.hooks.contains_key("Notification"));
    // Original orchestrator hook still there
    assert!(merged.hooks.contains_key("SessionStart"));
}

#[test]
fn merge_realistic_gt_config() {
    let orchestrator = settings(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "gt-prime-context"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "gt-sandbox-guard"}]}]
        },
        "permissions": {
            "allow": ["Bash", "Read", "Write", "Edit"],
            "deny": []
        },
        "env": { "GT_WORKSPACE_ID": "ws-123" }
    }));
    let coop = settings(json!({
        "hooks": {
            "SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-start-hook"}]}],
            "PostToolUse": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-post-tool"}]}],
            "Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-stop-gate"}]}],
            "Notification": [{"matcher": "idle_prompt|permission_prompt", "hooks": [{"type": "command", "command": "coop-notify"}]}],
            "PreToolUse": [{"matcher": "ExitPlanMode|AskUserQuestion|EnterPlanMode", "hooks": [{"type": "command", "command": "coop-pre-tool"}]}],
            "UserPromptSubmit": [{"matcher": "", "hooks": [{"type": "command", "command": "coop-prompt-submit"}]}]
        }
    }));
    let merged = merge_settings(&orchestrator, coop);

    // SessionStart: gt-prime first, coop second
    let session_start = &merged.hooks["SessionStart"];
    assert_eq!(session_start.len(), 2);
    assert_eq!(session_start[0].hooks[0].command.as_deref(), Some("gt-prime-context"));
    assert_eq!(session_start[1].hooks[0].command.as_deref(), Some("coop-start-hook"));

    // PreToolUse: gt-guard first, coop second
    let pre_tool = &merged.hooks["PreToolUse"];
    assert_eq!(pre_tool.len(), 2);
    assert_eq!(pre_tool[0].hooks[0].command.as_deref(), Some("gt-sandbox-guard"));
    assert_eq!(pre_tool[1].hooks[0].command.as_deref(), Some("coop-pre-tool"));

    // Coop-only hooks present
    assert!(merged.hooks.contains_key("PostToolUse"));
    assert!(merged.hooks.contains_key("Stop"));
    assert!(merged.hooks.contains_key("Notification"));
    assert!(merged.hooks.contains_key("UserPromptSubmit"));

    // Non-hook keys pass through
    let perms = merged.permissions.as_ref().unwrap();
    assert_eq!(perms.allow[0], json!("Bash"));
    let env = merged.env.as_ref().unwrap();
    assert_eq!(env["GT_WORKSPACE_ID"], "ws-123");
}
