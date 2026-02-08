// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::screen::ScreenSnapshot;

use super::{AgentState, Detector};

/// User-provided JSON configuration for screen pattern matching.
#[derive(Debug, Clone, Deserialize)]
pub struct ScreenPatternConfig {
    pub prompt_pattern: Option<String>,
    #[serde(default)]
    pub working_patterns: Vec<String>,
    #[serde(default)]
    pub error_patterns: Vec<String>,
}

/// Compiled regex patterns for screen classification.
pub struct ScreenPatterns {
    pub prompt: Option<Regex>,
    pub working: Vec<Regex>,
    pub error: Vec<Regex>,
}

impl std::fmt::Debug for ScreenPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScreenPatterns")
            .field("prompt", &self.prompt.as_ref().map(|r| r.as_str()))
            .field("working", &self.working.len())
            .field("error", &self.error.len())
            .finish()
    }
}

/// Load and compile screen patterns from a JSON config file.
pub fn load_agent_config(path: &Path) -> anyhow::Result<ScreenPatterns> {
    let contents = std::fs::read_to_string(path)?;
    let config: ScreenPatternConfig = serde_json::from_str(&contents)?;
    compile_config(&config)
}

/// Compile a `ScreenPatternConfig` into `ScreenPatterns`.
pub fn compile_config(config: &ScreenPatternConfig) -> anyhow::Result<ScreenPatterns> {
    let prompt = config
        .prompt_pattern
        .as_deref()
        .map(Regex::new)
        .transpose()?;

    let working = config
        .working_patterns
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<Vec<_>, _>>()?;

    let error = config
        .error_patterns
        .iter()
        .map(|p| Regex::new(p))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ScreenPatterns {
        prompt,
        working,
        error,
    })
}

/// Classify the current screen state based on pattern matches.
///
/// Priority: Error > Prompt (last non-empty line) > Working.
/// Returns `None` if no patterns match.
pub fn classify(patterns: &ScreenPatterns, snapshot: &ScreenSnapshot) -> Option<AgentState> {
    // Check error patterns across all lines (highest priority)
    for line in &snapshot.lines {
        for pat in &patterns.error {
            if pat.is_match(line) {
                return Some(AgentState::Error {
                    detail: line.clone(),
                });
            }
        }
    }

    // Check prompt pattern on the last non-empty line
    if let Some(ref prompt_re) = patterns.prompt {
        let last_non_empty = snapshot.lines.iter().rev().find(|l| !l.trim().is_empty());
        if let Some(line) = last_non_empty {
            if prompt_re.is_match(line) {
                return Some(AgentState::WaitingForInput);
            }
        }
    }

    // Check working patterns across all lines
    for line in &snapshot.lines {
        for pat in &patterns.working {
            if pat.is_match(line) {
                return Some(AgentState::Working);
            }
        }
    }

    None
}

/// Tier 5 detector that parses rendered screen content with user-configured
/// regex patterns to classify agent state.
pub struct ScreenParser {
    patterns: ScreenPatterns,
    snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    poll_interval: Duration,
}

impl ScreenParser {
    pub fn new(
        patterns: ScreenPatterns,
        snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    ) -> Self {
        Self {
            patterns,
            snapshot_fn,
            poll_interval: Duration::from_secs(2),
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Detector for ScreenParser {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(self.poll_interval);
            let mut last_state: Option<AgentState> = None;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }

                let snapshot = (self.snapshot_fn)();
                let new_state = classify(&self.patterns, &snapshot);

                if let Some(ref state) = new_state {
                    if last_state.as_ref() != Some(state) {
                        let _ = state_tx.send(state.clone()).await;
                        last_state = new_state;
                    }
                } else if last_state.is_some() {
                    last_state = None;
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        5
    }
}

#[cfg(test)]
#[path = "screen_parse_tests.rs"]
mod tests;
