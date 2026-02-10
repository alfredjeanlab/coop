// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::{AgentState, Detector, PromptKind};
use crate::EVENT_CHANNEL_CAPACITY;

/// A state emission from the composite detector, including the tier that
/// produced it.
#[derive(Debug, Clone)]
pub struct DetectedState {
    pub state: AgentState,
    pub tier: u8,
    pub cause: String,
}

/// Combines multiple [`Detector`] tiers to produce a unified agent state
/// stream.
///
/// Tier resolution rules:
/// - Lower tier number = higher confidence.
/// - States from equal-or-higher confidence tiers are accepted immediately.
/// - Lower confidence tiers may only *escalate* state priority; downgrades
///   are silently rejected.
/// - Duplicate states (prev == next) are suppressed.
pub struct CompositeDetector {
    pub tiers: Vec<Box<dyn Detector>>,
}

impl CompositeDetector {
    /// Run the composite detector, spawning all tier detectors and
    /// multiplexing their outputs with tier priority + dedup.
    ///
    /// - `output_tx`: deduplicated state emissions sent to the session loop.
    pub async fn run(
        mut self,
        output_tx: mpsc::Sender<DetectedState>,
        shutdown: CancellationToken,
    ) {
        // Internal channel where each detector sends (tier, state, cause).
        let (tag_tx, mut tag_rx) =
            mpsc::channel::<(u8, AgentState, String)>(EVENT_CHANNEL_CAPACITY);

        // Spawn each detector with a forwarding task that tags with tier.
        for detector in self.tiers.drain(..) {
            let tier = detector.tier();
            let inner_tx = tag_tx.clone();
            let sd = shutdown.clone();
            let (det_tx, mut det_rx) = mpsc::channel::<(AgentState, String)>(16);

            tokio::spawn(detector.run(det_tx, sd));
            tokio::spawn(async move {
                while let Some((state, cause)) = det_rx.recv().await {
                    if inner_tx.send((tier, state, cause)).await.is_err() {
                        break;
                    }
                }
            });
        }
        drop(tag_tx); // only forwarding tasks hold senders

        let mut current_state = AgentState::Starting;
        let mut current_tier: u8 = u8::MAX;

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                tagged = tag_rx.recv() => {
                    let Some((tier, new_state, cause)) = tagged else { break };

                    // Terminal states always accepted immediately.
                    if matches!(new_state, AgentState::Exited { .. }) {
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier, cause }).await;
                        continue;
                    }

                    // Dedup: same state from any tier → update tier tracking only.
                    if new_state == current_state {
                        if tier < current_tier {
                            current_tier = tier;
                        }
                        continue;
                    }

                    // State changed.
                    if tier <= current_tier {
                        // Same or higher confidence → accept immediately,
                        // UNLESS a generic Permission prompt would overwrite
                        // a more specific Plan or Question prompt from the
                        // same tier (Claude fires both notification and
                        // pre_tool_use hooks for the same prompt moment).
                        if tier == current_tier
                            && prompt_supersedes(&current_state, &new_state)
                        {
                            continue;
                        }
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier, cause }).await;
                    } else if new_state.state_priority() > current_state.state_priority() {
                        // Lower confidence tier escalating state → accept.
                        current_state = new_state.clone();
                        current_tier = tier;
                        let _ = output_tx.send(DetectedState { state: new_state, tier, cause }).await;
                    } else {
                        // Lower confidence tier attempting to downgrade or
                        // maintain state priority → reject silently.
                        debug!(
                            tier,
                            new = new_state.as_str(),
                            current = current_state.as_str(),
                            "rejected state downgrade from lower confidence tier"
                        );
                    }
                }
            }
        }
    }
}

/// Returns `true` when `current` is a specific prompt state that should not
/// be overwritten by the more generic `incoming` prompt from the same tier.
///
/// Plan and Question prompts carry richer context than Permission prompts.
/// When the agent fires both a specific pre-tool-use event and a generic
/// permission notification for the same user-facing moment, the specific
/// state should stick.
///
/// Setup prompts are intentionally excluded — they come from the screen
/// detector and represent sequential dialogs (e.g. security_notes →
/// workspace trust), not concurrent events for the same moment.
fn prompt_supersedes(current: &AgentState, incoming: &AgentState) -> bool {
    match (current, incoming) {
        (AgentState::Prompt { prompt: cur }, AgentState::Prompt { prompt: inc }) => {
            inc.kind == PromptKind::Permission
                && matches!(cur.kind, PromptKind::Plan | PromptKind::Question)
        }
        _ => false,
    }
}

impl std::fmt::Debug for CompositeDetector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeDetector").field("tiers", &self.tiers.len()).finish()
    }
}

#[cfg(test)]
#[path = "composite_tests.rs"]
mod composite_tests;
