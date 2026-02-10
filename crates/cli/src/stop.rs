// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Agent-agnostic stop hook configuration and gating logic.
//!
//! The stop hook becomes a gating HTTP call: the hook script `curl`s coop,
//! which returns a verdict (`{}` for allow, `{"decision":"block","reason":"..."}`
//! for block). A signal endpoint lets orchestrators unblock the next stop check.

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, RwLock};

/// Top-level stop hook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopConfig {
    /// How to handle stop hook calls.
    #[serde(default)]
    pub mode: StopMode,
    /// Custom prompt text included in the block reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Schema describing the expected signal body fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<StopSchema>,
}

impl Default for StopConfig {
    fn default() -> Self {
        Self { mode: StopMode::Allow, prompt: None, schema: None }
    }
}

/// Stop hook mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopMode {
    /// Always allow the agent to stop (default behavior).
    #[default]
    Allow,
    /// Block stops until a signal is received via the signal endpoint.
    Signal,
}

/// Schema describing expected fields in the signal body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopSchema {
    /// Named fields the signal body should contain.
    pub fields: BTreeMap<String, StopSchemaField>,
}

/// A single field in the stop schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopSchemaField {
    /// Whether this field is required.
    #[serde(default)]
    pub required: bool,
    /// Allowed values (if restricted to an enum).
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub r#enum: Option<Vec<String>>,
    /// Per-value descriptions for enum fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptions: Option<BTreeMap<String, String>>,
    /// Field-level description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A stop verdict event emitted to WebSocket/gRPC consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopEvent {
    /// What happened at the stop check.
    #[serde(rename = "type")]
    pub r#type: StopType,
    /// Signal body (when type is Signaled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<Value>,
    /// Error details (when type is Error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    /// Monotonic sequence number.
    pub seq: u64,
}

/// Classification of a stop verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopType {
    /// Signal was received; agent is allowed to stop.
    Signaled,
    /// Agent is in an unrecoverable error state; allowed to stop.
    Error,
    /// Claude's safety valve (`stop_hook_active`) triggered; must allow.
    SafetyValve,
    /// Stop was blocked; agent should continue working.
    Blocked,
    /// Mode is `allow`; agent is always allowed to stop.
    Allowed,
}

impl StopType {
    /// Wire-format string for this type.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Signaled => "signaled",
            Self::Error => "error",
            Self::SafetyValve => "safety_valve",
            Self::Blocked => "blocked",
            Self::Allowed => "allowed",
        }
    }
}

impl std::fmt::Display for StopType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Runtime state for the stop hook gating system.
pub struct StopState {
    /// Mutable stop config (can be changed at runtime via API).
    pub config: RwLock<StopConfig>,
    /// Fast check: has a signal been received?
    pub signaled: AtomicBool,
    /// The signal body stored by the signal endpoint.
    pub signal_body: RwLock<Option<Value>>,
    /// Broadcast channel for stop events.
    pub stop_tx: broadcast::Sender<StopEvent>,
    /// Precomputed resolve URL for block reason generation.
    pub resolve_url: String,
    /// Monotonic sequence counter for stop events.
    pub stop_seq: std::sync::atomic::AtomicU64,
}

impl StopState {
    /// Create a new `StopState` with the given initial config and resolve URL.
    pub fn new(config: StopConfig, resolve_url: String) -> Self {
        let (stop_tx, _) = broadcast::channel(crate::run::SIGNAL_CHANNEL_CAPACITY);
        Self {
            config: RwLock::new(config),
            signaled: AtomicBool::new(false),
            signal_body: RwLock::new(None),
            stop_tx,
            resolve_url,
            stop_seq: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Emit a stop event to all subscribers and return it.
    pub fn emit(
        &self,
        r#type: StopType,
        signal: Option<Value>,
        error_detail: Option<String>,
    ) -> StopEvent {
        let seq = self.stop_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let event = StopEvent { r#type, signal, error_detail, seq };
        // Ignore send errors (no receivers is fine).
        let _ = self.stop_tx.send(event.clone());
        event
    }
}

impl std::fmt::Debug for StopState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StopState")
            .field("signaled", &self.signaled.load(std::sync::atomic::Ordering::Relaxed))
            .field("resolve_url", &self.resolve_url)
            .finish()
    }
}

/// Assemble the block reason text from the stop config.
///
/// This is the `reason` field returned in `{"decision":"block","reason":"..."}`.
/// It tells the agent what to do: run `coop send` with the right body.
///
/// The output is designed to be directly actionable — concrete `coop send`
/// commands the agent can copy-paste.
pub fn generate_block_reason(config: &StopConfig) -> String {
    let mut parts = Vec::new();

    // Find enum fields eligible for command expansion.
    let primary_enum = config.schema.as_ref().and_then(|schema| {
        let enum_fields: Vec<_> =
            schema.fields.iter().filter(|(_, f)| f.r#enum.is_some()).collect();
        if enum_fields.len() == 1 {
            let (name, field) = enum_fields[0];
            Some((name.clone(), field.clone()))
        } else {
            None
        }
    });

    if let Some((enum_name, enum_field)) = primary_enum {
        // Custom prompt (or default directive)
        if let Some(ref prompt) = config.prompt {
            parts.push(prompt.clone());
        }

        // Expand into one `coop send` per enum value.
        parts.push("Please confirm by running one of:".to_owned());
        let values = enum_field.r#enum.as_deref().unwrap_or_default();
        let descs = enum_field.descriptions.as_ref();
        for (i, v) in values.iter().enumerate() {
            let body = generate_example_body(config.schema.as_ref(), Some((&enum_name, v)));
            if let Some(vd) = descs.and_then(|d| d.get(v)) {
                parts.push(format!("{}. {vd}", i + 1));
            } else {
                parts.push(format!("{}. {v}", i + 1));
            }
            parts.push(format!("    `coop send '{body}'`"));
        }
    } else {
        // No enum to expand — custom prompt, schema example, or defaults.
        if let Some(ref prompt) = config.prompt {
            parts.push(prompt.clone());
        }

        let has_schema = config.schema.as_ref().map(|s| !s.fields.is_empty()).unwrap_or(false);
        let prompt_has_json =
            config.prompt.as_ref().map(|p| p.contains('{') && p.contains('}')).unwrap_or(false);

        if has_schema {
            // User defined a schema — show a single example from it.
            let body = generate_example_body(config.schema.as_ref(), None);
            parts.push(format!("When ready to stop, run: `coop send '{body}'`"));
        } else if prompt_has_json {
            // Prompt already contains inline JSON examples.
            parts.push("When ready to stop, run: `coop send '<json>'`".to_owned());
        } else {
            // No schema, no inline JSON — provide sensible defaults.
            parts.push("Follow the instructions provided in the stop hook above, then:".to_owned());
            parts.push("1. If you are done, use the Bash tool:".to_owned());
            parts.push(
                "    `coop send '{\"status\":\"done\",\"message\":\"<summary>\"}'`".to_owned(),
            );
            parts.push("2. If you are still working, use the Bash tool:".to_owned());
            parts.push(
                "    `coop send '{\"status\":\"continue\",\"message\":\"<what remains>\"}'`"
                    .to_owned(),
            );
            parts.push("3. If you need clarification, use the AskUserQuestion tool.".to_owned());
        }
    }

    parts.join("\n")
}

/// Build an example JSON body string from a schema.
///
/// When `enum_override` is provided, that field uses the given value; other
/// fields use their first enum value or a `<name>` placeholder.
fn generate_example_body(
    schema: Option<&StopSchema>,
    enum_override: Option<(&str, &str)>,
) -> String {
    let schema = match schema {
        Some(s) if !s.fields.is_empty() => s,
        _ => return "{}".to_owned(),
    };

    let mut obj = serde_json::Map::new();
    for (name, field) in &schema.fields {
        let val = if let Some((override_name, override_val)) = enum_override {
            if name == override_name {
                override_val.to_owned()
            } else if let Some(ref values) = field.r#enum {
                values.first().cloned().unwrap_or_default()
            } else {
                format!("<{name}>")
            }
        } else if let Some(ref values) = field.r#enum {
            values.first().cloned().unwrap_or_default()
        } else {
            format!("<{name}>")
        };
        obj.insert(name.clone(), Value::String(val));
    }
    serde_json::to_string(&Value::Object(obj)).unwrap_or_else(|_| "{}".to_owned())
}

#[cfg(test)]
#[path = "stop_tests.rs"]
mod tests;
