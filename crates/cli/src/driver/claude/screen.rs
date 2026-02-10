// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::driver::{AgentState, Detector, PromptContext, PromptKind};
use crate::screen::ScreenSnapshot;

/// Tier 5 detector: classifies Claude's rendered terminal screen.
///
/// Detects the idle prompt (`❯`) on the last non-empty line, anti-matching
/// known startup prompts that should be handled separately.
pub struct ClaudeScreenDetector {
    snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    poll: Duration,
}

impl ClaudeScreenDetector {
    pub fn new(
        config: &Config,
        snapshot_fn: Arc<dyn Fn() -> ScreenSnapshot + Send + Sync>,
    ) -> Self {
        Self { snapshot_fn, poll: config.screen_poll() }
    }
}

impl Detector for ClaudeScreenDetector {
    fn run(
        self: Box<Self>,
        state_tx: mpsc::Sender<(AgentState, String)>,
        shutdown: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let mut interval = tokio::time::interval(self.poll);
            let mut last_state: Option<AgentState> = None;

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = interval.tick() => {}
                }

                let snapshot = (self.snapshot_fn)();
                let classified = classify_claude_screen(&snapshot);

                if let Some((ref state, ref cause)) = classified {
                    if last_state.as_ref() != Some(state) {
                        let _ = state_tx.send((state.clone(), cause.clone())).await;
                        last_state = Some(state.clone());
                    }
                } else if last_state.is_some() {
                    last_state = None;
                }
            }
        })
    }

    fn tier(&self) -> u8 {
        5
    }
}

/// Classification of an interactive dialog screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    /// Tool permission dialog — suppressed (handled by Tier 1 hooks).
    ToolPermission,
    /// Workspace trust — emitted as `Prompt(Permission)` with subtype `"trust"`.
    Permission,
    /// Onboarding/setup dialog — emitted as `Prompt(Setup)` with a subtype string.
    Setup(&'static str),
}

/// Classify Claude's screen, returning the state and a cause string.
///
/// Emits `Prompt(Setup)` for onboarding dialogs, `Prompt(Permission)` for
/// workspace trust, and `Idle` for the idle prompt. Tool
/// permission dialogs and startup text prompts are suppressed (`None`).
fn classify_claude_screen(snapshot: &ScreenSnapshot) -> Option<(AgentState, String)> {
    // Classify interactive dialogs first — they take priority over the
    // simple startup text prompts which match more broadly.
    match classify_interactive_dialog(&snapshot.lines) {
        Some(DialogKind::ToolPermission) => return None,
        Some(DialogKind::Permission) => {
            let options = parse_options_from_screen(&snapshot.lines);
            return Some((
                AgentState::Prompt {
                    prompt: PromptContext::new(PromptKind::Permission)
                        .with_subtype("trust")
                        .with_options(options)
                        .with_ready(),
                },
                "screen:permission".to_owned(),
            ));
        }
        Some(DialogKind::Setup(subtype)) => {
            let options = parse_options_from_screen(&snapshot.lines);
            let auth_url =
                if subtype == "oauth_login" { extract_auth_url(&snapshot.lines) } else { None };
            let mut ctx = PromptContext::new(PromptKind::Setup)
                .with_subtype(subtype)
                .with_options(options)
                .with_ready();
            ctx.auth_url = auth_url;
            return Some((AgentState::Prompt { prompt: ctx }, "screen:setup".to_owned()));
        }
        None => {}
    }

    // Emit text-based startup prompts as Prompt(Setup) for API visibility.
    // These are NOT auto-dismissed (no reliable keystroke encoding for text
    // prompts). Checked after dialog classification because the startup
    // detector matches broadly (e.g. "trust this folder" appears in both the
    // simple y/n prompt and the interactive Accessing workspace dialog).
    if let Some(startup) = detect_startup_prompt(&snapshot.lines) {
        let subtype = match startup {
            StartupPrompt::WorkspaceTrust => "startup_trust",
            StartupPrompt::BypassPermissions => "startup_bypass",
            StartupPrompt::LoginRequired => "startup_login",
        };
        return Some((
            AgentState::Prompt {
                prompt: PromptContext::new(PromptKind::Setup).with_subtype(subtype).with_ready(),
            },
            "screen:setup".to_owned(),
        ));
    }

    // Look for Claude's idle prompt indicator anywhere in the visible lines.
    // Claude Code renders `❯` (U+276F) at the start of its input line.
    // Status text like "ctrl+t to hide tasks" may appear below the prompt,
    // so we scan all non-empty lines rather than only the last.
    for line in snapshot.lines.iter().rev() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.starts_with('\u{276f}') {
            return Some((AgentState::Idle, "screen:idle".to_owned()));
        }
    }

    None
}

/// Signal phrases for a dialog screen, paired with its classification.
/// Each screen defines 2-3 signal phrases; a match requires 2+ signals.
/// Signals are `(phrase, case_insensitive)`.
type DialogScreen = (DialogKind, &'static [(&'static str, bool)]);

const DIALOG_SCREENS: &[DialogScreen] = &[
    // Security notes
    (
        DialogKind::Setup("security_notes"),
        &[
            ("Security notes:", false),
            ("Claude can make mistakes", false),
            ("Press Enter to continue", false),
        ],
    ),
    // Login success
    (
        DialogKind::Setup("login_success"),
        &[("Login successful", false), ("Logged in as", false), ("Press Enter to continue", false)],
    ),
    // OAuth login
    (
        DialogKind::Setup("oauth_login"),
        &[("Paste code here if prompted", false), ("oauth/authorize", false)],
    ),
    // Login method picker
    (
        DialogKind::Setup("login_method"),
        &[
            ("Select login method:", false),
            ("Claude account with subscription", false),
            ("Anthropic Console account", false),
        ],
    ),
    // Workspace trust
    (
        DialogKind::Permission,
        &[
            ("Accessing workspace:", false),
            ("Yes, I trust this folder", false),
            ("enter to confirm", true),
        ],
    ),
    // Terminal setup
    (
        DialogKind::Setup("terminal_setup"),
        &[
            ("Use Claude Code's terminal setup?", false),
            ("Yes, use recommended settings", false),
            ("enter to confirm", true),
        ],
    ),
    // Theme picker
    (
        DialogKind::Setup("theme_picker"),
        &[("Choose the text style", false), ("Dark mode", false), ("enter to confirm", true)],
    ),
    // Settings error
    (
        DialogKind::Setup("settings_error"),
        &[
            ("Settings Error", false),
            ("Continue without these settings", false),
            ("Exit and fix manually", false),
        ],
    ),
    // Tool permission
    (
        DialogKind::ToolPermission,
        &[
            ("Do you want to proceed?", false),
            ("Yes, and don't ask again", false),
            ("Esc to cancel", false),
        ],
    ),
];

/// Minimum number of signals that must match to identify a dialog screen.
const DIALOG_SIGNAL_THRESHOLD: usize = 2;

/// Classify the screen as an interactive dialog, returning the dialog kind
/// if recognized, or `None` if no dialog is detected.
///
/// Each known dialog screen defines 2-3 signal phrases; a match requires
/// at least [`DIALOG_SIGNAL_THRESHOLD`] signals present on screen.
fn classify_interactive_dialog(lines: &[String]) -> Option<DialogKind> {
    for (kind, signals) in DIALOG_SCREENS {
        let mut hits = 0;
        for &(phrase, ci) in *signals {
            let found = lines.iter().any(|line| {
                let trimmed = line.trim();
                if ci {
                    trimmed.to_lowercase().contains(phrase)
                } else {
                    trimmed.contains(phrase)
                }
            });
            if found {
                hits += 1;
                if hits >= DIALOG_SIGNAL_THRESHOLD {
                    return Some(*kind);
                }
            }
        }
    }
    None
}

/// Extract an OAuth authorization URL from screen lines.
///
/// Looks for `https://<any-domain>/oauth/authorize?` and may span
/// multiple terminal lines.  Continuation lines start at column 0
/// (no leading whitespace) while surrounding UI text is indented.
fn extract_auth_url(lines: &[String]) -> Option<String> {
    let suffix = "/oauth/authorize?";
    let start_idx = lines.iter().position(|line| {
        let t = line.trim_start();
        t.starts_with("https://") && t.contains(suffix)
    })?;

    let mut url = lines[start_idx].trim().to_string();

    // Concatenate hard-wrapped continuation lines.
    for line in &lines[start_idx + 1..] {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with(' ') {
            break;
        }
        url.push_str(trimmed);
    }

    Some(url)
}

/// Known startup prompts that Claude Code may present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPrompt {
    /// "Do you trust the files in this folder?"
    WorkspaceTrust,
    /// "Allow tool use without prompting?" / --dangerously-skip-permissions
    BypassPermissions,
    /// "Please sign in" / login / onboarding flow
    LoginRequired,
}

/// Classify a screen snapshot as a startup prompt.
///
/// Scans the last non-empty lines of the screen for known prompt patterns.
pub fn detect_startup_prompt(screen_lines: &[String]) -> Option<StartupPrompt> {
    // Work backwards through lines to find the last non-empty content.
    let trimmed: Vec<&str> =
        screen_lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();

    if trimmed.is_empty() {
        return None;
    }

    // Check the last few lines for known patterns.
    let tail = if trimmed.len() >= 5 { &trimmed[trimmed.len() - 5..] } else { &trimmed };
    let combined = tail.join(" ");
    let lower = combined.to_lowercase();

    // Workspace trust prompt
    if lower.contains("trust the files")
        || lower.contains("do you trust")
        || lower.contains("trust this folder")
        || lower.contains("trust this workspace")
    {
        return Some(StartupPrompt::WorkspaceTrust);
    }

    // Permission bypass prompt
    if lower.contains("skip permissions")
        || lower.contains("dangerously-skip-permissions")
        || lower.contains("allow tool use without prompting")
        || lower.contains("bypass permissions")
    {
        return Some(StartupPrompt::BypassPermissions);
    }

    // Login / onboarding prompt
    if lower.contains("please sign in")
        || lower.contains("please log in")
        || lower.contains("login required")
        || lower.contains("sign in to continue")
        || lower.contains("authenticate")
    {
        return Some(StartupPrompt::LoginRequired);
    }

    None
}

/// Parse numbered option labels from terminal screen lines.
///
/// Scans lines bottom-up looking for patterns like `❯ 1. Yes` or `  2. Don't ask again`.
/// Handles Claude's real TUI format:
/// - Selected option: `❯ 1. Label`
/// - Unselected: `  2. Label`
/// - Description lines indented under options (skipped)
/// - Separator lines `────...` and footer hints (skipped)
///
/// Collects matches and stops at the first non-option, non-skippable line above
/// the block. Returns options in ascending order (option 1 first).
pub fn parse_options_from_screen(lines: &[String]) -> Vec<String> {
    let mut options: Vec<(u32, String)> = Vec::new();
    let mut found_any = false;

    for line in lines.iter().rev() {
        let trimmed = line.trim();

        // Skip blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip hint/footer lines (e.g. "Esc to cancel · Tab to amend")
        if is_hint_line(trimmed) {
            continue;
        }

        // Skip separator lines (e.g. "────────────")
        if is_separator_line(trimmed) {
            if found_any {
                // Separator above the options block can appear between groups
                // (e.g. question dialog splits options 1-3 from option 4)
                continue;
            }
            continue;
        }

        // Try to parse as a numbered option
        if let Some((num, label)) = parse_numbered_option(trimmed) {
            options.push((num, label));
            found_any = true;
        } else if found_any {
            // Non-option, non-skippable line. Could be a description line
            // indented under a previous option, or the end of the block.
            // Description lines are deeply indented (5+ spaces) with no
            // leading digit — skip those.
            if is_description_line(line) {
                continue;
            }
            // Otherwise we've hit content above the options block — stop.
            break;
        }
    }

    // Sort by option number ascending and return just the labels
    options.sort_by_key(|(num, _)| *num);
    options.into_iter().map(|(_, label)| label).collect()
}

/// Extract plan prompt context from the terminal screen.
///
/// Plan prompts are detected via the screen rather than the session log,
/// so context is built from the visible screen lines.
pub fn extract_plan_context(_screen: &ScreenSnapshot) -> PromptContext {
    PromptContext::new(PromptKind::Plan)
}

/// Try to parse a line as a numbered option: `[❯ ] N. label`.
///
/// Strips leading selection indicator (`❯`) and whitespace before matching.
/// The `❯` may be followed by a regular space or a non-breaking space (U+00A0).
/// Returns `(number, label)` if the line matches.
fn parse_numbered_option(trimmed: &str) -> Option<(u32, String)> {
    // Strip the selection indicator (❯) if present, then any mix of
    // regular spaces and non-breaking spaces (U+00A0).
    let s = trimmed.strip_prefix('❯').unwrap_or(trimmed);
    let s = s.trim_start_matches([' ', '\u{00A0}']);

    // Must start with one or more digits
    let digit_end = s.find(|c: char| !c.is_ascii_digit())?;
    if digit_end == 0 {
        return None;
    }

    let num: u32 = s[..digit_end].parse().ok()?;

    // Must be followed by ". "
    let rest = s[digit_end..].strip_prefix(". ")?;

    // Label must be non-empty
    if rest.is_empty() {
        return None;
    }

    // Strip trailing selection indicators (e.g. " ✔" or " ✓") that Claude
    // renders after the currently-active option in picker dialogs.
    let label = rest.trim_end().trim_end_matches(['✔', '✓']).trim_end().to_string();

    if label.is_empty() {
        return None;
    }

    Some((num, label))
}

/// Separator lines are composed entirely of box-drawing characters.
fn is_separator_line(trimmed: &str) -> bool {
    !trimmed.is_empty() && trimmed.chars().all(|c| matches!(c, '─' | '╌' | '━' | '═' | '│' | '┃'))
}

/// Hint/footer lines contain navigation instructions.
fn is_hint_line(trimmed: &str) -> bool {
    // Common Claude TUI footer patterns
    trimmed.contains("Esc to cancel")
        || trimmed.contains("Enter to select")
        || trimmed.contains("Enter to confirm")
        || trimmed.contains("Tab to amend")
        || trimmed.contains("Arrow keys to navigate")
}

/// Description lines are indented continuation text under a numbered option.
/// They start with 5+ spaces (deeper than option indentation) and don't begin
/// with a digit (ruling out numbered options themselves).
fn is_description_line(raw_line: &str) -> bool {
    let leading = raw_line.len() - raw_line.trim_start().len();
    if leading < 5 {
        return false;
    }
    let first_non_space = raw_line.trim_start().chars().next();
    !matches!(first_non_space, Some('0'..='9') | Some('❯') | None)
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "startup_tests.rs"]
mod startup_tests;
