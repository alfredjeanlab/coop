// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/// Channel capacity for high-throughput data (I/O) channels.
pub const IO_CHANNEL_CAPACITY: usize = 256;
/// Channel capacity for low-frequency event/signal channels.
pub const EVENT_CHANNEL_CAPACITY: usize = 64;

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
