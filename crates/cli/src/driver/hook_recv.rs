// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::path::{Path, PathBuf};

use nix::sys::stat::Mode;
use serde::Deserialize;
use tokio::io::AsyncBufReadExt;

use crate::event::HookEvent;

/// Receives structured hook events from a named pipe (FIFO).
///
/// Agent hooks write JSON lines to the pipe. The receiver reads and parses
/// them into [`HookEvent`] values.
pub struct HookReceiver {
    pipe_path: PathBuf,
    reader: Option<tokio::io::BufReader<tokio::fs::File>>,
}

/// Intermediate type for parsing hook JSON from the pipe.
#[derive(Deserialize)]
struct RawHookEvent {
    event: String,
    tool: Option<String>,
}

impl HookReceiver {
    /// Create a new hook receiver, creating the named pipe at `pipe_path`.
    pub fn new(pipe_path: &Path) -> anyhow::Result<Self> {
        nix::unistd::mkfifo(pipe_path, Mode::from_bits_truncate(0o600))?;
        Ok(Self {
            pipe_path: pipe_path.to_path_buf(),
            reader: None,
        })
    }

    /// Path to the named pipe.
    pub fn pipe_path(&self) -> &Path {
        &self.pipe_path
    }

    /// Read the next hook event from the pipe.
    ///
    /// Returns `None` on EOF or unrecoverable error. Skips malformed lines.
    pub async fn next_event(&mut self) -> Option<HookEvent> {
        let reader = match self.ensure_reader() {
            Ok(r) => r,
            Err(_) => return None,
        };

        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => return None,
                Ok(_) => {
                    if let Some(event) = parse_hook_line(line.trim()) {
                        return Some(event);
                    }
                    // Malformed line — skip and try next
                }
                Err(_) => return None,
            }
        }
    }

    /// Ensure the pipe is opened for reading.
    ///
    /// Opens with `O_RDWR | O_NONBLOCK` so the read end stays open even
    /// when no writers are connected (prevents spurious EOF).
    fn ensure_reader(&mut self) -> anyhow::Result<&mut tokio::io::BufReader<tokio::fs::File>> {
        if self.reader.is_none() {
            // O_RDWR prevents blocking on open (both ends present) and
            // avoids EOF when the last writer closes. tokio::fs::File
            // handles blocking reads via spawn_blocking.
            let std_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.pipe_path)?;
            let tokio_file = tokio::fs::File::from_std(std_file);
            self.reader = Some(tokio::io::BufReader::new(tokio_file));
        }
        self.reader
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("reader not initialized"))
    }
}

impl Drop for HookReceiver {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.pipe_path);
    }
}

/// Parse a raw JSON line from the hook pipe into a [`HookEvent`].
fn parse_hook_line(line: &str) -> Option<HookEvent> {
    let raw: RawHookEvent = serde_json::from_str(line).ok()?;
    match raw.event.as_str() {
        "post_tool_use" => Some(HookEvent::ToolComplete {
            tool: raw.tool.unwrap_or_default(),
        }),
        "stop" => Some(HookEvent::AgentStop),
        "session_end" => Some(HookEvent::SessionEnd),
        _ => None,
    }
}

#[cfg(test)]
#[path = "hook_recv_tests.rs"]
mod tests;
