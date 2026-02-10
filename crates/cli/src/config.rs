// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::driver::AgentType;
use crate::start::StartConfig;
use crate::stop::StopConfig;

/// Default ring buffer size in bytes (1 MB).
pub const DEFAULT_RING_SIZE: usize = 1_048_576;

/// Controls how much coop auto-responds to agent prompts during startup.
///
/// - `Auto`: auto-dismiss "disruption" prompts (setup dialogs, workspace trust)
///   so the agent reaches idle ASAP.
/// - `Manual`: detection works and API exposes prompts, but nothing is
///   auto-dismissed (today's behavior).
/// - `Pristine`: no hook/FIFO injection; passive monitoring only (Tier 2 log + Tier 5 screen).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroomLevel {
    #[default]
    Auto,
    Manual,
    Pristine,
}

impl std::fmt::Display for GroomLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            Self::Manual => f.write_str("manual"),
            Self::Pristine => f.write_str("pristine"),
        }
    }
}

impl std::str::FromStr for GroomLevel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "manual" => Ok(Self::Manual),
            "pristine" => Ok(Self::Pristine),
            other => anyhow::bail!("invalid groom level: {other}"),
        }
    }
}

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
    pub port_grpc: Option<u16>,

    /// Bearer token for API authentication.
    #[arg(long, env = "COOP_AUTH_TOKEN")]
    pub auth_token: Option<String>,

    /// Agent type (claude, codex, gemini, unknown).
    #[arg(long, env = "COOP_AGENT", default_value = "unknown")]
    pub agent: String,

    /// Path to agent-specific config file.
    #[arg(long, env = "COOP_AGENT_CONFIG")]
    pub agent_config: Option<PathBuf>,

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
    #[arg(long, env = "COOP_RING_SIZE", default_value_t = DEFAULT_RING_SIZE)]
    pub ring_size: usize,

    /// TERM environment variable for the child process.
    #[arg(long, env = "TERM", default_value = "xterm-256color")]
    pub term: String,

    /// Health-check-only HTTP port.
    #[arg(long, env = "COOP_HEALTH_PORT")]
    pub port_health: Option<u16>,

    /// Log format (json or text).
    #[arg(long, env = "COOP_LOG_FORMAT", default_value = "json")]
    pub log_format: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, env = "COOP_LOG_LEVEL", default_value = "info")]
    pub log_level: String,

    /// Resume a previous session from a log path or workspace ID.
    #[arg(long, env = "COOP_RESUME")]
    pub resume: Option<String>,

    /// Command to run (after --).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,

    /// Groom level: auto, manual, pristine.
    #[arg(long, env = "COOP_GROOM", default_value = "auto")]
    pub groom: String,

    // -- Duration overrides (skip from CLI; set in Config::test()) --------
    /// Drain timeout in ms (0 = disabled, immediate kill on shutdown).
    #[clap(skip)]
    pub drain_timeout_ms: Option<u64>,
    #[clap(skip)]
    pub shutdown_timeout_ms: Option<u64>,
    #[clap(skip)]
    pub screen_debounce_ms: Option<u64>,
    #[clap(skip)]
    pub process_poll_ms: Option<u64>,
    #[clap(skip)]
    pub screen_poll_ms: Option<u64>,
    #[clap(skip)]
    pub log_poll_ms: Option<u64>,
    #[clap(skip)]
    pub tmux_poll_ms: Option<u64>,
    #[clap(skip)]
    pub reap_poll_ms: Option<u64>,
    #[clap(skip)]
    pub input_delay_ms: Option<u64>,
    #[clap(skip)]
    pub input_delay_per_byte_ms: Option<u64>,
    #[clap(skip)]
    pub input_delay_max_ms: Option<u64>,
    #[clap(skip)]
    pub nudge_timeout_ms: Option<u64>,
    #[clap(skip)]
    pub idle_timeout_ms: Option<u64>,
}

fn env_duration_ms(var: &str, default: u64) -> Duration {
    let ms = std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    Duration::from_millis(ms)
}

macro_rules! duration_field {
    ($method:ident, $field:ident, $env:literal, $default:expr) => {
        pub fn $method(&self) -> Duration {
            match self.$field {
                Some(ms) => Duration::from_millis(ms),
                None => env_duration_ms($env, $default),
            }
        }
    };
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
        self.agent_enum()?;

        // Validate groom level
        let groom = self.groom_level()?;

        // --resume is only valid with --agent claude and cannot combine with --attach
        if self.resume.is_some() {
            if self.agent_enum()? != AgentType::Claude {
                anyhow::bail!("--resume is only supported with --agent claude");
            }
            if self.attach.is_some() {
                anyhow::bail!("--resume cannot be combined with --attach");
            }
            if groom == GroomLevel::Pristine {
                anyhow::bail!("--resume cannot be combined with groom=pristine");
            }
        }

        Ok(())
    }

    // -- Tuning knobs (field override → env var → compiled default) --------

    duration_field!(shutdown_timeout, shutdown_timeout_ms, "COOP_SHUTDOWN_TIMEOUT_MS", 10_000);
    duration_field!(screen_debounce, screen_debounce_ms, "COOP_SCREEN_DEBOUNCE_MS", 50);
    duration_field!(process_poll, process_poll_ms, "COOP_PROCESS_POLL_MS", 10_000);
    duration_field!(screen_poll, screen_poll_ms, "COOP_SCREEN_POLL_MS", 3_000);
    duration_field!(log_poll, log_poll_ms, "COOP_LOG_POLL_MS", 3_000);
    duration_field!(tmux_poll, tmux_poll_ms, "COOP_TMUX_POLL_MS", 1_000);
    duration_field!(reap_poll, reap_poll_ms, "COOP_REAP_POLL_MS", 50);
    duration_field!(input_delay, input_delay_ms, "COOP_INPUT_DELAY_MS", 200);
    duration_field!(
        input_delay_per_byte,
        input_delay_per_byte_ms,
        "COOP_INPUT_DELAY_PER_BYTE_MS",
        1
    );
    duration_field!(input_delay_max, input_delay_max_ms, "COOP_INPUT_DELAY_MAX_MS", 5_000);
    duration_field!(nudge_timeout, nudge_timeout_ms, "COOP_NUDGE_TIMEOUT_MS", 4_000);
    duration_field!(idle_timeout, idle_timeout_ms, "COOP_IDLE_TIMEOUT_MS", 0);
    duration_field!(drain_timeout, drain_timeout_ms, "COOP_DRAIN_TIMEOUT_MS", 20_000);

    /// Build a minimal `Config` for tests (port 0, `echo` command).
    #[doc(hidden)]
    pub fn test() -> Self {
        Self {
            port: Some(0),
            socket: None,
            host: "127.0.0.1".into(),
            port_grpc: None,
            auth_token: None,
            agent: "unknown".into(),
            agent_config: None,
            attach: None,
            cols: 80,
            rows: 24,
            ring_size: 4096,
            term: "xterm-256color".into(),
            port_health: None,
            log_format: "json".into(),
            log_level: "debug".into(),
            resume: None,
            groom: "manual".into(),
            command: vec!["echo".into()],
            drain_timeout_ms: Some(100),
            shutdown_timeout_ms: Some(100),
            screen_debounce_ms: Some(10),
            process_poll_ms: Some(50),
            screen_poll_ms: Some(50),
            log_poll_ms: Some(50),
            tmux_poll_ms: Some(50),
            reap_poll_ms: Some(10),
            input_delay_ms: Some(10),
            input_delay_per_byte_ms: Some(0),
            input_delay_max_ms: Some(50),
            nudge_timeout_ms: Some(100),
            idle_timeout_ms: Some(0),
        }
    }

    /// Parse the groom level string into an enum.
    pub fn groom_level(&self) -> anyhow::Result<GroomLevel> {
        self.groom.parse()
    }

    /// Parse the agent type string into an enum.
    pub fn agent_enum(&self) -> anyhow::Result<AgentType> {
        match self.agent.to_lowercase().as_str() {
            "claude" => Ok(AgentType::Claude),
            "codex" => Ok(AgentType::Codex),
            "gemini" => Ok(AgentType::Gemini),
            "unknown" => Ok(AgentType::Unknown),
            other => anyhow::bail!("invalid agent type: {other}"),
        }
    }
}

/// Contents of the `--agent-config` JSON file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentFileConfig {
    /// Stop hook configuration. `None` means default allow behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<StopConfig>,
    /// Start hook configuration. `None` means no injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<StartConfig>,
    /// Agent settings (hooks, permissions, env, plugins) merged with coop's hooks.
    /// Orchestrator settings form the base layer; coop's detection hooks are appended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// MCP server definitions (`{"server-name": {"command": ...}, ...}`).
    /// For Claude, wrapped in `{"mcpServers": ...}` and passed via `--mcp-config`.
    /// For Gemini, inserted as `mcpServers` in the settings file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp: Option<serde_json::Value>,
}

/// Load and parse the agent config file at `path`.
///
/// Returns `AgentFileConfig` with any missing keys set to `None`.
pub fn load_agent_config(path: &Path) -> anyhow::Result<AgentFileConfig> {
    let contents = std::fs::read_to_string(path)?;
    let config: AgentFileConfig = serde_json::from_str(&contents)?;
    Ok(config)
}

/// Merge orchestrator settings with coop's generated hook config.
///
/// Rules:
/// 1. `hooks`: per hook type, concatenate arrays (orchestrator entries first, coop entries appended)
/// 2. All other top-level keys: orchestrator values pass through unchanged (coop never sets these)
///
/// Returns the merged settings as a JSON value.
pub fn merge_settings(
    orchestrator: &serde_json::Value,
    coop: serde_json::Value,
) -> serde_json::Value {
    let mut merged = orchestrator.clone();

    let Some(coop_hooks) = coop.get("hooks").and_then(|h| h.as_object()) else {
        return merged;
    };

    // Ensure merged has a hooks object
    let merged_obj = match merged.as_object_mut() {
        Some(obj) => obj,
        None => return coop,
    };
    if !merged_obj.contains_key("hooks") {
        merged_obj.insert("hooks".to_string(), serde_json::json!({}));
    }
    let merged_hooks = merged_obj.get_mut("hooks").and_then(|h| h.as_object_mut());
    let Some(merged_hooks) = merged_hooks else {
        return merged;
    };

    for (hook_type, coop_entries) in coop_hooks {
        let Some(coop_arr) = coop_entries.as_array() else {
            continue;
        };
        match merged_hooks.get_mut(hook_type) {
            Some(existing) => {
                if let Some(existing_arr) = existing.as_array_mut() {
                    existing_arr.extend(coop_arr.iter().cloned());
                }
            }
            None => {
                merged_hooks.insert(hook_type.clone(), coop_entries.clone());
            }
        }
    }

    merged
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
