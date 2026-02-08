// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use clap::Parser;

use super::{AgentType, Config};

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

#[test]
fn invalid_no_transport() {
    let config = parse(&["coop", "--", "echo"]);
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("--port or --socket"),
        "unexpected error: {err}"
    );
}

#[test]
fn invalid_no_command_or_attach() {
    let config = parse(&["coop", "--port", "8080"]);
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("command or --attach"),
        "unexpected error: {err}"
    );
}

#[test]
fn invalid_both_command_and_attach() {
    let config = parse(&[
        "coop",
        "--port",
        "8080",
        "--attach",
        "tmux:sess",
        "--",
        "echo",
    ]);
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("cannot specify both"),
        "unexpected error: {err}"
    );
}

#[test]
fn agent_type_claude() -> anyhow::Result<()> {
    let config = parse(&[
        "coop",
        "--port",
        "8080",
        "--agent-type",
        "claude",
        "--",
        "echo",
    ]);
    assert_eq!(config.agent_type_enum()?, AgentType::Claude);
    Ok(())
}

#[test]
fn agent_type_unknown_default() -> anyhow::Result<()> {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.agent_type_enum()?, AgentType::Unknown);
    Ok(())
}

#[test]
fn agent_type_invalid() {
    let config = parse(&[
        "coop",
        "--port",
        "8080",
        "--agent-type",
        "gpt",
        "--",
        "echo",
    ]);
    assert!(config.agent_type_enum().is_err());
}

#[test]
fn defaults_are_correct() {
    let config = parse(&["coop", "--port", "8080", "--", "echo"]);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.cols, 200);
    assert_eq!(config.rows, 50);
    assert_eq!(config.ring_size, 1048576);
    assert_eq!(config.idle_grace, 60);
    assert_eq!(config.idle_timeout, 0);
    assert_eq!(config.log_format, "json");
    assert_eq!(config.log_level, "info");
}
