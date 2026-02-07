// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

/// Incremental line-buffered parser for newline-delimited JSON on stdout.
///
/// Feeds raw bytes from the PTY output stream and extracts complete JSON
/// values separated by newlines.
#[derive(Debug, Default)]
pub struct JsonlParser {
    pub line_buf: Vec<u8>,
}

impl JsonlParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed raw bytes and return any complete JSON values found.
    ///
    /// Buffers partial lines internally. Each newline-terminated segment
    /// is attempted as JSON; non-JSON lines are silently dropped.
    pub fn feed(&mut self, data: &[u8]) -> Vec<serde_json::Value> {
        let mut entries = Vec::new();
        for &byte in data {
            if byte == b'\n' {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&self.line_buf) {
                    entries.push(json);
                }
                self.line_buf.clear();
            } else {
                self.line_buf.push(byte);
            }
        }
        entries
    }
}

#[cfg(test)]
#[path = "jsonl_stdout_tests.rs"]
mod tests;
