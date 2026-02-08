// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared test infrastructure: builders, mocks, and assertion helpers.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, ExitStatus, NudgeEncoder, RespondEncoder};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::pty::Backend;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::transport::state::{AppState, WriteLock};

// ---------------------------------------------------------------------------
// TestAppStateBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing `AppState` in tests with sensible defaults.
pub struct TestAppStateBuilder {
    ring_size: usize,
    child_pid: u32,
    auth_token: Option<String>,
    agent_state: AgentState,
    nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    respond_encoder: Option<Arc<dyn RespondEncoder>>,
}

impl Default for TestAppStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestAppStateBuilder {
    pub fn new() -> Self {
        Self {
            ring_size: 4096,
            child_pid: 0,
            auth_token: None,
            agent_state: AgentState::Starting,
            nudge_encoder: None,
            respond_encoder: None,
        }
    }

    pub fn ring_size(mut self, n: usize) -> Self {
        self.ring_size = n;
        self
    }

    pub fn child_pid(mut self, pid: u32) -> Self {
        self.child_pid = pid;
        self
    }

    pub fn auth_token(mut self, t: impl Into<String>) -> Self {
        self.auth_token = Some(t.into());
        self
    }

    pub fn agent_state(mut self, s: AgentState) -> Self {
        self.agent_state = s;
        self
    }

    pub fn nudge_encoder(mut self, e: Arc<dyn NudgeEncoder>) -> Self {
        self.nudge_encoder = Some(e);
        self
    }

    pub fn respond_encoder(mut self, e: Arc<dyn RespondEncoder>) -> Self {
        self.respond_encoder = Some(e);
        self
    }

    /// Build state and return the `input_rx` receiver alongside it.
    pub fn build(self) -> (Arc<AppState>, mpsc::Receiver<InputEvent>) {
        let (input_tx, input_rx) = mpsc::channel(16);
        let state = self.build_with_sender(input_tx);
        (state, input_rx)
    }

    /// Build state using an externally-created `input_tx`.
    pub fn build_with_sender(self, input_tx: mpsc::Sender<InputEvent>) -> Arc<AppState> {
        let (output_tx, _) = broadcast::channel::<OutputEvent>(256);
        let (state_tx, _) = broadcast::channel::<StateChangeEvent>(64);

        Arc::new(AppState {
            started_at: Instant::now(),
            agent_type: "unknown".to_owned(),
            screen: Arc::new(RwLock::new(Screen::new(80, 24))),
            ring: Arc::new(RwLock::new(RingBuffer::new(self.ring_size))),
            agent_state: Arc::new(RwLock::new(self.agent_state)),
            input_tx,
            output_tx,
            state_tx,
            child_pid: Arc::new(AtomicU32::new(self.child_pid)),
            exit_status: Arc::new(RwLock::new(None)),
            write_lock: Arc::new(WriteLock::new()),
            ws_client_count: Arc::new(AtomicI32::new(0)),
            bytes_written: AtomicU64::new(0),
            auth_token: self.auth_token,
            nudge_encoder: self.nudge_encoder,
            respond_encoder: self.respond_encoder,
            shutdown: CancellationToken::new(),
            state_seq: AtomicU64::new(0),
            detection_tier: AtomicU8::new(u8::MAX),
            idle_grace_deadline: Arc::new(std::sync::Mutex::new(None)),
            idle_grace_duration: Duration::from_secs(60),
            ring_total_written: Arc::new(AtomicU64::new(0)),
        })
    }
}

// ---------------------------------------------------------------------------
// MockPtyBackend
// ---------------------------------------------------------------------------

/// A fake PTY backend for deterministic, sub-millisecond session tests.
pub struct MockPtyBackend {
    output: Vec<Bytes>,
    chunk_delay: Duration,
    exit_status: ExitStatus,
    drain_input: bool,
    captured_input: Arc<std::sync::Mutex<Vec<Bytes>>>,
}

impl Default for MockPtyBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPtyBackend {
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            chunk_delay: Duration::ZERO,
            exit_status: ExitStatus {
                code: Some(0),
                signal: None,
            },
            drain_input: false,
            captured_input: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn with_output(chunks: Vec<Bytes>) -> Self {
        Self {
            output: chunks,
            ..Self::new()
        }
    }

    pub fn exit_status(mut self, s: ExitStatus) -> Self {
        self.exit_status = s;
        self
    }

    pub fn chunk_delay(mut self, d: Duration) -> Self {
        self.chunk_delay = d;
        self
    }

    pub fn drain_input(mut self) -> Self {
        self.drain_input = true;
        self
    }

    pub fn captured_input(&self) -> Arc<std::sync::Mutex<Vec<Bytes>>> {
        Arc::clone(&self.captured_input)
    }
}

impl Backend for MockPtyBackend {
    fn run(
        &mut self,
        output_tx: mpsc::Sender<Bytes>,
        mut input_rx: mpsc::Receiver<Bytes>,
        _resize_rx: mpsc::Receiver<(u16, u16)>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<ExitStatus>> + Send + '_>> {
        let output = std::mem::take(&mut self.output);
        let chunk_delay = self.chunk_delay;
        let exit_status = self.exit_status;
        let drain_input = self.drain_input;
        let captured_input = Arc::clone(&self.captured_input);

        Box::pin(async move {
            for chunk in output {
                if output_tx.send(chunk).await.is_err() {
                    break;
                }
                if chunk_delay > Duration::ZERO {
                    tokio::time::sleep(chunk_delay).await;
                }
            }
            if drain_input {
                while let Some(data) = input_rx.recv().await {
                    if let Ok(mut buf) = captured_input.lock() {
                        buf.push(data);
                    }
                }
            }
            Ok(exit_status)
        })
    }

    fn resize(&self, _cols: u16, _rows: u16) -> anyhow::Result<()> {
        Ok(())
    }

    fn child_pid(&self) -> Option<u32> {
        None
    }
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/// Extension trait to convert any `Display` error into `anyhow::Error`.
/// Replaces `.map_err(|e| anyhow::anyhow!("{e}"))` with `.anyhow()`.
pub trait AnyhowExt<T> {
    fn anyhow(self) -> anyhow::Result<T>;
}

impl<T, E: std::fmt::Display> AnyhowExt<T> for Result<T, E> {
    fn anyhow(self) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// Assert that an expression evaluates to `Err` whose Display output
/// contains the given substring.
#[macro_export]
macro_rules! assert_err_contains {
    ($expr:expr, $substr:expr) => {{
        let result = $expr;
        let err = result.expect_err(concat!("expected Err for: ", stringify!($expr)));
        let msg = err.to_string();
        assert!(
            msg.contains($substr),
            "expected error containing {:?}, got: {msg:?}",
            $substr
        );
    }};
}
