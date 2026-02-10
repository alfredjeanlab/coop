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

use crate::config::{Config, GroomLevel};
use crate::driver::{
    classify_error_detail, disruption_option, AgentState, CompositeDetector, DetectedState,
    Detector, ExitStatus, NudgeStep, OptionParser, PromptKind,
};
use crate::event::{InputEvent, OutputEvent, PromptOutcome, TransitionEvent};
use crate::pty::{Backend, BackendInput, Boxed};
use crate::transport::Store;
use crate::{DATA_CHANNEL_CAPACITY, SIGNAL_CHANNEL_CAPACITY};

/// Maximum number of attempts to enrich prompt options from screen content.
const ENRICH_MAX_ATTEMPTS: u32 = 10;
/// Delay between prompt enrichment polling attempts.
const ENRICH_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Runtime objects for building a new [`Session`] (not derivable from [`Config`]).
pub struct SessionConfig {
    pub backend: Box<dyn Backend>,
    pub detectors: Vec<Box<dyn Detector>>,
    pub app_state: Arc<Store>,
    pub consumer_input_rx: mpsc::Receiver<InputEvent>,
    pub shutdown: CancellationToken,
    /// Driver-provided parser for extracting numbered option labels from
    /// rendered screen lines during prompt enrichment.
    pub option_parser: Option<OptionParser>,
}

impl SessionConfig {
    pub fn new(
        app_state: Arc<Store>,
        backend: impl Boxed,
        consumer_input_rx: mpsc::Receiver<InputEvent>,
    ) -> Self {
        Self {
            backend: backend.boxed(),
            app_state,
            detectors: Vec::new(),
            consumer_input_rx,
            shutdown: CancellationToken::new(),
            option_parser: None,
        }
    }

    pub fn with_detectors(mut self, detectors: Vec<Box<dyn Detector>>) -> Self {
        self.detectors = detectors;
        self
    }

    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    pub fn with_option_parser(mut self, parser: OptionParser) -> Self {
        self.option_parser = Some(parser);
        self
    }
}

/// Core session that runs the select-loop multiplexer.
pub struct Session {
    app_state: Arc<Store>,
    backend_output_rx: mpsc::Receiver<Bytes>,
    backend_input_tx: mpsc::Sender<BackendInput>,
    resize_tx: mpsc::Sender<(u16, u16)>,
    consumer_input_rx: mpsc::Receiver<InputEvent>,
    detector_rx: mpsc::Receiver<DetectedState>,
    shutdown: CancellationToken,
    backend_handle: JoinHandle<anyhow::Result<ExitStatus>>,
    option_parser: Option<OptionParser>,
}

impl Session {
    /// Build and start a new session.
    ///
    /// Steps:
    /// 1. Sets initial PID on AppState
    /// 2. Sets initial terminal size via `backend.resize()`
    /// 3. Spawns backend.run() on a separate task
    /// 4. Spawns all detectors
    pub fn new(config: &Config, session: SessionConfig) -> Self {
        let SessionConfig {
            mut backend,
            detectors,
            app_state,
            consumer_input_rx,
            shutdown,
            option_parser,
        } = session;

        // Set initial PID (Release so signal-delivery loads with Acquire see it)
        if let Some(pid) = backend.child_pid() {
            app_state.terminal.child_pid.store(pid, std::sync::atomic::Ordering::Release);
        }

        // Set initial terminal size
        let _ = backend.resize(config.cols, config.rows);

        // Create backend I/O channels
        let (backend_output_tx, backend_output_rx) = mpsc::channel(DATA_CHANNEL_CAPACITY);
        let (backend_input_tx, backend_input_rx) =
            mpsc::channel::<BackendInput>(DATA_CHANNEL_CAPACITY);
        let (resize_tx, resize_rx) = mpsc::channel(4);

        // Spawn backend task
        let backend_handle = tokio::spawn(async move {
            backend.run(backend_output_tx, backend_input_rx, resize_rx).await
        });

        // Build and spawn the composite detector (tier resolution + dedup).
        let (detector_tx, detector_rx) = mpsc::channel(SIGNAL_CHANNEL_CAPACITY);
        let composite = CompositeDetector { tiers: detectors };
        let detector_shutdown = shutdown.clone();
        tokio::spawn(composite.run(detector_tx, detector_shutdown));

        Self {
            app_state,
            backend_output_rx,
            backend_input_tx,
            resize_tx,
            consumer_input_rx,
            detector_rx,
            shutdown,
            backend_handle,
            option_parser,
        }
    }

    /// Run the session loop until the backend exits or shutdown is triggered.
    pub async fn run(mut self, config: &Config) -> anyhow::Result<ExitStatus> {
        let idle_timeout = config.idle_timeout();
        let shutdown_timeout = config.shutdown_timeout();
        let graceful_timeout = config.drain_timeout();
        let mut screen_debounce = tokio::time::interval(config.screen_debounce());
        let mut state_seq: u64 = 0;
        let mut idle_since: Option<tokio::time::Instant> = None;
        let mut last_state = AgentState::Starting;
        let mut drain_deadline: Option<tokio::time::Instant> = None;
        let mut next_escape_at: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                // 1. Backend output → feed screen, write ring buffer, broadcast
                data = self.backend_output_rx.recv() => {
                    match data {
                        Some(bytes) => {
                            // Write to ring buffer
                            {
                                let mut ring = self.app_state.terminal.ring.write().await;
                                ring.write(&bytes);
                                // Update atomic counter for lock-free activity tracking.
                                self.app_state.terminal.ring_total_written.store(
                                    ring.total_written(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                            }
                            // Feed screen
                            {
                                let mut screen = self.app_state.terminal.screen.write().await;
                                screen.feed(&bytes);
                            }
                            // Broadcast raw output
                            let _ = self.app_state.channels.output_tx.send(OutputEvent::Raw(bytes));
                        }
                        None => {
                            // Backend channel closed — backend exited
                            break;
                        }
                    }
                }

                // 2. Consumer input → forward to backend or handle resize/signal
                event = self.consumer_input_rx.recv() => {
                    // Notify the enter-retry monitor that input activity occurred.
                    // WaitForDrain is excluded because it's an internal sync
                    // marker, not user/transport-initiated input.
                    if !matches!(event, Some(InputEvent::WaitForDrain(_)) | None) {
                        self.app_state.input_activity.notify_waiters();
                    }
                    match event {
                        Some(InputEvent::Write(data)) => {
                            let len = data.len() as u64;
                            self.app_state.lifecycle.bytes_written.fetch_add(len, std::sync::atomic::Ordering::Relaxed);
                            if self.backend_input_tx.send(BackendInput::Write(data)).await.is_err() {
                                debug!("backend input channel closed");
                                break;
                            }
                        }
                        Some(InputEvent::WaitForDrain(tx)) => {
                            if self.backend_input_tx.send(BackendInput::Drain(tx)).await.is_err() {
                                debug!("backend input channel closed");
                                break;
                            }
                        }
                        Some(InputEvent::Resize { cols, rows }) => {
                            // Resize screen model
                            {
                                let mut screen = self.app_state.terminal.screen.write().await;
                                screen.resize(cols, rows);
                            }
                            // Send resize to backend task which calls TIOCSWINSZ on the
                            // PTY fd (the ioctl also delivers SIGWINCH to the child).
                            let _ = self.resize_tx.try_send((cols, rows));
                        }
                        Some(InputEvent::Signal(sig)) => {
                            let pid = self.app_state.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
                            if pid != 0 {
                                let _ = kill(Pid::from_raw(pid as i32), sig.to_nix());
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
                        let mut current = self.app_state.driver.agent_state.write().await;
                        let prev = current.clone();
                        *current = detected.state.clone();
                        drop(current);
                        last_state = detected.state.clone();

                        // Mark ready on first transition away from Starting.
                        if matches!(prev, AgentState::Starting)
                            && !matches!(detected.state, AgentState::Starting)
                        {
                            self.app_state
                                .ready
                                .store(true, std::sync::atomic::Ordering::Release);
                        }

                        // Store error detail + category when entering Error state.
                        if let AgentState::Error { ref detail } = detected.state {
                            let category = classify_error_detail(detail);
                            *self.app_state.driver.error.write().await = Some(
                                crate::transport::state::ErrorInfo {
                                    detail: detail.clone(),
                                    category,
                                },
                            );
                        } else {
                            *self.app_state.driver.error.write().await = None;
                        }

                        // Store metadata for the HTTP/gRPC API.
                        self.app_state.driver.state_seq.store(state_seq, std::sync::atomic::Ordering::Release);
                        *self.app_state.driver.detection.write().await = crate::transport::state::DetectionInfo {
                            tier: detected.tier,
                            cause: detected.cause.clone(),
                        };

                        let last_message = self.app_state.driver.last_message.read().await.clone();
                        let _ = self.app_state.channels.state_tx.send(TransitionEvent {
                            prev,
                            next: detected.state.clone(),
                            seq: state_seq,
                            cause: detected.cause,
                            last_message,
                        });

                        // Spawn deferred option enrichment for Permission/Plan prompts.
                        // Hook events fire before the screen renders numbered options,
                        // so we wait briefly for the PTY output to catch up, then parse
                        // options from the screen and re-broadcast the enriched state.
                        if let AgentState::Prompt { ref prompt } = detected.state {
                            if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
                                if let Some(ref parser) = self.option_parser {
                                    let app = Arc::clone(&self.app_state);
                                    let seq = state_seq;
                                    let parser = Arc::clone(parser);
                                    tokio::spawn(enrich_prompt_options(app, seq, parser));
                                }
                            }
                        }

                        // Auto-dismiss disruption prompts in groom=auto mode.
                        // The prompt state is broadcast BEFORE auto-dismiss so API
                        // clients see the action transparently.
                        if let AgentState::Prompt { ref prompt } = detected.state {
                            if self.app_state.config.groom == GroomLevel::Auto {
                                if let Some(option) = disruption_option(prompt) {
                                    if prompt.subtype.as_deref() == Some("settings_error") {
                                        warn!("auto-dismissing settings error dialog (option {option})");
                                    }
                                    if let Some(ref encoder) = self.app_state.config.respond_encoder {
                                        // "Press Enter to continue" screens have no numbered
                                        // options — just send Enter instead of "N" + delay + Enter.
                                        let steps = if prompt.options.is_empty() {
                                            vec![NudgeStep { bytes: b"\r".to_vec(), delay_after: None }]
                                        } else if prompt.kind == PromptKind::Permission {
                                            encoder.encode_permission(option)
                                        } else {
                                            encoder.encode_setup(option)
                                        };
                                        let tx = self.app_state.channels.input_tx.clone();
                                        let gate = Arc::clone(&self.app_state.input_gate);
                                        let expected_seq = state_seq;
                                        let driver = Arc::clone(&self.app_state.driver);
                                        let terminal = Arc::clone(&self.app_state.terminal);
                                        let prompt_tx = self.app_state.channels.prompt_tx.clone();
                                        let prompt_type = prompt.kind.as_str().to_owned();
                                        let prompt_subtype = prompt.subtype.clone();
                                        let groom_option = if prompt.options.is_empty() { None } else { Some(option) };
                                        tokio::spawn(async move {
                                            tokio::time::sleep(Duration::from_millis(500)).await;
                                            // Guard: skip if state changed (someone already responded).
                                            let current = driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
                                            if current != expected_seq {
                                                return;
                                            }
                                            let _delivery = gate.acquire().await;
                                            // Re-check after gate acquisition.
                                            let current = driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
                                            if current != expected_seq {
                                                return;
                                            }
                                            // Deliver steps one at a time. Between steps where a
                                            // delay is present, check whether the screen already
                                            // transitioned (e.g. number key auto-confirmed in a
                                            // picker dialog). Skip remaining steps to prevent
                                            // keystrokes bleeding into the next screen.
                                            let step_count = steps.len();
                                            for (i, step) in steps.into_iter().enumerate() {
                                                // Snapshot screen before steps with a delay when
                                                // more steps follow — we compare after the delay.
                                                let pre_lines = if step.delay_after.is_some() && i + 1 < step_count {
                                                    terminal.screen.try_read().ok().map(|s| s.snapshot().lines)
                                                } else {
                                                    None
                                                };

                                                if tx.send(InputEvent::Write(Bytes::from(step.bytes))).await.is_err() {
                                                    break;
                                                }
                                                if let Some(delay) = step.delay_after {
                                                    let (drain_tx, drain_rx) = tokio::sync::oneshot::channel();
                                                    let _ = tx.send(InputEvent::WaitForDrain(drain_tx)).await;
                                                    let _ = drain_rx.await;
                                                    tokio::time::sleep(delay).await;
                                                }

                                                // If the screen content changed after the delay,
                                                // the keystroke already took full effect — stop
                                                // before sending the next step (e.g. Enter).
                                                if let Some(pre) = pre_lines {
                                                    if let Ok(screen) = terminal.screen.try_read() {
                                                        if screen.snapshot().lines != pre {
                                                            break;
                                                        }
                                                    }
                                                }
                                            }
                                            let _ = prompt_tx.send(PromptOutcome {
                                                source: "groom".to_owned(),
                                                r#type: prompt_type,
                                                subtype: prompt_subtype,
                                                option: groom_option,
                                            });
                                        });
                                    }
                                }
                            }
                        }

                        // Track idle time for idle_timeout.
                        if matches!(detected.state, AgentState::Idle)
                            && idle_timeout > Duration::ZERO
                        {
                            if idle_since.is_none() {
                                idle_since = Some(tokio::time::Instant::now());
                            }
                        } else {
                            idle_since = None;
                        }

                        // Drain check: agent reached idle during drain → kill now.
                        if drain_deadline.is_some() && matches!(detected.state, AgentState::Idle) {
                            debug!("drain: agent reached idle, sending SIGHUP");
                            let pid = self.app_state.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
                            if pid != 0 {
                                let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGHUP);
                            }
                            break;
                        }
                    }
                }

                // 4. Screen debounce timer → broadcast ScreenUpdate if changed.
                _ = screen_debounce.tick() => {
                    let mut screen = self.app_state.terminal.screen.write().await;
                    let changed = screen.changed();
                    if changed {
                        let seq = screen.seq();
                        screen.clear_changed();
                        drop(screen);

                        let _ = self.app_state.channels.output_tx.send(OutputEvent::ScreenUpdate { seq });
                    }
                }

                // 5. Idle timeout → trigger shutdown when idle too long
                _ = async {
                    match idle_since {
                        Some(since) => tokio::time::sleep_until(since + idle_timeout).await,
                        None => std::future::pending().await,
                    }
                }, if idle_since.is_some() => {
                    debug!("idle timeout reached, triggering shutdown");
                    self.shutdown.cancel();
                    break;
                }

                // 6. Drain escape ticker — periodically send Escape during drain
                _ = async {
                    match next_escape_at {
                        Some(at) => tokio::time::sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                }, if next_escape_at.is_some() => {
                    debug!("drain: sending Escape");
                    let esc = Bytes::from_static(b"\x1b");
                    self.app_state.lifecycle.bytes_written.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    self.app_state.input_activity.notify_waiters();
                    let _ = self.backend_input_tx.send(BackendInput::Write(esc)).await;
                    next_escape_at = Some(tokio::time::Instant::now() + Duration::from_secs(2));
                }

                // 7. Drain deadline — force-kill after graceful timeout
                _ = async {
                    match drain_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                }, if drain_deadline.is_some() => {
                    debug!("drain: deadline reached, force-killing");
                    let pid = self.app_state.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
                    if pid != 0 {
                        let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGHUP);
                    }
                    break;
                }

                // 8. Shutdown signal (disabled once drain mode is active)
                _ = self.shutdown.cancelled(), if drain_deadline.is_none() => {
                    debug!("shutdown signal received");
                    let pid = self.app_state.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
                    if graceful_timeout > Duration::ZERO
                        && !matches!(last_state, AgentState::Idle)
                    {
                        debug!("entering graceful drain mode (timeout={graceful_timeout:?})");
                        drain_deadline = Some(tokio::time::Instant::now() + graceful_timeout);
                        next_escape_at = Some(tokio::time::Instant::now());
                    } else {
                        // Immediate kill (graceful disabled or agent already idle)
                        if pid != 0 {
                            let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGHUP);
                        }
                        break;
                    }
                }
            }
        }

        // Drain any pending output so final bytes are captured.
        while let Ok(bytes) = self.backend_output_rx.try_recv() {
            {
                let mut ring = self.app_state.terminal.ring.write().await;
                ring.write(&bytes);
                self.app_state
                    .terminal
                    .ring_total_written
                    .store(ring.total_written(), std::sync::atomic::Ordering::Relaxed);
            }
            {
                let mut screen = self.app_state.terminal.screen.write().await;
                screen.feed(&bytes);
            }
            let _ = self.app_state.channels.output_tx.send(OutputEvent::Raw(bytes));
        }

        // Drop the input sender to signal the backend to stop
        drop(self.backend_input_tx);

        // Wait for backend with timeout
        let status = tokio::select! {
            result = &mut self.backend_handle => {
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
            _ = tokio::time::sleep(shutdown_timeout) => {
                warn!("backend did not exit within {:?}, sending SIGKILL", shutdown_timeout);
                let pid = self.app_state.terminal.child_pid.load(std::sync::atomic::Ordering::Acquire);
                if pid != 0 {
                    // Signal the process group to also kill grandchildren.
                    let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
                }
                // Abort the backend task to release the PTY master FD.
                self.backend_handle.abort();
                ExitStatus { code: Some(137), signal: Some(9) }
            }
        };

        // Store exit status and broadcast exited state.
        // ORDERING: exit_status must be written before agent_state so that
        // any reader who observes AgentState::Exited is guaranteed to find
        // exit_status populated.
        {
            let mut exit = self.app_state.terminal.exit_status.write().await;
            *exit = Some(status);
        }
        let mut current = self.app_state.driver.agent_state.write().await;
        let prev = current.clone();
        *current = AgentState::Exited { status };
        drop(current);
        state_seq += 1;
        let last_message = self.app_state.driver.last_message.read().await.clone();
        let _ = self.app_state.channels.state_tx.send(TransitionEvent {
            prev,
            next: AgentState::Exited { status },
            seq: state_seq,
            cause: String::new(),
            last_message,
        });

        Ok(status)
    }

    /// Get a reference to the shared application state.
    pub fn app_state(&self) -> &Arc<Store> {
        &self.app_state
    }
}

/// Wait for the screen to render prompt options, parse them, and re-broadcast the enriched prompt context.
///
/// This runs as a detached task because hook events fire before the PTY output
/// containing numbered options reaches the screen buffer.
/// Retries up to `ENRICH_MAX_ATTEMPTS` times, then falls back to universal Accept/Cancel options that encode to Enter/Esc.
async fn enrich_prompt_options(app: Arc<Store>, expected_seq: u64, parser: OptionParser) {
    let mut last_snap_lines = 0usize;

    for _ in 0..ENRICH_MAX_ATTEMPTS {
        tokio::time::sleep(ENRICH_POLL_INTERVAL).await;

        // Bail if the state has changed since we spawned.
        let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
        if current_seq != expected_seq {
            return;
        }

        let screen = app.terminal.screen.read().await;
        let snap = screen.snapshot();
        drop(screen);
        last_snap_lines = snap.lines.len();

        let options = parser(&snap.lines);
        if !options.is_empty() {
            let mut agent = app.driver.agent_state.write().await;

            // Re-check seq under the write lock.
            let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
            if current_seq != expected_seq {
                return;
            }

            if let AgentState::Prompt { ref mut prompt } = *agent {
                if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
                    prompt.options = options;
                    prompt.ready = true;

                    let next = agent.clone();
                    drop(agent);

                    let last_message = app.driver.last_message.read().await.clone();
                    let _ = app.channels.state_tx.send(TransitionEvent {
                        prev: next.clone(),
                        next,
                        seq: expected_seq,
                        cause: "ready".to_owned(),
                        last_message,
                    });
                }
            }
            return;
        }
    }

    // All retries exhausted — set fallback options so API consumers have
    // something to present (Enter for accept, Esc for cancel).
    debug!(last_snap_lines, "prompt option enrichment: setting fallback options");

    let mut agent = app.driver.agent_state.write().await;
    let current_seq = app.driver.state_seq.load(std::sync::atomic::Ordering::Acquire);
    if current_seq != expected_seq {
        return;
    }

    if let AgentState::Prompt { ref mut prompt } = *agent {
        if matches!(prompt.kind, PromptKind::Permission | PromptKind::Plan) {
            prompt.options = vec!["Accept".to_string(), "Cancel".to_string()];
            prompt.options_fallback = true;
            prompt.ready = true;

            let next = agent.clone();
            drop(agent);

            let last_message = app.driver.last_message.read().await.clone();
            let _ = app.channels.state_tx.send(TransitionEvent {
                prev: next.clone(),
                next,
                seq: expected_seq,
                cause: "ready".to_owned(),
                last_message,
            });
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
