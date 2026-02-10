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

// ---------------------------------------------------------------------------
// Named constants (extracted from inline magic numbers)
// ---------------------------------------------------------------------------

/// Channel capacity for high-throughput data I/O (input, backend, output broadcast).
pub const DATA_CHANNEL_CAPACITY: usize = 256;

/// Channel capacity for event/signal channels (state transitions, prompts, detectors).
pub const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Default terminal width (columns) used as fallback when the real size is unavailable.
pub const DEFAULT_TERM_COLS: u16 = 80;

/// Default terminal height (rows) used as fallback when the real size is unavailable.
pub const DEFAULT_TERM_ROWS: u16 = 24;

/// Default ring buffer size in bytes (1 MiB).
pub const DEFAULT_RING_SIZE: usize = 1_048_576;

/// Smaller ring buffer size for tests (64 KiB).
pub const TEST_RING_SIZE: usize = 65_536;
