// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::config::GroomLevel;
use crate::driver::{
    classify_error_detail, AgentState, AgentType, DetectedState, ErrorCategory, ExitStatus,
    NudgeEncoder, RespondEncoder,
};
use crate::event::{InputEvent, OutputEvent, PromptEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;
use crate::start::StartState;
use crate::stop::StopState;

/// Shared application state passed to all handlers via axum `State` extractor.
///
/// Organized into focused sub-structs by concern:
/// - `terminal`: screen, ring buffer, child process
/// - `driver`: agent detection state machine (with explicit transition methods)
/// - `channels`: channel endpoints for session ↔ transport communication
/// - `config`: static session settings
/// - `lifecycle`: runtime lifecycle primitives
pub struct AppState {
    pub terminal: Arc<TerminalState>,
    pub driver: Arc<DriverState>,
    pub channels: TransportChannels,
    pub config: SessionSettings,
    pub lifecycle: LifecycleState,

    /// Serializes structured input delivery (nudge, respond) and enforces
    /// a minimum inter-delivery gap to prevent garbled terminal input.
    pub delivery_gate: Arc<DeliveryGate>,
    /// Stop hook gating state. Always present (defaults to mode=allow).
    pub stop: Arc<StopState>,
    /// Start hook state. Always present (defaults to empty config).
    pub start: Arc<StartState>,
    /// Notified by the session loop whenever any `InputEvent` is processed.
    /// Used by the enter-retry monitor to cancel itself if other input
    /// activity occurs on the PTY (e.g. raw keys, resize, signal, new delivery).
    pub input_activity: Arc<tokio::sync::Notify>,
}

/// Terminal I/O: screen, ring buffer, child process.
pub struct TerminalState {
    pub screen: RwLock<Screen>,
    pub ring: RwLock<RingBuffer>,
    /// Lock-free mirror of `ring.total_written()` updated by the session loop.
    ///
    /// Arc-wrapped so the session loop can hand a cheap clone to the
    /// [`CompositeDetector`] activity callback without holding a reference
    /// to the entire `TerminalState`.
    pub ring_total_written: Arc<AtomicU64>,
    pub child_pid: AtomicU32,
}

/// Classified error detail and category, stored atomically under a single lock.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    pub detail: String,
    pub category: ErrorCategory,
}

/// Agent detection state machine.
///
/// Owns all fields that participate in agent state transitions so that
/// transition methods can update them atomically without cross-struct
/// ordering concerns.
pub struct DriverState {
    pub agent_state: RwLock<AgentState>,
    pub state_seq: AtomicU64,
    pub detection_tier: AtomicU8,
    /// Freeform cause string from the detector that produced the current state.
    pub detection_cause: RwLock<String>,
    /// Error detail + category when agent is in `Error` state, `None` otherwise.
    /// Combined into a single lock to prevent readers from seeing a torn state
    /// (e.g. detail=Some with category=None).
    pub error: RwLock<Option<ErrorInfo>>,
    /// Last assistant message text (concatenated text blocks from the most recent
    /// assistant JSONL entry). Written directly by log/stdout detectors.
    pub last_message: Arc<RwLock<Option<String>>>,
    /// Whether the agent has transitioned out of `Starting` and is ready.
    pub ready: AtomicBool,
}

impl DriverState {
    /// Format the current detection tier as a display string.
    pub fn detection_tier_str(&self) -> String {
        let tier = self.detection_tier.load(Ordering::Acquire);
        if tier == u8::MAX {
            "none".to_owned()
        } else {
            tier.to_string()
        }
    }

    /// Apply a detected state change, updating all driver fields.
    ///
    /// Returns a [`StateChangeEvent`] ready for broadcast.
    pub async fn apply_detected(&self, detected: DetectedState) -> StateChangeEvent {
        let seq = self.state_seq.fetch_add(1, Ordering::AcqRel) + 1;

        let mut current = self.agent_state.write().await;
        let prev = current.clone();
        *current = detected.state.clone();
        drop(current);

        // Mark ready on first transition away from Starting.
        if matches!(prev, AgentState::Starting) && !matches!(detected.state, AgentState::Starting) {
            self.ready.store(true, Ordering::Release);
        }

        // Store error detail + category when entering Error state.
        if let AgentState::Error { ref detail } = detected.state {
            let category = classify_error_detail(detail);
            *self.error.write().await = Some(ErrorInfo { detail: detail.clone(), category });
        } else {
            *self.error.write().await = None;
        }

        // Store detection metadata.
        self.detection_tier.store(detected.tier, Ordering::Release);
        *self.detection_cause.write().await = detected.cause.clone();

        let last_message = self.last_message.read().await.clone();

        StateChangeEvent { prev, next: detected.state, seq, cause: detected.cause, last_message }
    }

    /// Apply an exit transition.
    ///
    /// Sets `agent_state` to [`AgentState::Exited`] and bumps the sequence
    /// counter.  The exit status is carried inside the `Exited` variant, so
    /// no separate field (or cross-struct ordering) is required.
    ///
    /// Returns a [`StateChangeEvent`] ready for broadcast.
    pub async fn apply_exited(&self, status: ExitStatus) -> StateChangeEvent {
        let seq = self.state_seq.fetch_add(1, Ordering::AcqRel) + 1;

        let mut current = self.agent_state.write().await;
        let prev = current.clone();
        *current = AgentState::Exited { status };
        drop(current);

        let last_message = self.last_message.read().await.clone();

        StateChangeEvent {
            prev,
            next: AgentState::Exited { status },
            seq,
            cause: String::new(),
            last_message,
        }
    }
}

/// Channel endpoints for consumer ↔ session communication.
pub struct TransportChannels {
    pub input_tx: mpsc::Sender<InputEvent>,
    pub output_tx: broadcast::Sender<OutputEvent>,
    pub state_tx: broadcast::Sender<StateChangeEvent>,
    pub prompt_tx: broadcast::Sender<PromptEvent>,
}

/// Static session configuration (immutable after construction).
pub struct SessionSettings {
    pub started_at: Instant,
    pub agent: AgentType,
    pub auth_token: Option<String>,
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    /// Timeout for the enter-retry safety net after nudge delivery.
    /// `Duration::ZERO` disables the retry.
    pub nudge_timeout: Duration,
    /// How aggressively coop auto-responds to agent prompts.
    pub groom: GroomLevel,
}

/// Runtime lifecycle primitives.
pub struct LifecycleState {
    pub shutdown: CancellationToken,
    pub ws_client_count: AtomicI32,
    pub bytes_written: AtomicU64,
}

/// Serializes structured input delivery sequences (nudge, respond) and
/// enforces a minimum inter-delivery gap (debounce) to prevent garbled
/// terminal input when deliveries arrive in rapid succession.
///
/// Terminal-based agents can drop or mis-interpret keystrokes when input
/// arrives faster than the TUI can process.  The gate ensures at least
/// `debounce` time elapses between the end of one delivery and the start
/// of the next.
pub struct DeliveryGate {
    lock: tokio::sync::Mutex<DeliveryGateInner>,
    debounce: Duration,
}

struct DeliveryGateInner {
    last_delivery: Option<Instant>,
    /// Cancel token for any pending enter-retry from the previous delivery.
    retry_cancel: Option<CancellationToken>,
}

impl DeliveryGate {
    pub fn new(debounce: Duration) -> Self {
        Self {
            lock: tokio::sync::Mutex::new(DeliveryGateInner {
                last_delivery: None,
                retry_cancel: None,
            }),
            debounce,
        }
    }

    /// Acquire exclusive delivery access, sleeping if the minimum
    /// inter-delivery gap has not yet elapsed.
    ///
    /// Cancels any pending enter-retry from the previous delivery.
    pub async fn acquire(&self) -> DeliveryGuard<'_> {
        let guard = self.lock.lock().await;
        // Cancel pending retry from previous delivery
        if let Some(ref token) = guard.retry_cancel {
            token.cancel();
        }
        if let Some(last) = guard.last_delivery {
            let elapsed = last.elapsed();
            if elapsed < self.debounce {
                tokio::time::sleep(self.debounce - elapsed).await;
            }
        }
        DeliveryGuard { inner: guard }
    }
}

/// RAII guard returned by [`DeliveryGate::acquire`].
///
/// Records the delivery completion timestamp on drop so that subsequent
/// acquisitions can enforce the debounce interval.
pub struct DeliveryGuard<'a> {
    inner: tokio::sync::MutexGuard<'a, DeliveryGateInner>,
}

impl DeliveryGuard<'_> {
    /// Store a cancellation token for the enter-retry monitor spawned
    /// after this delivery.  The next `acquire()` call will cancel it.
    pub fn set_retry_cancel(&mut self, token: CancellationToken) {
        self.inner.retry_cancel = Some(token);
    }
}

impl Drop for DeliveryGuard<'_> {
    fn drop(&mut self) {
        self.inner.last_delivery = Some(Instant::now());
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agent", &self.config.agent)
            .field("auth_token", &self.config.auth_token.is_some())
            .finish()
    }
}
