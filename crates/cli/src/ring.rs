// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/// Fixed-capacity circular byte buffer for raw PTY output.
///
/// Tracks the total number of bytes ever written so consumers can request
/// replay from a global byte offset. When the buffer wraps, older data is
/// silently discarded.
#[derive(Debug)]
pub struct RingBuffer {
    buf: Vec<u8>,
    capacity: usize,
    write_pos: usize,
    total_written: u64,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0u8; capacity],
            capacity,
            write_pos: 0,
            total_written: 0,
        }
    }

    /// Append data into the circular buffer.
    pub fn write(&mut self, data: &[u8]) {
        for chunk in data.chunks(self.capacity) {
            let start = self.write_pos;
            let end = start + chunk.len();

            if end <= self.capacity {
                self.buf[start..end].copy_from_slice(chunk);
            } else {
                let first = self.capacity - start;
                self.buf[start..self.capacity].copy_from_slice(&chunk[..first]);
                self.buf[..chunk.len() - first].copy_from_slice(&chunk[first..]);
            }

            self.write_pos = end % self.capacity;
            self.total_written += chunk.len() as u64;
        }
    }

    /// Read bytes starting from the given global byte offset.
    ///
    /// Returns `None` if the requested offset has already been overwritten
    /// (too old) or is beyond the current write position (too new).
    /// Otherwise returns one or two slices covering the requested range.
    pub fn read_from(&self, offset: u64) -> Option<(&[u8], &[u8])> {
        if offset > self.total_written {
            return None;
        }

        let oldest = self.total_written.saturating_sub(self.capacity as u64);
        if offset < oldest {
            return None;
        }

        let available = (self.total_written - offset) as usize;
        if available == 0 {
            return Some((&[], &[]));
        }

        // Start position in the circular buffer for the requested offset
        let start = if self.write_pos >= available {
            self.write_pos - available
        } else {
            self.capacity - (available - self.write_pos)
        };

        if start + available <= self.capacity {
            Some((&self.buf[start..start + available], &[]))
        } else {
            let first = self.capacity - start;
            Some((
                &self.buf[start..self.capacity],
                &self.buf[..available - first],
            ))
        }
    }

    /// How many bytes are readable starting from the given offset.
    pub fn available_from(&self, offset: u64) -> u64 {
        if offset > self.total_written {
            return 0;
        }
        let oldest = self.total_written.saturating_sub(self.capacity as u64);
        if offset < oldest {
            return 0;
        }
        self.total_written - offset
    }

    /// Total bytes ever written through this buffer.
    pub fn total_written(&self) -> u64 {
        self.total_written
    }
}

#[cfg(test)]
#[path = "ring_tests.rs"]
mod tests;
