// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Shared test infrastructure: builders, mocks, and assertion helpers.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{
    AgentState, AgentType, Detector, ExitStatus, NudgeEncoder, NudgeStep, RespondEncoder,
};
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::pty::Backend;
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::transport::state::{
    AppState, DriverState, LifecycleState, SessionSettings, TerminalState, TransportChannels,
    WriteLock,
};

/// Builder for constructing `AppState` in tests with sensible defaults.
pub struct AppStateBuilder {
    ring_size: usize,
    child_pid: u32,
    auth_token: Option<String>,
    agent_state: AgentState,
    nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    respond_encoder: Option<Arc<dyn RespondEncoder>>,
}

impl Default for AppStateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AppStateBuilder {
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
            terminal: Arc::new(TerminalState {
                screen: RwLock::new(Screen::new(80, 24)),
                ring: RwLock::new(RingBuffer::new(self.ring_size)),
                ring_total_written: Arc::new(AtomicU64::new(0)),
                child_pid: AtomicU32::new(self.child_pid),
                exit_status: RwLock::new(None),
            }),
            driver: Arc::new(DriverState {
                agent_state: RwLock::new(self.agent_state),
                state_seq: AtomicU64::new(0),
                detection_tier: AtomicU8::new(u8::MAX),
                idle_grace_deadline: Arc::new(parking_lot::Mutex::new(None)),
                error_detail: RwLock::new(None),
                error_category: RwLock::new(None),
            }),
            channels: TransportChannels {
                input_tx,
                output_tx,
                state_tx,
            },
            config: SessionSettings {
                started_at: Instant::now(),
                agent: AgentType::Unknown,
                auth_token: self.auth_token,
                nudge_encoder: self.nudge_encoder,
                respond_encoder: self.respond_encoder,
                idle_grace_duration: Duration::from_secs(60),
            },
            lifecycle: LifecycleState {
                shutdown: CancellationToken::new(),
                write_lock: Arc::new(WriteLock::new()),
                ws_client_count: AtomicI32::new(0),
                bytes_written: AtomicU64::new(0),
            },
            ready: Arc::new(AtomicBool::new(false)),
            nudge_mutex: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

/// A fake PTY backend for deterministic, sub-millisecond session tests.
pub struct MockPty {
    output: Vec<Bytes>,
    chunk_delay: Duration,
    exit_status: ExitStatus,
    drain_input: bool,
    captured_input: Arc<parking_lot::Mutex<Vec<Bytes>>>,
}

impl Default for MockPty {
    fn default() -> Self {
        Self::new()
    }
}

impl MockPty {
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            chunk_delay: Duration::ZERO,
            exit_status: ExitStatus {
                code: Some(0),
                signal: None,
            },
            drain_input: false,
            captured_input: Arc::new(parking_lot::Mutex::new(Vec::new())),
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

    pub fn captured_input(&self) -> Arc<parking_lot::Mutex<Vec<Bytes>>> {
        Arc::clone(&self.captured_input)
    }
}

impl Backend for MockPty {
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
                    captured_input.lock().push(data);
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

/// Stub nudge encoder that passes through message bytes unchanged.
pub struct StubNudgeEncoder;
impl NudgeEncoder for StubNudgeEncoder {
    fn encode(&self, message: &str) -> Vec<NudgeStep> {
        vec![NudgeStep {
            bytes: message.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
}

/// Stub respond encoder for testing permission/plan/question flows.
pub struct StubRespondEncoder;
impl RespondEncoder for StubRespondEncoder {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep> {
        let text = if accept { "y" } else { "n" };
        vec![NudgeStep {
            bytes: text.as_bytes().to_vec(),
            delay_after: None,
        }]
    }
    fn encode_plan(&self, accept: bool, _text: Option<&str>) -> Vec<NudgeStep> {
        self.encode_permission(accept)
    }
    fn encode_question(&self, _option: Option<u32>, text: Option<&str>) -> Vec<NudgeStep> {
        let t = text.unwrap_or("1");
        vec![NudgeStep {
            bytes: t.as_bytes().to_vec(),
            delay_after: None,
        }]
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

/// A configurable detector for testing [`CompositeDetector`] tier resolution.
///
/// Emits a sequence of `(delay, state)` pairs, then waits for shutdown.
pub struct MockDetector {
    tier_val: u8,
    states: Vec<(Duration, AgentState)>,
}

impl MockDetector {
    pub fn new(tier: u8, states: Vec<(Duration, AgentState)>) -> Self {
        Self {
            tier_val: tier,
            states,
        }
    }
}

impl Detector for MockDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            for (delay, state) in self.states {
                tokio::select! {
                    _ = shutdown.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {
                        if state_tx.send(state).await.is_err() {
                            return;
                        }
                    }
                }
            }
            shutdown.cancelled().await;
        })
    }

    fn tier(&self) -> u8 {
        self.tier_val
    }
}

/// Spawn an HTTP server on a random port for integration testing.
///
/// Returns the bound address and a join handle for the server task.
pub async fn spawn_http_server(
    app_state: Arc<AppState>,
) -> anyhow::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let router = crate::transport::build_router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((addr, handle))
}

/// Spawn a gRPC server on a random port for integration testing.
///
/// Returns the bound address and a join handle for the server task.
pub async fn spawn_grpc_server(
    app_state: Arc<AppState>,
) -> anyhow::Result<(std::net::SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let grpc = crate::transport::grpc::CoopGrpc::new(app_state);
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let handle = tokio::spawn(async move {
        let _ = grpc.into_router().serve_with_incoming(incoming).await;
    });
    Ok((addr, handle))
}
