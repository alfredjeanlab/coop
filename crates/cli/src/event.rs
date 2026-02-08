// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use bytes::Bytes;
use nix::sys::signal::Signal;
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
    Signal(PtySignal),
}

/// Named signals that can be delivered to the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtySignal {
    Hup,
    Int,
    Quit,
    Kill,
    Usr1,
    Usr2,
    Term,
    Cont,
    Stop,
    Tstp,
    Winch,
}

impl PtySignal {
    /// Parse a signal name (e.g. "SIGINT", "INT", "2") into a `PtySignal`.
    pub fn from_name(name: &str) -> Option<Self> {
        let upper = name.to_uppercase();
        let bare: &str = match upper.strip_prefix("SIG") {
            Some(s) => s,
            None => &upper,
        };

        match bare {
            "HUP" | "1" => Some(Self::Hup),
            "INT" | "2" => Some(Self::Int),
            "QUIT" | "3" => Some(Self::Quit),
            "KILL" | "9" => Some(Self::Kill),
            "USR1" | "10" => Some(Self::Usr1),
            "USR2" | "12" => Some(Self::Usr2),
            "TERM" | "15" => Some(Self::Term),
            "CONT" | "18" => Some(Self::Cont),
            "STOP" | "19" => Some(Self::Stop),
            "TSTP" | "20" => Some(Self::Tstp),
            "WINCH" | "28" => Some(Self::Winch),
            _ => None,
        }
    }

    /// Convert to the corresponding `nix` signal for delivery.
    pub fn to_nix(self) -> Signal {
        match self {
            Self::Hup => Signal::SIGHUP,
            Self::Int => Signal::SIGINT,
            Self::Quit => Signal::SIGQUIT,
            Self::Kill => Signal::SIGKILL,
            Self::Usr1 => Signal::SIGUSR1,
            Self::Usr2 => Signal::SIGUSR2,
            Self::Term => Signal::SIGTERM,
            Self::Cont => Signal::SIGCONT,
            Self::Stop => Signal::SIGSTOP,
            Self::Tstp => Signal::SIGTSTP,
            Self::Winch => Signal::SIGWINCH,
        }
    }
}

/// Lifecycle events for hook integrations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEvent {
    ToolComplete { tool: String },
    AgentStop,
    SessionEnd,
}
