// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Watches a session log file for new JSONL lines appended after a tracked
/// byte offset. Uses `notify` for filesystem events with a polling fallback.
pub struct LogWatcher {
    path: PathBuf,
    offset: u64,
}

impl LogWatcher {
    pub fn new(path: PathBuf) -> Self {
        Self { path, offset: 0 }
    }

    /// Current byte offset into the log file.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Read new complete lines appended since the last read.
    pub fn read_new_lines(&mut self) -> anyhow::Result<Vec<String>> {
        let file = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };

        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(self.offset))?;

        let mut lines = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break;
            }
            self.offset += bytes_read as u64;
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }

        Ok(lines)
    }

    /// Start watching the file, sending batches of new lines to `line_tx`.
    ///
    /// Uses `notify` for filesystem events with a 5-second polling fallback.
    /// Runs until the `shutdown` token is cancelled or the channel closes.
    pub async fn run(mut self, line_tx: mpsc::Sender<Vec<String>>, shutdown: CancellationToken) {
        // Set up notify watcher to detect file changes
        let (wake_tx, mut wake_rx) = mpsc::channel::<()>(1);
        let _watcher = self.setup_notify_watcher(wake_tx);

        let mut poll_interval = tokio::time::interval(Duration::from_secs(5));

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = wake_rx.recv() => {}
                _ = poll_interval.tick() => {}
            }

            match self.read_new_lines() {
                Ok(lines) if !lines.is_empty() => {
                    if line_tx.send(lines).await.is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    /// Set up a `notify` file watcher on the log file's parent directory.
    /// Returns the watcher handle (must be kept alive).
    fn setup_notify_watcher(
        &self,
        wake_tx: mpsc::Sender<()>,
    ) -> Option<notify::RecommendedWatcher> {
        use notify::{RecursiveMode, Watcher};

        let mut watcher = notify::recommended_watcher(move |_: notify::Result<notify::Event>| {
            let _ = wake_tx.try_send(());
        })
        .ok()?;

        // Watch the parent directory so we detect file creation too
        let watch_path = self.path.parent().unwrap_or(self.path.as_ref());
        watcher
            .watch(watch_path, RecursiveMode::NonRecursive)
            .ok()?;

        Some(watcher)
    }
}

#[cfg(test)]
#[path = "log_watch_tests.rs"]
mod tests;
