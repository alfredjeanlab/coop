// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Pre-spawn preparation for `--agent claude` sessions.
//!
//! Centralizes session ID generation, log path computation, settings file
//! writing, and FIFO pipe setup. Must run **before** spawning the backend
//! so the child process finds the FIFO and settings on startup.

use std::path::{Path, PathBuf};

use super::hooks::generate_hook_config;
use crate::driver::SessionSetup;

/// Prepare a Claude session setup.
///
/// Dispatches to the appropriate preparation path based on mode:
/// - **pristine**: no FIFO or hooks, but keeps session-id + log path for Tier 2.
/// - **resume**: reuses existing log path and conversation ID.
/// - **fresh**: generates new session ID, hooks, and settings.
pub fn prepare(
    working_dir: &Path,
    coop_url: &str,
    base_settings: Option<&serde_json::Value>,
    mcp_config: Option<&serde_json::Value>,
    pristine: bool,
    resume_log: Option<&Path>,
) -> anyhow::Result<SessionSetup> {
    if pristine {
        prepare_pristine(working_dir, coop_url, base_settings, mcp_config)
    } else if let Some(log_path) = resume_log {
        prepare_resume(log_path, coop_url, base_settings, mcp_config)
    } else {
        prepare_fresh(working_dir, coop_url, base_settings, mcp_config)
    }
}

/// Prepare a fresh Claude session.
fn prepare_fresh(
    working_dir: &Path,
    coop_url: &str,
    base_settings: Option<&serde_json::Value>,
    mcp_config: Option<&serde_json::Value>,
) -> anyhow::Result<SessionSetup> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let log_path = session_log_path(working_dir, &session_id);

    let session_dir = crate::driver::coop_session_dir(&session_id)?;
    let hook_pipe_path = session_dir.join("hook.pipe");
    let settings_path = write_settings_file(&session_dir, &hook_pipe_path, base_settings)?;

    let env_vars = crate::driver::hook_env_vars(&hook_pipe_path, coop_url);
    let mut extra_args = vec![
        "--session-id".to_owned(),
        session_id.clone(),
        "--settings".to_owned(),
        settings_path.display().to_string(),
    ];

    if let Some(mcp) = mcp_config {
        write_mcp_config(&session_dir, mcp, &mut extra_args)?;
    }

    Ok(SessionSetup {
        session_id,
        hook_pipe_path: Some(hook_pipe_path),
        session_log_path: Some(log_path),
        session_dir,
        env_vars,
        extra_args,
    })
}

/// Prepare a resumed Claude session.
fn prepare_resume(
    existing_log_path: &Path,
    coop_url: &str,
    base_settings: Option<&serde_json::Value>,
    mcp_config: Option<&serde_json::Value>,
) -> anyhow::Result<SessionSetup> {
    let session_id =
        existing_log_path.file_stem().and_then(|s| s.to_str()).unwrap_or("unknown").to_owned();
    let session_dir = crate::driver::coop_session_dir(&session_id)?;
    let hook_pipe_path = session_dir.join("hook.pipe");
    let settings_path = write_settings_file(&session_dir, &hook_pipe_path, base_settings)?;

    let env_vars = crate::driver::hook_env_vars(&hook_pipe_path, coop_url);

    let mut extra_args = super::resume::resume_args(&session_id);
    extra_args.push("--settings".to_owned());
    extra_args.push(settings_path.display().to_string());

    if let Some(mcp) = mcp_config {
        write_mcp_config(&session_dir, mcp, &mut extra_args)?;
    }

    Ok(SessionSetup {
        session_id,
        hook_pipe_path: Some(hook_pipe_path),
        session_log_path: Some(existing_log_path.to_path_buf()),
        session_dir,
        env_vars,
        extra_args,
    })
}

/// Prepare a Claude session in pristine mode (no FIFO, no hooks).
fn prepare_pristine(
    working_dir: &Path,
    coop_url: &str,
    base_settings: Option<&serde_json::Value>,
    mcp_config: Option<&serde_json::Value>,
) -> anyhow::Result<SessionSetup> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let log_path = session_log_path(working_dir, &session_id);
    let session_dir = crate::driver::coop_session_dir(&session_id)?;

    let mut extra_args = vec!["--session-id".to_owned(), session_id.clone()];

    // Write orchestrator settings as-is (no coop hooks merged).
    if let Some(settings) = base_settings {
        let path = session_dir.join("coop-settings.json");
        std::fs::write(&path, serde_json::to_string_pretty(settings)?)?;
        extra_args.push("--settings".to_owned());
        extra_args.push(path.display().to_string());
    }

    if let Some(mcp) = mcp_config {
        write_mcp_config(&session_dir, mcp, &mut extra_args)?;
    }

    Ok(SessionSetup {
        session_id,
        hook_pipe_path: None,
        session_log_path: Some(log_path),
        session_dir,
        env_vars: vec![("COOP_URL".to_string(), coop_url.to_string())],
        extra_args,
    })
}

/// Write Claude MCP config file and append `--mcp-config` to extra args.
fn write_mcp_config(
    session_dir: &Path,
    mcp: &serde_json::Value,
    extra_args: &mut Vec<String>,
) -> anyhow::Result<()> {
    let wrapped = serde_json::json!({ "mcpServers": mcp });
    let mcp_path = session_dir.join("mcp.json");
    std::fs::write(&mcp_path, serde_json::to_string_pretty(&wrapped)?)?;
    tracing::info!("wrote Claude MCP config to {}", mcp_path.display());
    extra_args.push("--mcp-config".to_owned());
    extra_args.push(mcp_path.display().to_string());
    Ok(())
}

/// Write a Claude settings JSON file containing the hook configuration.
///
/// If `base_settings` is provided, merges orchestrator hooks (base)
/// with coop's hooks (appended). Returns the path to the written file.
fn write_settings_file(
    dir: &Path,
    pipe_path: &Path,
    base_settings: Option<&serde_json::Value>,
) -> anyhow::Result<PathBuf> {
    let coop_config = generate_hook_config(pipe_path);
    let mut merged = match base_settings {
        Some(orch) => crate::config::merge_settings(orch, coop_config),
        None => coop_config,
    };
    // Ensure `coop send` is allowed — the stop hook tells the agent to run it.
    inject_coop_permissions(&mut merged);
    let path = dir.join("coop-settings.json");
    let contents = serde_json::to_string_pretty(&merged)?;
    std::fs::write(&path, contents)?;
    Ok(path)
}

/// Append coop's own permission rules to the merged settings.
///
/// The stop hook block reason tells the agent to run `coop send '...'`,
/// so coop must ensure that command is pre-approved.
fn inject_coop_permissions(config: &mut serde_json::Value) {
    use serde_json::json;

    let rule = json!("Bash(coop send:*)");

    // Navigate to config.permissions.allow, creating along the way.
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let perms = obj.entry("permissions").or_insert_with(|| json!({}));
    let Some(perms_obj) = perms.as_object_mut() else {
        return;
    };
    let allow = perms_obj.entry("allow").or_insert_with(|| json!([]));
    if let Some(arr) = allow.as_array_mut() {
        if !arr.contains(&rule) {
            arr.push(rule);
        }
    }
}

/// Write (or update) Claude's `.credentials.json` with an OAuth access token.
///
/// If the file already exists, merges the new token into the existing
/// `claudeAiOauth` object, preserving fields like `refreshToken` and `scopes`.
/// If the file does not exist, creates a minimal credential structure.
///
/// This bridges the gap between environment-variable-based credential delivery
/// (used by orchestrators via session/switch) and Claude Code's file-based
/// credential reading.
pub fn write_credentials_file(access_token: &str) -> anyhow::Result<PathBuf> {
    let config_dir = claude_config_dir();
    std::fs::create_dir_all(&config_dir)?;
    let cred_path = config_dir.join(".credentials.json");

    let mut creds: serde_json::Value = if cred_path.exists() {
        let contents = std::fs::read_to_string(&cred_path)?;
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure claudeAiOauth object exists.
    let oauth = creds
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("credentials file is not a JSON object"))?
        .entry("claudeAiOauth")
        .or_insert_with(|| {
            serde_json::json!({
                "accessToken": "",
                "refreshToken": "",
                "expiresAt": 9999999999999u64,
                "scopes": [
                    "user:inference",
                    "user:profile",
                    "user:sessions:claude_code",
                    "user:mcp_servers",
                ],
            })
        });

    // Update the access token and reset the expiry.
    // The broker manages token refresh, so set expiresAt far into the future
    // to prevent Claude Code from treating the token as expired.
    if let Some(obj) = oauth.as_object_mut() {
        obj.insert("accessToken".to_owned(), serde_json::Value::String(access_token.to_owned()));
        obj.insert("expiresAt".to_owned(), serde_json::json!(9_999_999_999_999u64));
    }

    std::fs::write(&cred_path, serde_json::to_string_pretty(&creds)?)?;
    tracing::info!("wrote OAuth credentials to {}", cred_path.display());
    Ok(cred_path)
}

/// Return Claude's config directory.
///
/// Respects `CLAUDE_CONFIG_DIR` if set, otherwise defaults to `$HOME/.claude`.
pub(crate) fn claude_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".claude")
}

/// Compute the expected session log path for a given working directory
/// and session ID.
///
/// Claude stores logs at `<config-dir>/projects/<project-dir-name>/<uuid>.jsonl`.
pub(crate) fn session_log_path(working_dir: &Path, session_id: &str) -> PathBuf {
    let dir_name = project_dir_name(working_dir);
    claude_config_dir().join("projects").join(dir_name).join(format!("{session_id}.jsonl"))
}

/// Convert a working directory path into Claude's project directory name.
///
/// Canonicalizes the path, then replaces `/` with `-` and strips the
/// leading `-` (matching Claude's internal convention).
pub fn project_dir_name(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.display().to_string();
    s.replace(['/', '.'], "-")
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
