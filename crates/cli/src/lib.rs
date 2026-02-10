// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

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

/// Channel capacity for high-throughput I/O (input, output, backend).
pub const CHANNEL_CAP_IO: usize = 256;

/// Channel capacity for low-throughput signal/event channels (state, prompt, detector).
pub const CHANNEL_CAP_SIGNAL: usize = 64;

/// Fallback terminal width (standard VT100 default).
pub const FALLBACK_TERM_COLS: u16 = 80;

/// Fallback terminal height (standard VT100 default).
pub const FALLBACK_TERM_ROWS: u16 = 24;

/// Default ring buffer size (1 MiB).
pub const DEFAULT_RING_SIZE: usize = 1_048_576;
