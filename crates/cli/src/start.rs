// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent-agnostic start hook configuration and context injection.
//!
//! When a session lifecycle event fires (startup, resume, clear, compact),
//! the hook script `curl`s coop's `/api/v1/hooks/start` endpoint. Coop
//! composes a shell script from `text` and `shell` config, returns it as
//! plain text, and the hook `eval`s the response.

use std::collections::BTreeMap;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::run::SIGNAL_CHANNEL_CAPACITY;
use tokio::sync::RwLock;

/// Top-level start hook configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartConfig {
    /// Static text to inject (delivered as base64-decoded printf).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Shell commands to run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
    /// Per-event overrides keyed by source (e.g. "clear", "resume").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub event: BTreeMap<String, StartEventConfig>,
}

/// Per-event override configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StartEventConfig {
    /// Static text to inject for this event type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Shell commands to run for this event type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shell: Vec<String>,
}

/// A start hook event emitted to WebSocket/gRPC consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartEvent {
    /// Source of the lifecycle event (e.g. "start", "resume", "clear").
    pub source: String,
    /// Session ID if available from the hook data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether a non-empty script was injected.
    pub injected: bool,
    /// Monotonic sequence number.
    pub seq: u64,
}

/// Runtime state for the start hook system.
pub struct StartState {
    /// Mutable start config (can be changed at runtime via API).
    pub config: RwLock<StartConfig>,
    /// Broadcast channel for start events.
    pub start_tx: broadcast::Sender<StartEvent>,
    /// Monotonic sequence counter for start events.
    pub start_seq: std::sync::atomic::AtomicU64,
}

impl StartState {
    /// Create a new `StartState` with the given initial config.
    pub fn new(config: StartConfig) -> Self {
        let (start_tx, _) = broadcast::channel(SIGNAL_CHANNEL_CAPACITY);
        Self {
            config: RwLock::new(config),
            start_tx,
            start_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Emit a start event to all subscribers and return it.
    pub fn emit(&self, source: String, session_id: Option<String>, injected: bool) -> StartEvent {
        let seq = self.start_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let event = StartEvent { source, session_id, injected, seq };
        // Ignore send errors (no receivers is fine).
        let _ = self.start_tx.send(event.clone());
        event
    }
}

impl std::fmt::Debug for StartState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StartState").finish()
    }
}

/// Compose a shell script from the start config for a given event source.
///
/// Lookup: match `event[source]` first → fall back to top-level `text`/`shell`.
/// Returns empty string if no injection is configured.
///
/// For `text`: `printf '%s' '<base64>' | base64 -d`
/// For `shell`: each command on its own line
/// Both: text first, then commands
pub fn compose_start_script(config: &StartConfig, source: &str) -> String {
    let (text, shell) = if let Some(event_config) = config.event.get(source) {
        (&event_config.text, &event_config.shell)
    } else {
        (&config.text, &config.shell)
    };

    let mut parts = Vec::new();

    if let Some(ref t) = text {
        if !t.is_empty() {
            let encoded = base64::engine::general_purpose::STANDARD.encode(t.as_bytes());
            parts.push(format!("printf '%s' '{encoded}' | base64 -d"));
        }
    }

    for cmd in shell {
        if !cmd.is_empty() {
            parts.push(cmd.clone());
        }
    }

    parts.join("\n")
}

#[cfg(test)]
#[path = "start_tests.rs"]
mod tests;
