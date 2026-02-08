// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::driver::AgentState;

/// Raw or rendered output from the terminal backend.
#[derive(Debug, Clone)]
pub enum OutputEvent {
    Raw(Bytes),
    ScreenUpdate { seq: u64 },
}

/// Agent state transition with sequence number for ordering.
#[derive(Debug, Clone)]
pub struct StateChangeEvent {
    pub prev: AgentState,
    pub next: AgentState,
    pub seq: u64,
}

/// Input sent to the child process through the PTY.
#[derive(Debug, Clone)]
pub enum InputEvent {
    Write(Bytes),
    Resize { cols: u16, rows: u16 },
    Signal(i32),
}

/// Lifecycle events for hook integrations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEvent {
    ToolComplete { tool: String },
    AgentStop,
    SessionEnd,
}
