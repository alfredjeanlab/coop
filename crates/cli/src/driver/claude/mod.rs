// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod detect;
pub mod encoding;
pub mod hooks;
pub mod prompt;
pub mod state;

use std::path::PathBuf;

use bytes::Bytes;
use tokio::sync::mpsc;

use super::hook_recv::HookReceiver;
use super::Detector;
use detect::{HookDetector, LogDetector, StdoutDetector};
use encoding::{ClaudeNudgeEncoder, ClaudeRespondEncoder};

/// Configuration for building a [`ClaudeDriver`].
pub struct ClaudeDriverConfig {
    /// Path to Claude's session log file (Tier 2).
    pub session_log_path: Option<PathBuf>,
    /// Path for the hook named pipe (Tier 1).
    pub hook_pipe_path: Option<PathBuf>,
    /// Channel for raw stdout JSONL bytes (Tier 3).
    /// Used when Claude runs with `--print --output-format stream-json`.
    pub stdout_rx: Option<mpsc::Receiver<Bytes>>,
}

/// Claude Code agent driver.
///
/// Provides encoding for nudge/respond actions and detection tiers
/// for monitoring Claude's agent state.
pub struct ClaudeDriver {
    pub nudge: ClaudeNudgeEncoder,
    pub respond: ClaudeRespondEncoder,
    pub detectors: Vec<Box<dyn Detector>>,
    /// Stored for `env_vars()`; the pipe path must stay available.
    hook_pipe_path: Option<PathBuf>,
}

impl ClaudeDriver {
    /// Build a new driver from the given configuration.
    ///
    /// Constructs detectors based on available tiers:
    /// - Tier 1 (HookDetector): if `hook_pipe_path` is set
    /// - Tier 2 (LogDetector): if `session_log_path` is set
    /// - Tier 3 (StdoutDetector): if `stdout_rx` is provided
    pub fn new(config: ClaudeDriverConfig) -> anyhow::Result<Self> {
        let mut detectors: Vec<Box<dyn Detector>> = Vec::new();
        let hook_pipe_path = config.hook_pipe_path.clone();

        // Tier 1: Hook events (highest confidence)
        if let Some(pipe_path) = config.hook_pipe_path {
            let receiver = HookReceiver::new(&pipe_path)?;
            detectors.push(Box::new(HookDetector { receiver }));
        }

        // Tier 2: Session log watching
        if let Some(log_path) = config.session_log_path {
            detectors.push(Box::new(LogDetector { log_path }));
        }

        // Tier 3: Structured stdout JSONL
        if let Some(stdout_rx) = config.stdout_rx {
            detectors.push(Box::new(StdoutDetector { stdout_rx }));
        }

        // Sort by tier (lowest number = highest priority)
        detectors.sort_by_key(|d| d.tier());

        Ok(Self {
            nudge: ClaudeNudgeEncoder,
            respond: ClaudeRespondEncoder,
            detectors,
            hook_pipe_path,
        })
    }

    /// Consume the driver and return its detectors.
    pub fn into_detectors(self) -> Vec<Box<dyn Detector>> {
        self.detectors
    }

    /// Return environment variables needed by the Claude child process.
    pub fn env_vars(&self) -> Vec<(String, String)> {
        match &self.hook_pipe_path {
            Some(path) => hooks::hook_env_vars(path),
            None => vec![],
        }
    }
}
