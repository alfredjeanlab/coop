// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session resume support for Claude Code.
//!
//! When a coop process restarts, it can reconnect to a previous Claude
//! session by discovering the session log and passing `--continue` with
//! the session ID.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::driver::AgentState;

/// Recovered state from a previous session log.
#[derive(Debug, Clone)]
pub struct ResumeState {
    /// Last known agent state from the log.
    pub last_state: AgentState,
    /// Byte offset to resume log detection from.
    pub log_offset: u64,
    /// Claude session/conversation ID recovered from the log.
    pub conversation_id: Option<String>,
}

/// Find the most recent Claude session log for a workspace.
///
/// Scans `~/.claude/projects/<workspace-hash>/` for the latest `.jsonl` file.
/// The `workspace_hint` is a path or identifier used to locate the session
/// directory.
pub fn discover_session_log(workspace_hint: &str) -> anyhow::Result<Option<PathBuf>> {
    // Try the hint directly as a path first.
    let direct = Path::new(workspace_hint);
    if direct.is_file() && matches!(direct.extension().and_then(|e| e.to_str()), Some("jsonl")) {
        return Ok(Some(direct.to_path_buf()));
    }

    // Scan the Claude projects directory for session logs.
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return Ok(None);
    }

    let projects_dir = Path::new(&home).join(".claude").join("projects");
    if !projects_dir.is_dir() {
        return Ok(None);
    }

    // Build candidate directories: look for a hash that matches the workspace hint,
    // or scan all project directories.
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&projects_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                // Match if the directory name contains the workspace hint
                // (Claude uses a hash of the workspace path as directory name).
                if dir_name.contains(workspace_hint) || workspace_hint.contains(&dir_name) {
                    candidates.push(path);
                }
            }
        }
    }

    // Find the most recent .jsonl file across all candidate directories.
    let mut best: Option<(PathBuf, std::time::SystemTime)> = None;

    for dir in &candidates {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            if best.as_ref().is_none_or(|(_, prev)| modified > *prev) {
                                best = Some((path, modified));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(best.map(|(path, _)| path))
}

/// Parse a session log to determine the last known agent state and resume offset.
///
/// Reads the log file and processes each JSONL entry to build the resume state.
pub fn parse_resume_state(log_path: &Path) -> anyhow::Result<ResumeState> {
    let file = std::fs::File::open(log_path)?;
    let metadata = file.metadata()?;
    let file_size = metadata.len();

    let reader = BufReader::new(file);
    let mut last_state = AgentState::Starting;
    let mut conversation_id: Option<String> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) {
            // Try to extract the conversation/session ID from the first entry.
            if conversation_id.is_none() {
                if let Some(id) = json
                    .get("sessionId")
                    .or_else(|| json.get("conversationId"))
                    .and_then(|v| v.as_str())
                {
                    conversation_id = Some(id.to_owned());
                }
            }

            // Use the existing state parser to track the last state.
            if let Some(state) = super::state::parse_claude_state(&json) {
                last_state = state;
            }
        }
    }

    Ok(ResumeState {
        last_state,
        log_offset: file_size,
        conversation_id,
    })
}

/// Build additional CLI arguments for Claude's `--continue` mode.
pub fn resume_args(state: &ResumeState) -> Vec<String> {
    let mut args = vec!["--continue".to_owned()];
    if let Some(ref id) = state.conversation_id {
        args.push("--session-id".to_owned());
        args.push(id.clone());
    }
    args
}

/// Open a log file and seek to a specific byte offset for tailing.
///
/// Returns a reader positioned at the given offset.
pub fn open_log_at_offset(log_path: &Path, offset: u64) -> anyhow::Result<impl BufRead> {
    let mut file = std::fs::File::open(log_path)?;
    file.seek(SeekFrom::Start(offset))?;
    Ok(BufReader::new(file))
}

#[cfg(test)]
#[path = "resume_tests.rs"]
mod tests;
