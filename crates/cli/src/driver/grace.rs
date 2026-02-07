// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::time::{Duration, Instant};

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
