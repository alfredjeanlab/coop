// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal;
use nix::unistd::Pid;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{AgentState, Detector, ExitStatus};

/// Checks whether a process with the given PID is alive.
pub fn is_process_alive(pid: u32) -> bool {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false;
    };
    signal::kill(Pid::from_raw(pid_i32), None).is_ok()
}

/// Tier 4 detector that monitors child process liveness and ring buffer
/// activity. Emits `AgentState::Exited` when the child process dies.
pub struct ProcessMonitor {
    child_pid: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
    ring_total_written: Arc<dyn Fn() -> u64 + Send + Sync>,
    poll_interval: Duration,
}

impl ProcessMonitor {
    pub fn new(
        child_pid: Arc<dyn Fn() -> Option<u32> + Send + Sync>,
        ring_total_written: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            child_pid,
            ring_total_written,
            poll_interval: Duration::from_secs(5),
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl Detector for ProcessMonitor {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(self.poll_interval);
            let mut last_written = (self.ring_total_written)();

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }

                let current_written = (self.ring_total_written)();
                let _active = current_written > last_written;
                last_written = current_written;

                if let Some(pid) = (self.child_pid)() {
                    if !is_process_alive(pid) {
                        let _ = state_tx
                            .send(AgentState::Exited {
                                status: ExitStatus {
                                    code: None,
                                    signal: None,
                                },
                            })
                            .await;
                        break;
                    }
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        4
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
