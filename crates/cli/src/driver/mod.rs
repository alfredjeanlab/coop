// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

pub mod claude;
pub mod composite;
pub mod error_category;
pub mod gemini;
pub mod hook_detect;
pub mod hook_recv;
pub mod jsonl_stdout;
pub mod log_watch;
pub mod nudge;
pub mod process;
pub mod screen_parse;
pub mod unknown;

pub use composite::{CompositeDetector, DetectedState};
pub use error_category::{classify_error_detail, ErrorCategory};

use serde::{Deserialize, Serialize};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Known agent types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentType {
    Claude,
    Codex,
    Gemini,
    Unknown,
}

impl fmt::Display for AgentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Claude => f.write_str("claude"),
            Self::Codex => f.write_str("codex"),
            Self::Gemini => f.write_str("gemini"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

/// Classified state of the agent process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentState {
    Starting,
    Working,
    Idle,
    Prompt { prompt: PromptContext },
    Error { detail: String },
    Exited { status: ExitStatus },
    Unknown,
}

/// Distinguishes the type of prompt the agent is presenting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Permission,
    Plan,
    Question,
    Setup,
}

impl PromptKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Plan => "plan",
            Self::Question => "question",
            Self::Setup => "setup",
        }
    }
}

/// Contextual information about a prompt the agent is presenting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptContext {
    /// Prompt type: permission, plan, question, setup.
    #[serde(rename = "type")]
    pub kind: PromptKind,
    /// Prompt subtype for further classification within a kind.
    ///
    /// Known subtypes by kind:
    /// - **permission**: `"trust"` (workspace trust), `"tool"` (tool permission)
    /// - **setup**: `"theme_picker"`, `"terminal_setup"`, `"security_notes"`,
    ///   `"login_success"`, `"login_method"`, `"oauth_login"`
    /// - **plan**, **question**: (none currently)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtype: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// OAuth authorization URL (present during setup oauth_login prompts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// Numbered option labels parsed from the terminal screen (permission/plan prompts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// True when `options` contains fallback labels (parser couldn't find real options).
    #[serde(default)]
    pub options_fallback: bool,
    /// All questions in a multi-question dialog.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<QuestionContext>,
    /// 0-indexed active question; == questions.len() means confirm phase.
    #[serde(default)]
    pub question_current: usize,
    /// True when all async enrichment (e.g. option parsing) is complete.
    /// Permission/Plan prompts start `false` until enrichment finishes;
    /// Question and Setup prompts are immediately `true`.
    #[serde(default)]
    pub ready: bool,
}

impl PromptContext {
    /// Create a new `PromptContext` with the given kind and default fields.
    pub fn new(kind: PromptKind) -> Self {
        Self {
            kind,
            subtype: None,
            tool: None,
            input: None,
            auth_url: None,
            options: vec![],
            options_fallback: false,
            questions: vec![],
            question_current: 0,
            ready: false,
        }
    }

    pub fn with_subtype(mut self, s: impl Into<String>) -> Self {
        self.subtype = Some(s.into());
        self
    }

    pub fn with_tool(mut self, t: impl Into<String>) -> Self {
        self.tool = Some(t.into());
        self
    }

    pub fn with_input(mut self, i: impl Into<String>) -> Self {
        self.input = Some(i.into());
        self
    }

    pub fn with_auth_url(mut self, u: impl Into<String>) -> Self {
        self.auth_url = Some(u.into());
        self
    }

    pub fn with_options(mut self, o: Vec<String>) -> Self {
        self.options = o;
        self
    }

    pub fn with_options_fallback(mut self) -> Self {
        self.options_fallback = true;
        self
    }

    pub fn with_questions(mut self, q: Vec<QuestionContext>) -> Self {
        self.questions = q;
        self
    }

    pub fn with_ready(mut self) -> Self {
        self.ready = true;
        self
    }
}

/// A single question within a multi-question dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionContext {
    pub question: String,
    pub options: Vec<String>,
}

/// An answer to a single question within a dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Exit status of the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
        state_tx: mpsc::Sender<(AgentState, String)>,
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
    fn encode_permission(&self, option: u32) -> Vec<NudgeStep>;
    fn encode_plan(&self, option: u32, feedback: Option<&str>) -> Vec<NudgeStep>;
    fn encode_question(&self, answers: &[QuestionAnswer], total_questions: usize)
        -> Vec<NudgeStep>;
    fn encode_setup(&self, option: u32) -> Vec<NudgeStep>;
}

/// Lifecycle events for hook integrations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HookEvent {
    SessionStart,
    SessionEnd,
    TurnStart,
    TurnEnd,
    ToolAfter {
        tool: String,
    },
    ToolBefore {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_input: Option<serde_json::Value>,
    },
    Notification {
        notification_type: String,
    },
}

/// Driver-provided function that parses numbered option labels from rendered
/// screen lines. Used by the session's prompt enrichment loop.
pub type OptionParser = Arc<dyn Fn(&[String]) -> Vec<String> + Send + Sync>;

/// Compute a scaled nudge delay based on message length.
///
/// For short messages (≤256 bytes), returns the base delay.
/// For longer messages, adds `per_byte` for each byte beyond 256,
/// capped at `max`.
pub fn compute_nudge_delay(
    base: Duration,
    per_byte: Duration,
    max: Duration,
    len: usize,
) -> Duration {
    let extra_bytes = len.saturating_sub(256);
    let scaled = base + per_byte * extra_bytes as u32;
    scaled.min(max)
}

/// If this prompt is a disruption (safe to auto-dismiss), returns the
/// option number to select. Returns `None` for elicitations.
pub fn disruption_option(prompt: &PromptContext) -> Option<u32> {
    match prompt.kind {
        PromptKind::Setup => match prompt.subtype.as_deref() {
            Some("security_notes") => Some(1),
            Some("login_success") => Some(1),
            Some("terminal_setup") => Some(1),
            Some("theme_picker") => Some(1),
            Some("settings_error") => Some(2), // "Continue without these settings"
            _ => None, // oauth_login, login_method, startup_* = elicitations
        },
        PromptKind::Permission => match prompt.subtype.as_deref() {
            Some("trust") => Some(1), // "Yes, I trust this folder"
            _ => None,
        },
        _ => None,
    }
}

impl AgentState {
    /// Return the wire-format string for this state (e.g. `"working"`,
    /// `"prompt"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Prompt { .. } => "prompt",
            Self::Error { .. } => "error",
            Self::Exited { .. } => "exited",
            Self::Unknown => "unknown",
        }
    }

    /// Relative priority for tier-based state resolution.
    ///
    /// Lower-confidence tiers may only *escalate* state (move to a higher
    /// priority); they are never allowed to downgrade it.  Same-or-higher
    /// confidence tiers may transition in any direction.
    ///
    /// ```text
    /// Starting(0) < Idle(1) < Error(2) < Working(3) < Prompt(4)
    /// ```
    ///
    /// `Unknown` is treated the same as `Starting` (lowest).
    /// `Exited` is handled separately (always accepted) and never compared.
    pub fn state_priority(&self) -> u8 {
        match self {
            Self::Starting | Self::Unknown => 0,
            Self::Idle => 1,
            Self::Error { .. } => 2,
            Self::Working => 3,
            Self::Prompt { .. } => 4,
            Self::Exited { .. } => 5,
        }
    }

    /// Extract the prompt context from state variants that carry one.
    pub fn prompt(&self) -> Option<&PromptContext> {
        match self {
            Self::Prompt { prompt } => Some(prompt),
            _ => None,
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod driver_tests;
