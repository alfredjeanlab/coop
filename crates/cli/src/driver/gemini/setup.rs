// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Pre-spawn preparation for `--agent gemini` sessions.
//!
//! Centralizes settings file writing and FIFO pipe setup. Must run
//! **before** spawning the backend so the child process finds the
//! FIFO and settings on startup.

use std::path::{Path, PathBuf};

use super::hooks::generate_hook_config;
use crate::config::{AgentSettings, McpConfig};

/// Everything needed to spawn a Gemini session.
pub struct GeminiSessionSetup {
    /// Path to the named FIFO pipe (for Tier 1 hook detection).
    pub hook_pipe_path: PathBuf,
    /// Environment variables to set on the child process.
    pub env_vars: Vec<(String, String)>,
    /// Extra CLI arguments to append to the Gemini command.
    pub extra_args: Vec<String>,
    /// Keeps the temp directory (pipe + settings) alive for the session.
    _temp_dir: tempfile::TempDir,
}

/// Prepare a fresh Gemini session.
///
/// Writes a settings file with hook config and creates the pipe path.
/// The settings file is injected via `GEMINI_CLI_SYSTEM_SETTINGS_PATH`
/// so hooks are active without modifying user or project settings.
/// If `base_settings` is provided, orchestrator hooks are layered
/// beneath coop's detection hooks in the merged settings file.
/// If `mcp_config` is provided, its `mcpServers` are included in the
/// settings file (Gemini reads MCP servers from settings, not a separate file).
pub fn prepare_gemini_session(
    _working_dir: &Path,
    coop_url: &str,
    base_settings: Option<&AgentSettings>,
    mcp_config: Option<&McpConfig>,
) -> anyhow::Result<GeminiSessionSetup> {
    let temp_dir = tempfile::tempdir()?;
    let hook_pipe_path = temp_dir.path().join("hook.pipe");
    let settings_path =
        write_settings_file(temp_dir.path(), &hook_pipe_path, base_settings, mcp_config)?;

    let mut env_vars = super::hooks::hook_env_vars(&hook_pipe_path, coop_url);
    // Inject settings file via system settings path so Gemini loads our hooks
    env_vars
        .push(("GEMINI_CLI_SYSTEM_SETTINGS_PATH".to_string(), settings_path.display().to_string()));

    Ok(GeminiSessionSetup { hook_pipe_path, env_vars, extra_args: vec![], _temp_dir: temp_dir })
}

/// Write a Gemini settings JSON file containing the hook configuration.
///
/// If `base_settings` is provided, merges orchestrator hooks (base)
/// with coop's hooks (appended). If `mcp_config` is provided, its
/// `mcpServers` key is included in the settings file. Returns the path
/// to the written file.
fn write_settings_file(
    dir: &Path,
    pipe_path: &Path,
    base_settings: Option<&AgentSettings>,
    mcp_config: Option<&McpConfig>,
) -> anyhow::Result<PathBuf> {
    let coop_config = generate_hook_config(pipe_path);
    let mut merged = match base_settings {
        Some(orch) => crate::config::merge_settings(orch, coop_config),
        None => coop_config,
    };
    // Gemini reads mcpServers from settings.json (not a separate file).
    // The `mcp` field holds the server map directly.
    if let Some(mcp) = mcp_config {
        let mcp_value = serde_json::to_value(mcp)?;
        merged.extra.insert("mcpServers".to_string(), mcp_value);
    }
    let path = dir.join("coop-gemini-settings.json");
    let contents = serde_json::to_string_pretty(&merged)?;
    std::fs::write(&path, contents)?;
    Ok(path)
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
