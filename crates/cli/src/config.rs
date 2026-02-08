// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::PathBuf;

use clap::Parser;

/// Terminal session manager for AI coding agents.
#[derive(Debug, Parser)]
#[command(name = "coop", version, about)]
pub struct Config {
    /// HTTP port to listen on.
    #[arg(long, env = "COOP_PORT")]
    pub port: Option<u16>,

    /// Unix socket path for HTTP.
    #[arg(long, env = "COOP_SOCKET")]
    pub socket: Option<String>,

    /// Host address to bind to.
    #[arg(long, env = "COOP_HOST", default_value = "0.0.0.0")]
    pub host: String,

    /// gRPC port to listen on.
    #[arg(long, env = "COOP_GRPC_PORT")]
    pub grpc_port: Option<u16>,

    /// Bearer token for API authentication.
    #[arg(long, env = "COOP_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    /// Agent type (claude, codex, gemini, unknown).
    #[arg(long, env = "COOP_AGENT_TYPE", default_value = "unknown")]
    pub agent_type: String,

    /// Path to agent-specific config file.
    #[arg(long, env = "COOP_AGENT_CONFIG")]
    pub agent_config: Option<PathBuf>,

    /// Idle grace period in seconds before confirming idle state.
    #[arg(long, env = "COOP_IDLE_GRACE", default_value = "60")]
    pub idle_grace: u64,

    /// Attach to an existing session (e.g. tmux:session-name).
    #[arg(long, env = "COOP_ATTACH")]
    pub attach: Option<String>,

    /// Terminal columns.
    #[arg(long, env = "COOP_COLS", default_value = "200")]
    pub cols: u16,

    /// Terminal rows.
    #[arg(long, env = "COOP_ROWS", default_value = "50")]
    pub rows: u16,

    /// Ring buffer size in bytes.
    #[arg(long, env = "COOP_RING_SIZE", default_value = "1048576")]
    pub ring_size: usize,

    /// TERM environment variable for the child process.
    #[arg(long, env = "TERM", default_value = "xterm-256color")]
    pub term: String,

    /// Health-check-only HTTP port.
    #[arg(long, env = "COOP_HEALTH_PORT")]
    pub health_port: Option<u16>,

    /// Idle timeout in seconds (0 = disabled).
    #[arg(long, env = "COOP_IDLE_TIMEOUT", default_value = "0")]
    pub idle_timeout: u64,

    /// Log format (json or text).
    #[arg(long, env = "COOP_LOG_FORMAT", default_value = "json")]
    pub log_format: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, env = "COOP_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Auto-handle startup prompts (trust, permissions).
    /// Default: true for --agent-type claude, false otherwise.
    #[arg(long, env = "COOP_SKIP_STARTUP_PROMPTS")]
    pub skip_startup_prompts: Option<bool>,

    /// Resume a previous session from a log path or workspace ID.
    #[arg(long, env = "COOP_RESUME")]
    pub resume: Option<String>,

    /// Command to run (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// Known agent types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Codex,
    Gemini,
    Unknown,
}

impl Config {
    /// Validate the configuration after parsing.
    pub fn validate(&self) -> anyhow::Result<()> {
        // Must have at least one transport
        if self.port.is_none() && self.socket.is_none() {
            anyhow::bail!("either --port or --socket must be specified");
        }

        // Must have either command or attach (not both, not neither)
        let has_command = !self.command.is_empty();
        let has_attach = self.attach.is_some();

        if has_command && has_attach {
            anyhow::bail!("cannot specify both a command and --attach");
        }
        if !has_command && !has_attach {
            anyhow::bail!("either a command or --attach must be specified");
        }

        // Validate agent type
        self.agent_type_enum()?;

        // --resume is only valid with --agent-type claude and cannot combine with --attach
        if self.resume.is_some() {
            if self.agent_type_enum()? != AgentType::Claude {
                anyhow::bail!("--resume is only supported with --agent-type claude");
            }
            if self.attach.is_some() {
                anyhow::bail!("--resume cannot be combined with --attach");
            }
        }

        Ok(())
    }

    /// Parse the agent type string into an enum.
    pub fn agent_type_enum(&self) -> anyhow::Result<AgentType> {
        match self.agent_type.to_lowercase().as_str() {
            "claude" => Ok(AgentType::Claude),
            "codex" => Ok(AgentType::Codex),
            "gemini" => Ok(AgentType::Gemini),
            "unknown" => Ok(AgentType::Unknown),
            other => anyhow::bail!("invalid agent type: {other}"),
        }
    }

    /// Resolve whether startup prompts should be auto-handled.
    /// Defaults to `true` for Claude, `false` otherwise.
    pub fn effective_skip_startup_prompts(&self) -> bool {
        self.skip_startup_prompts.unwrap_or_else(|| {
            self.agent_type_enum()
                .map(|t| t == AgentType::Claude)
                .unwrap_or(false)
        })
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
