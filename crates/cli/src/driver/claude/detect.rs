// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::driver::hook_recv::HookReceiver;
use crate::driver::jsonl_stdout::JsonlParser;
use crate::driver::log_watch::LogWatcher;
use crate::driver::{AgentState, Detector};
use crate::event::HookEvent;

use super::state::parse_claude_state;

/// Tier 1 detector: receives push events from Claude's hook system.
///
/// Maps hook events to agent states:
/// - `AgentStop` / `SessionEnd` → `WaitingForInput`
/// - `ToolComplete` → `Working`
pub struct HookDetector {
    pub receiver: HookReceiver,
}

impl Detector for HookDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut receiver = self.receiver;
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = receiver.next_event() => {
                        match event {
                            Some(HookEvent::AgentStop) | Some(HookEvent::SessionEnd) => {
                                let _ = state_tx.send(AgentState::WaitingForInput).await;
                            }
                            Some(HookEvent::ToolComplete { .. }) => {
                                let _ = state_tx.send(AgentState::Working).await;
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        1
    }
}

/// Tier 2 detector: watches Claude's session log file for new JSONL entries.
///
/// Parses each new line with `parse_claude_state` and emits the resulting
/// state to the composite detector.
pub struct LogDetector {
    pub log_path: PathBuf,
}

impl Detector for LogDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let watcher = LogWatcher::new(self.log_path);
            let (line_tx, mut line_rx) = mpsc::channel(32);
            let watch_shutdown = shutdown.clone();

            tokio::spawn(async move {
                watcher.run(line_tx, watch_shutdown).await;
            });

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    batch = line_rx.recv() => {
                        match batch {
                            Some(lines) => {
                                for line in &lines {
                                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                                        if let Some(state) = parse_claude_state(&json) {
                                            let _ = state_tx.send(state).await;
                                        }
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        2
    }
}

/// Tier 3 detector: parses structured JSONL from Claude's stdout stream.
///
/// Used when Claude is invoked with `--print --output-format stream-json`.
/// Receives raw PTY bytes from a channel, feeds them through a JSONL parser,
/// and classifies each parsed entry.
pub struct StdoutDetector {
    pub stdout_rx: mpsc::Receiver<Bytes>,
}

impl Detector for StdoutDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut parser = JsonlParser::new();
            let mut stdout_rx = self.stdout_rx;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    data = stdout_rx.recv() => {
                        match data {
                            Some(bytes) => {
                                for json in parser.feed(&bytes) {
                                    if let Some(state) = parse_claude_state(&json) {
                                        let _ = state_tx.send(state).await;
                                    }
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        3
    }
}

#[cfg(test)]
#[path = "detect_tests.rs"]
mod tests;
