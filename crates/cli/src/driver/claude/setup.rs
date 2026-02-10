// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Pre-spawn preparation for `--agent claude` sessions.
//!
//! Centralizes session ID generation, log path computation, settings file
//! writing, and FIFO pipe setup. Must run **before** spawning the backend
//! so the child process finds the FIFO and settings on startup.

use std::path::{Path, PathBuf};

use super::hooks::generate_hook_config;
use super::resume::ResumeState;
use crate::config::{AgentSettings, Permissions};

/// Everything needed to spawn (or resume) a Claude session.
pub struct ClaudeSessionSetup {
    /// Path to Claude's session log file (for Tier 2 log detection).
    pub session_log_path: PathBuf,
    /// Path to the named FIFO pipe (for Tier 1 hook detection).
    pub hook_pipe_path: PathBuf,
    /// Environment variables to set on the child process.
    pub env_vars: Vec<(String, String)>,
    /// Extra CLI arguments to append to the Claude command.
    pub extra_args: Vec<String>,
    /// Session directory containing the FIFO pipe and settings file.
    pub session_dir: PathBuf,
}

/// Prepare a fresh Claude session.
///
/// Generates a UUID for `--session-id`, computes the expected log path,
/// writes a settings file with hook config, and creates the pipe path.
/// If `base_settings` is provided, orchestrator hooks are layered
/// beneath coop's detection hooks in the merged settings file.
pub fn prepare_claude_session(
    working_dir: &Path,
    coop_url: &str,
    base_settings: Option<&AgentSettings>,
) -> anyhow::Result<ClaudeSessionSetup> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let log_path = session_log_path(working_dir, &session_id);

    let session_dir = coop_session_dir(&session_id)?;
    let hook_pipe_path = session_dir.join("hook.pipe");
    let settings_path = write_settings_file(&session_dir, &hook_pipe_path, base_settings)?;

    let env_vars = super::hooks::hook_env_vars(&hook_pipe_path, coop_url);
    let extra_args = vec![
        "--session-id".to_owned(),
        session_id,
        "--settings".to_owned(),
        settings_path.display().to_string(),
    ];

    Ok(ClaudeSessionSetup {
        session_log_path: log_path,
        hook_pipe_path,
        env_vars,
        extra_args,
        session_dir,
    })
}

/// Prepare a resumed Claude session.
///
/// Reuses the discovered log path and conversation ID. Writes a fresh
/// settings file so hooks are active in the new process.
pub fn prepare_claude_resume(
    resume_state: &ResumeState,
    existing_log_path: &Path,
    coop_url: &str,
    base_settings: Option<&AgentSettings>,
) -> anyhow::Result<ClaudeSessionSetup> {
    let resume_id = resume_state.conversation_id.as_deref().unwrap_or("unknown");
    let session_dir = coop_session_dir(resume_id)?;
    let hook_pipe_path = session_dir.join("hook.pipe");
    let settings_path = write_settings_file(&session_dir, &hook_pipe_path, base_settings)?;

    let env_vars = super::hooks::hook_env_vars(&hook_pipe_path, coop_url);

    let mut extra_args = super::resume::resume_args(resume_state);
    extra_args.push("--settings".to_owned());
    extra_args.push(settings_path.display().to_string());

    Ok(ClaudeSessionSetup {
        session_log_path: existing_log_path.to_path_buf(),
        hook_pipe_path,
        env_vars,
        extra_args,
        session_dir,
    })
}

/// Write a Claude settings JSON file containing the hook configuration.
///
/// If `base_settings` is provided, merges orchestrator hooks (base)
/// with coop's hooks (appended). Returns the path to the written file.
fn write_settings_file(
    dir: &Path,
    pipe_path: &Path,
    base_settings: Option<&AgentSettings>,
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
fn inject_coop_permissions(config: &mut AgentSettings) {
    let rule = serde_json::json!("Bash(coop send:*)");
    let perms = config.permissions.get_or_insert_with(Permissions::default);
    if !perms.allow.contains(&rule) {
        perms.allow.push(rule);
    }
}

/// Create and return the coop session directory for the given session ID.
///
/// Session artifacts (FIFO pipe, settings file) live at
/// `$XDG_STATE_HOME/coop/sessions/<session-id>/` (defaulting to
/// `~/.local/state/coop/sessions/<session-id>/`) so they survive for
/// debugging and session recovery.
pub(crate) fn coop_session_dir(session_id: &str) -> anyhow::Result<PathBuf> {
    let state_home = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{home}/.local/state")
    });
    let dir = Path::new(&state_home).join("coop").join("sessions").join(session_id);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Compute the expected session log path for a given working directory
/// and session ID.
///
/// Claude stores logs at `~/.claude/projects/<project-dir-name>/<uuid>.jsonl`.
pub(crate) fn session_log_path(working_dir: &Path, session_id: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let dir_name = project_dir_name(working_dir);
    Path::new(&home)
        .join(".claude")
        .join("projects")
        .join(dir_name)
        .join(format!("{session_id}.jsonl"))
}

/// Convert a working directory path into Claude's project directory name.
///
/// Canonicalizes the path, then replaces `/` with `-` and strips the
/// leading `-` (matching Claude's internal convention).
pub fn project_dir_name(path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let s = canonical.display().to_string();
    s.replace('/', "-").trim_start_matches('-').to_owned()
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
