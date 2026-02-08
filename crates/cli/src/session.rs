// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Session loop: core runtime orchestrating PTY, screen, ring buffer,
//! detection, and transport layers.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::driver::grace::IdleGraceTimer;
use crate::driver::{AgentState, CompositeDetector, DetectedState, Detector, ExitStatus};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::pty::Backend;
use crate::transport::AppState;

/// Parameters for building a new [`Session`].
pub struct SessionConfig {
    pub backend: Box<dyn Backend>,
    pub detectors: Vec<Box<dyn Detector>>,
    pub app_state: Arc<AppState>,
    pub consumer_input_rx: mpsc::Receiver<InputEvent>,
    pub cols: u16,
    pub rows: u16,
    pub idle_grace: Duration,
    pub idle_timeout: Duration,
    pub shutdown: CancellationToken,
}

/// Core session that runs the select-loop multiplexer.
pub struct Session {
    app_state: Arc<AppState>,
    backend_output_rx: mpsc::Receiver<Bytes>,
    backend_input_tx: mpsc::Sender<Bytes>,
    resize_tx: mpsc::Sender<(u16, u16)>,
    consumer_input_rx: mpsc::Receiver<InputEvent>,
    detector_rx: mpsc::Receiver<DetectedState>,
    idle_timeout: Duration,
    shutdown: CancellationToken,
    backend_handle: JoinHandle<anyhow::Result<ExitStatus>>,
}

impl Session {
    /// Build and start a new session.
    ///
    /// Steps:
    /// 1. Sets initial PID on AppState
    /// 2. Sets initial terminal size via `backend.resize()`
    /// 3. Spawns backend.run() on a separate task
    /// 4. Spawns all detectors
    pub fn new(config: SessionConfig) -> Self {
        let SessionConfig {
            mut backend,
            detectors,
            app_state,
            consumer_input_rx,
            cols,
            rows,
            idle_grace,
            idle_timeout,
            shutdown,
        } = config;
        // Set initial PID
        if let Some(pid) = backend.child_pid() {
            app_state
                .child_pid
                .store(pid, std::sync::atomic::Ordering::Relaxed);
        }

        // Set initial terminal size
        let _ = backend.resize(cols, rows);

        // Create backend I/O channels
        let (backend_output_tx, backend_output_rx) = mpsc::channel(256);
        let (backend_input_tx, backend_input_rx) = mpsc::channel(256);
        let (resize_tx, resize_rx) = mpsc::channel(4);

        // Spawn backend task
        let backend_handle = tokio::spawn(async move {
            backend
                .run(backend_output_tx, backend_input_rx, resize_rx)
                .await
        });

        // Build and spawn the composite detector (tier resolution + dedup).
        let (detector_tx, detector_rx) = mpsc::channel(64);
        let grace_timer = IdleGraceTimer::new(idle_grace);
        let composite = CompositeDetector {
            tiers: detectors,
            grace_timer,
        };
        let ring_total = Arc::clone(&app_state.ring_total_written);
        let activity_fn: Arc<dyn Fn() -> u64 + Send + Sync> =
            Arc::new(move || ring_total.load(std::sync::atomic::Ordering::Relaxed));
        let grace_deadline = Arc::clone(&app_state.idle_grace_deadline);
        let detector_shutdown = shutdown.clone();
        tokio::spawn(composite.run(detector_tx, activity_fn, grace_deadline, detector_shutdown));

        Self {
            app_state,
            backend_output_rx,
            backend_input_tx,
            resize_tx,
            consumer_input_rx,
            detector_rx,
            idle_timeout,
            shutdown,
            backend_handle,
        }
    }

    /// Run the session loop until the backend exits or shutdown is triggered.
    pub async fn run(mut self) -> anyhow::Result<ExitStatus> {
        let mut screen_debounce = tokio::time::interval(Duration::from_millis(50));
        let mut state_seq: u64 = 0;
        let mut idle_since: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                // 1. Backend output → feed screen, write ring buffer, broadcast
                data = self.backend_output_rx.recv() => {
                    match data {
                        Some(bytes) => {
                            // Write to ring buffer
                            {
                                let mut ring = self.app_state.ring.write().await;
                                ring.write(&bytes);
                                // Update atomic counter for lock-free activity tracking.
                                self.app_state.ring_total_written.store(
                                    ring.total_written(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                            // Feed screen
                            {
                                let mut screen = self.app_state.screen.write().await;
                                screen.feed(&bytes);
                            }
                            // Broadcast raw output
                            let _ = self.app_state.output_tx.send(OutputEvent::Raw(bytes));
                        }
                        None => {
                            // Backend channel closed — backend exited
                            break;
                        }
                    }
                }

                // 2. Consumer input → forward to backend or handle resize/signal
                event = self.consumer_input_rx.recv() => {
                    match event {
                        Some(InputEvent::Write(data)) => {
                            let len = data.len() as u64;
                            self.app_state.bytes_written.fetch_add(len, std::sync::atomic::Ordering::Relaxed);
                            if self.backend_input_tx.send(data).await.is_err() {
                                debug!("backend input channel closed");
                                break;
                            }
                        }
                        Some(InputEvent::Resize { cols, rows }) => {
                            // Resize screen model
                            {
                                let mut screen = self.app_state.screen.write().await;
                                screen.resize(cols, rows);
                            }
                            // Send resize to backend task which calls TIOCSWINSZ on the
                            // PTY fd (the ioctl also delivers SIGWINCH to the child).
                            let _ = self.resize_tx.send((cols, rows)).await;
                        }
                        Some(InputEvent::Signal(sig)) => {
                            let pid = self.app_state.child_pid.load(std::sync::atomic::Ordering::Relaxed);
                            if pid != 0 {
                                if let Some(signal) = signal_from_i32(sig) {
                                    let _ = kill(Pid::from_raw(pid as i32), signal);
                                }
                            }
                        }
                        None => {
                            // All input senders dropped
                            break;
                        }
                    }
                }

                // 3. Detector state changes → update agent_state, broadcast
                //    CompositeDetector already applies tier resolution + dedup.
                detected = self.detector_rx.recv() => {
                    if let Some(detected) = detected {
                        state_seq += 1;
                        let mut current = self.app_state.agent_state.write().await;
                        let prev = current.clone();
                        *current = detected.state.clone();
                        drop(current);

                        // Store metadata for the HTTP/gRPC API.
                        self.app_state.state_seq.store(state_seq, std::sync::atomic::Ordering::Relaxed);
                        self.app_state.detection_tier.store(detected.tier, std::sync::atomic::Ordering::Relaxed);

                        let _ = self.app_state.state_tx.send(StateChangeEvent {
                            prev,
                            next: detected.state.clone(),
                            seq: state_seq,
                        });

                        // Track idle time for idle_timeout.
                        if matches!(detected.state, AgentState::WaitingForInput)
                            && self.idle_timeout > Duration::ZERO
                        {
                            if idle_since.is_none() {
                                idle_since = Some(tokio::time::Instant::now());
                            }
                        } else {
                            idle_since = None;
                        }
                    }
                }

                // 4. Screen debounce timer → broadcast ScreenUpdate if changed
                _ = screen_debounce.tick() => {
                    let mut screen = self.app_state.screen.write().await;
                    if screen.changed() {
                        let seq = screen.seq();
                        screen.clear_changed();
                        drop(screen);
                        let _ = self.app_state.output_tx.send(OutputEvent::ScreenUpdate { seq });
                    }
                }

                // 5. Idle timeout → trigger shutdown when idle too long
                _ = async {
                    match idle_since {
                        Some(since) => tokio::time::sleep_until(since + self.idle_timeout).await,
                        None => std::future::pending().await,
                    }
                }, if idle_since.is_some() => {
                    debug!("idle timeout reached, triggering shutdown");
                    self.shutdown.cancel();
                    break;
                }

                // 6. Shutdown signal
                _ = self.shutdown.cancelled() => {
                    debug!("shutdown signal received");
                    // Send SIGHUP to child
                    let pid = self.app_state.child_pid.load(std::sync::atomic::Ordering::Relaxed);
                    if pid != 0 {
                        let _ = kill(Pid::from_raw(pid as i32), Signal::SIGHUP);
                    }
                    break;
                }
            }
        }

        // Drop the input sender to signal the backend to stop
        drop(self.backend_input_tx);

        // Wait for backend with timeout
        let status = tokio::select! {
            result = self.backend_handle => {
                match result {
                    Ok(Ok(status)) => status,
                    Ok(Err(e)) => {
                        warn!("backend error: {e}");
                        ExitStatus { code: Some(1), signal: None }
                    }
                    Err(e) => {
                        warn!("backend task panicked: {e}");
                        ExitStatus { code: Some(1), signal: None }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                warn!("backend did not exit within 10s, sending SIGKILL");
                let pid = self.app_state.child_pid.load(std::sync::atomic::Ordering::Relaxed);
                if pid != 0 {
                    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
                }
                ExitStatus { code: Some(137), signal: Some(9) }
            }
        };

        // Store exit status and broadcast exited state
        {
            let mut exit = self.app_state.exit_status.write().await;
            *exit = Some(status);
        }
        let mut current = self.app_state.agent_state.write().await;
        let prev = current.clone();
        *current = AgentState::Exited { status };
        drop(current);
        state_seq += 1;
        let _ = self.app_state.state_tx.send(StateChangeEvent {
            prev,
            next: AgentState::Exited { status },
            seq: state_seq,
        });

        Ok(status)
    }

    /// Get a reference to the shared application state.
    pub fn app_state(&self) -> &Arc<AppState> {
        &self.app_state
    }
}

fn signal_from_i32(sig: i32) -> Option<Signal> {
    match sig {
        1 => Some(Signal::SIGHUP),
        2 => Some(Signal::SIGINT),
        3 => Some(Signal::SIGQUIT),
        9 => Some(Signal::SIGKILL),
        10 => Some(Signal::SIGUSR1),
        12 => Some(Signal::SIGUSR2),
        15 => Some(Signal::SIGTERM),
        18 => Some(Signal::SIGCONT),
        19 => Some(Signal::SIGSTOP),
        20 => Some(Signal::SIGTSTP),
        28 => Some(Signal::SIGWINCH),
        _ => None,
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
