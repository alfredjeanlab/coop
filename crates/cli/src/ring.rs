// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

/// Fixed-capacity circular byte buffer for raw PTY output.
#[derive(Debug)]
pub struct RingBuffer {
    pub buf: Vec<u8>,
    pub capacity: usize,
    pub write_pos: usize,
    pub total_written: u64,
}
