// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/// Standard terminal width, used as test default and runtime fallback.
pub const DEFAULT_TERMINAL_COLS: u16 = 80;
/// Standard terminal height, used as test default and runtime fallback.
pub const DEFAULT_TERMINAL_ROWS: u16 = 24;

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
