// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/// Capacity for high-throughput data channels (input, output, backend I/O).
pub const DATA_CHANNEL_CAPACITY: usize = 256;
/// Capacity for low-throughput signal channels (state transitions, prompts, detector, start/stop).
pub const SIGNAL_CHANNEL_CAPACITY: usize = 64;
/// Default terminal width when actual size cannot be determined.
pub const DEFAULT_TERM_COLS: u16 = 80;
/// Default terminal height when actual size cannot be determined.
pub const DEFAULT_TERM_ROWS: u16 = 24;

pub mod attach;
pub mod config;
pub mod driver;
pub mod error;
pub mod event;
pub mod pty;
pub mod ring;
pub mod run;
pub mod screen;
pub mod send;
pub mod session;
pub mod start;
pub mod stop;
pub mod test_support;
pub mod transport;
