// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

//! WebSocket message types for the coop real-time protocol.
//!
//! Messages use internally-tagged JSON enums (`{"type": "input", ...}`) as
//! specified in DESIGN.md. Two top-level enums cover server-to-client and
//! client-to-server directions.

use serde::{Deserialize, Serialize};

use crate::driver::PromptContext;
use crate::screen::CursorPosition;

// ---------------------------------------------------------------------------
// Server -> Client
// ---------------------------------------------------------------------------

/// Messages sent from the server to connected WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Raw terminal output (base64-encoded).
    Output { data: String, offset: u64 },

    /// Point-in-time screen snapshot.
    Screen {
        lines: Vec<String>,
        cols: u16,
        rows: u16,
        alt_screen: bool,
        cursor: Option<CursorPosition>,
        seq: u64,
    },

    /// Agent state transition.
    StateChange {
        prev: String,
        next: String,
        seq: u64,
        prompt: Option<PromptContext>,
    },

    /// Child process exited.
    Exit {
        code: Option<i32>,
        signal: Option<i32>,
    },

    /// Error notification.
    Error { code: String, message: String },

    /// Terminal was resized.
    Resize { cols: u16, rows: u16 },

    /// Response to a client ping.
    Pong {},
}

// ---------------------------------------------------------------------------
// Client -> Server
// ---------------------------------------------------------------------------

/// Messages sent from WebSocket clients to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Send text input (UTF-8).
    Input { text: String },

    /// Send raw input (base64-encoded bytes).
    InputRaw { data: String },

    /// Send named key sequences.
    Keys { keys: Vec<String> },

    /// Resize the terminal.
    Resize { cols: u16, rows: u16 },

    /// Request a screen snapshot.
    ScreenRequest {},

    /// Request current agent state.
    StateRequest {},

    /// Nudge the agent with a message.
    Nudge { message: String },

    /// Respond to an agent prompt.
    Respond {
        accept: Option<bool>,
        option: Option<i32>,
        text: Option<String>,
    },

    /// Replay output from a given offset.
    Replay { offset: u64 },

    /// Acquire or release the input lock.
    Lock { action: LockAction },

    /// Authenticate with a token.
    Auth { token: String },

    /// Ping the server.
    Ping {},
}

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Action for the input lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LockAction {
    Acquire,
    Release,
}

/// WebSocket subscription mode (query parameter on upgrade).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionMode {
    Raw,
    Screen,
    State,
    #[default]
    All,
}
