// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::{Duration, Instant};

/// Result of checking the grace timer state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraceCheck {
    /// No grace period is active.
    NotPending,
    /// Grace period is active but has not yet elapsed.
    Waiting,
    /// Grace elapsed AND log size unchanged — idle confirmed.
    Confirmed,
    /// Log grew during grace period — not idle.
    Invalidated,
}

/// Timer that enforces a grace period before transitioning to idle state.
pub struct IdleGraceTimer {
    pub duration: Duration,
    pub pending: Option<GraceState>,
}

/// Snapshot of when the grace period was triggered.
pub struct GraceState {
    pub triggered_at: Instant,
    pub log_size_at_trigger: u64,
}

impl IdleGraceTimer {
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            pending: None,
        }
    }

    /// Start a grace period, recording the current log size.
    pub fn trigger(&mut self, log_size: u64) {
        self.pending = Some(GraceState {
            triggered_at: Instant::now(),
            log_size_at_trigger: log_size,
        });
    }

    /// Cancel the grace period (activity resumed).
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// Check whether the grace period has elapsed and the log is unchanged.
    pub fn check(&self, current_log_size: u64) -> GraceCheck {
        let Some(ref state) = self.pending else {
            return GraceCheck::NotPending;
        };

        if current_log_size != state.log_size_at_trigger {
            return GraceCheck::Invalidated;
        }

        if state.triggered_at.elapsed() >= self.duration {
            GraceCheck::Confirmed
        } else {
            GraceCheck::Waiting
        }
    }

    /// Returns `true` if a grace period is currently pending.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

impl std::fmt::Debug for IdleGraceTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdleGraceTimer")
            .field("duration", &self.duration)
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

impl std::fmt::Debug for GraceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraceState")
            .field("triggered_at", &self.triggered_at)
            .field("log_size_at_trigger", &self.log_size_at_trigger)
            .finish()
    }
}

#[cfg(test)]
#[path = "grace_tests.rs"]
mod tests;
