// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;

use crate::driver::{AgentState, ExitStatus, NudgeEncoder, RespondEncoder};
use crate::error::ErrorCode;
use crate::event::{InputEvent, OutputEvent, StateChangeEvent};
use crate::ring::RingBuffer;
use crate::screen::Screen;

/// Shared application state passed to all handlers via axum `State` extractor.
pub struct AppState {
    pub started_at: Instant,
    pub agent_type: String,
    pub screen: Arc<RwLock<Screen>>,
    pub ring: Arc<RwLock<RingBuffer>>,
    pub agent_state: Arc<RwLock<AgentState>>,
    pub input_tx: mpsc::Sender<InputEvent>,
    pub output_tx: broadcast::Sender<OutputEvent>,
    pub state_tx: broadcast::Sender<StateChangeEvent>,
    pub child_pid: Arc<AtomicU32>,
    pub exit_status: Arc<RwLock<Option<ExitStatus>>>,
    pub write_lock: Arc<WriteLock>,
    pub ws_client_count: Arc<AtomicI32>,
    pub bytes_written: AtomicU64,
    pub auth_token: Option<String>,
    pub nudge_encoder: Option<Arc<dyn NudgeEncoder>>,
    pub respond_encoder: Option<Arc<dyn RespondEncoder>>,
    pub shutdown: CancellationToken,

    /// Serializes multi-step nudge/respond delivery sequences.
    /// The write lock gates *access* (single writer), while the nudge mutex
    /// gates *delivery* (atomic multi-step sequences).
    pub nudge_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Whether the agent has transitioned out of `Starting` and is ready.
    pub ready: Arc<AtomicBool>,

    // Detection metadata
    pub state_seq: AtomicU64,
    pub detection_tier: AtomicU8,
    pub idle_grace_deadline: Arc<Mutex<Option<Instant>>>,
    pub idle_grace_duration: Duration,
    /// Lock-free mirror of `ring.total_written()` updated by the session loop.
    pub ring_total_written: Arc<AtomicU64>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("agent_type", &self.agent_type)
            .field("auth_token", &self.auth_token.is_some())
            .finish()
    }
}

/// Auto-expiry duration for the write lock.
const WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(30);

/// Owner identifier for the write lock.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LockOwner {
    Http,
    Ws(String),
}

/// Internal state for the write lock.
#[derive(Debug)]
struct LockInner {
    owner: Option<LockOwner>,
    acquired_at: Option<Instant>,
}

/// Single-writer concurrency primitive with 30-second auto-release.
///
/// HTTP POST endpoints acquire and release atomically (within a single request).
/// WebSocket clients acquire via `Lock { action: "acquire" }` and release via
/// `Lock { action: "release" }`. The lock auto-expires after 30 seconds.
#[derive(Debug)]
pub struct WriteLock {
    inner: Mutex<LockInner>,
}

/// Guard that releases the HTTP write lock when dropped.
pub struct WriteLockGuard<'a> {
    lock: &'a WriteLock,
}

impl Drop for WriteLockGuard<'_> {
    fn drop(&mut self) {
        let mut inner = match self.lock.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if inner.owner == Some(LockOwner::Http) {
            inner.owner = None;
            inner.acquired_at = None;
        }
    }
}

impl WriteLock {
    /// Create a new unlocked `WriteLock`.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LockInner {
                owner: None,
                acquired_at: None,
            }),
        }
    }

    /// Check if the lock is expired and clear it if so. Must be called with
    /// the inner mutex already held.
    fn maybe_expire(inner: &mut LockInner) {
        if let Some(acquired_at) = inner.acquired_at {
            if acquired_at.elapsed() >= WRITE_LOCK_TIMEOUT {
                inner.owner = None;
                inner.acquired_at = None;
            }
        }
    }

    /// Acquire the write lock for an HTTP request. Returns a guard that
    /// auto-releases on drop. Returns `WriterBusy` if held by another owner.
    pub fn acquire_http(&self) -> Result<WriteLockGuard<'_>, ErrorCode> {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::maybe_expire(&mut inner);
        if inner.owner.is_some() {
            return Err(ErrorCode::WriterBusy);
        }
        inner.owner = Some(LockOwner::Http);
        inner.acquired_at = Some(Instant::now());
        Ok(WriteLockGuard { lock: self })
    }

    /// Acquire the write lock for a WebSocket client. Returns `WriterBusy` if
    /// held by another owner.
    pub fn acquire_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::maybe_expire(&mut inner);
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => return Ok(()),
            Some(_) => return Err(ErrorCode::WriterBusy),
            None => {}
        }
        inner.owner = Some(LockOwner::Ws(client_id.to_owned()));
        inner.acquired_at = Some(Instant::now());
        Ok(())
    }

    /// Release the write lock for a WebSocket client. Returns `WriterBusy`
    /// if the client is not the current owner.
    pub fn release_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::maybe_expire(&mut inner);
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => {
                inner.owner = None;
                inner.acquired_at = None;
                Ok(())
            }
            Some(_) => Err(ErrorCode::WriterBusy),
            None => Ok(()),
        }
    }

    /// Check that the given WebSocket client currently holds the write lock.
    /// Returns `WriterBusy` if the client does not hold the lock.
    pub fn check_ws(&self, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::maybe_expire(&mut inner);
        match &inner.owner {
            Some(LockOwner::Ws(id)) if id == client_id => Ok(()),
            _ => Err(ErrorCode::WriterBusy),
        }
    }

    /// Force-release the lock if held by the given WebSocket client.
    /// Used during connection cleanup.
    pub fn force_release_ws(&self, client_id: &str) {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if inner.owner == Some(LockOwner::Ws(client_id.to_owned())) {
            inner.owner = None;
            inner.acquired_at = None;
        }
    }

    /// Check if the lock is currently held (after expiry check).
    pub fn is_held(&self) -> bool {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        Self::maybe_expire(&mut inner);
        inner.owner.is_some()
    }
}

impl Default for WriteLock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;
