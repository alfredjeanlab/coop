// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

pub mod grace;
pub mod jsonl_stdout;

use grace::IdleGraceTimer;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Classified state of the agent process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentState {
    Starting,
    Working,
    WaitingForInput,
    PermissionPrompt { prompt: PromptContext },
    PlanPrompt { prompt: PromptContext },
    AskUser { prompt: PromptContext },
    Error { detail: String },
    AltScreen,
    Exited { status: ExitStatus },
    Unknown,
}

/// Contextual information about a prompt the agent is presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptContext {
    pub prompt_type: String,
    pub tool: Option<String>,
    pub input_preview: Option<String>,
    pub question: Option<String>,
    pub options: Vec<String>,
    pub summary: Option<String>,
    pub screen_lines: Vec<String>,
}

/// Exit status of the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

/// A single step in a nudge sequence written to the PTY.
#[derive(Debug, Clone)]
pub struct NudgeStep {
    pub bytes: Vec<u8>,
    pub delay_after: Option<Duration>,
}

/// A state detection source that monitors structured data and emits
/// [`AgentState`] transitions.
///
/// Object-safe for use as `Box<dyn Detector>`.
pub trait Detector: Send + 'static {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<AgentState>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    fn tier(&self) -> u8;
}

/// Encodes a plain-text nudge message into PTY byte sequences.
pub trait NudgeEncoder: Send + Sync {
    fn encode(&self, message: &str) -> Vec<NudgeStep>;
}

/// Encodes structured prompt responses into PTY byte sequences.
pub trait RespondEncoder: Send + Sync {
    fn encode_permission(&self, accept: bool) -> Vec<NudgeStep>;
    fn encode_plan(&self, accept: bool, feedback: Option<&str>) -> Vec<NudgeStep>;
    fn encode_question(&self, option: Option<u32>, text: Option<&str>) -> Vec<NudgeStep>;
}

/// Combines multiple [`Detector`] tiers with a grace timer to produce
/// a unified agent state stream.
pub struct CompositeDetector {
    pub tiers: Vec<Box<dyn Detector>>,
    pub grace_timer: IdleGraceTimer,
    pub state_tx: mpsc::Sender<AgentState>,
}

impl std::fmt::Debug for CompositeDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeDetector")
            .field("tiers", &self.tiers.len())
            .field("grace_timer", &self.grace_timer)
            .finish()
    }
}
