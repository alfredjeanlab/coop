// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::path::Path;
use std::sync::Arc;

use crate::screen::ScreenSnapshot;

use super::process::ProcessMonitor;
use super::screen_parse::{load_agent_config, ScreenParser};
use super::{Detector, NudgeEncoder, RespondEncoder};

/// Build detectors for the unknown driver.
///
/// Always includes a Tier 4 `ProcessMonitor`. When `agent_config` is provided,
/// also includes a Tier 5 `ScreenParser` (requires `snapshot_fn`).
pub fn build_detectors(
    child_pid: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written: Arc<dyn Fn() -> u64 + Send + Sync>,
    agent_config: Option<&Path>,
    snapshot_fn: Option<Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>>,
) -> anyhow::Result<Vec<Box<dyn Detector>>> {
    let mut detectors: Vec<Box<dyn Detector>> = Vec::new();

    detectors.push(Box::new(ProcessMonitor::new(child_pid, ring_total_written)));

    if let Some(config_path) = agent_config {
        let snap_fn = snapshot_fn.ok_or_else(|| {
            anyhow::anyhow!("snapshot_fn is required when agent_config is provided")
        })?;
        let patterns = load_agent_config(config_path)?;
        detectors.push(Box::new(ScreenParser::new(patterns, snap_fn)));
    }

    Ok(detectors)
}

/// Unknown driver does not support nudge.
pub fn nudge_encoder() -> Option<Box<dyn NudgeEncoder>> {
    None
}

/// Unknown driver does not support respond.
pub fn respond_encoder() -> Option<Box<dyn RespondEncoder>> {
    None
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
