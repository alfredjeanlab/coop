// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{CompositeDetector, DetectedState};
use crate::driver::{AgentState, ExitStatus, PromptContext, PromptKind};
use crate::test_support::MockDetector;
use crate::EVENT_CHANNEL_CAPACITY;

/// Helper: run a CompositeDetector with given detectors and collect emitted states.
async fn run_composite(
    detectors: Vec<Box<dyn crate::driver::Detector>>,
    collect_timeout: Duration,
) -> anyhow::Result<Vec<DetectedState>> {
    let (output_tx, mut output_rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
    let composite = CompositeDetector { tiers: detectors };

    let shutdown = CancellationToken::new();

    let sd = shutdown.clone();
    tokio::spawn(async move {
        composite.run(output_tx, sd).await;
    });

    let mut results = Vec::new();
    let deadline = tokio::time::Instant::now() + collect_timeout;

    loop {
        tokio::select! {
            state = output_rx.recv() => {
                match state {
                    Some(s) => results.push(s),
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }

    shutdown.cancel();
    Ok(results)
}

#[tokio::test]
async fn higher_confidence_wins() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![(Duration::from_millis(50), AgentState::Working)])),
        Box::new(MockDetector::new(3, vec![(Duration::from_millis(100), AgentState::Idle)])),
    ];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 1);

    let has_waiting = results.iter().any(|s| s.state == AgentState::Idle);
    assert!(!has_waiting, "Idle from lower tier should be rejected as state downgrade");
    Ok(())
}

#[tokio::test]
async fn lower_confidence_escalation_accepted() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![])),
        Box::new(MockDetector::new(3, vec![(Duration::from_millis(50), AgentState::Working)])),
    ];

    let results = run_composite(detectors, Duration::from_millis(300)).await?;

    assert!(!results.is_empty(), "expected Working from tier 3");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 3);
    Ok(())
}

#[tokio::test]
async fn lower_confidence_downgrade_rejected() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![(Duration::from_millis(50), AgentState::Working)])),
        Box::new(MockDetector::new(3, vec![(Duration::from_millis(100), AgentState::Idle)])),
    ];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    let working = results.iter().any(|s| s.state == AgentState::Working);
    assert!(working, "expected Working state");

    let waiting = results.iter().any(|s| s.state == AgentState::Idle);
    assert!(!waiting, "Idle from lower tier should be rejected as state downgrade");
    Ok(())
}

#[tokio::test]
async fn equal_tier_replaces_state() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![Box::new(MockDetector::new(
        2,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::Idle),
        ],
    ))];

    let results = run_composite(detectors, Duration::from_millis(300)).await?;

    assert!(results.len() >= 2, "expected at least 2 states: {results:?}");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[1].state, AgentState::Idle);
    Ok(())
}

#[tokio::test]
async fn terminal_state_always_accepted() -> anyhow::Result<()> {
    let exit = AgentState::Exited { status: ExitStatus { code: Some(0), signal: None } };

    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![(Duration::from_millis(50), AgentState::Working)])),
        Box::new(MockDetector::new(3, vec![(Duration::from_millis(100), exit.clone())])),
    ];

    let results = run_composite(detectors, Duration::from_millis(300)).await?;

    let has_exited = results.iter().any(|s| matches!(s.state, AgentState::Exited { .. }));
    assert!(has_exited, "terminal state should be accepted from any tier");
    Ok(())
}

#[tokio::test]
async fn dedup_suppresses_identical() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![Box::new(MockDetector::new(
        1,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::Working),
        ],
    ))];

    let results = run_composite(detectors, Duration::from_millis(300)).await?;

    let working_count = results.iter().filter(|s| s.state == AgentState::Working).count();
    assert_eq!(working_count, 1, "duplicate state should be suppressed: {results:?}");
    Ok(())
}

fn empty_prompt(kind: PromptKind) -> PromptContext {
    PromptContext::new(kind)
}

#[tokio::test]
async fn tier1_supersedes_tier5_screen_idle() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![(Duration::from_millis(50), AgentState::Working)])),
        Box::new(MockDetector::new(5, vec![(Duration::from_millis(100), AgentState::Idle)])),
    ];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 1);

    let has_waiting = results.iter().any(|s| s.state == AgentState::Idle);
    assert!(!has_waiting, "tier 5 Idle should be rejected as downgrade from Working");
    Ok(())
}

#[tokio::test]
async fn tier2_supersedes_tier5_screen_idle() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(2, vec![(Duration::from_millis(50), AgentState::Working)])),
        Box::new(MockDetector::new(5, vec![(Duration::from_millis(100), AgentState::Idle)])),
    ];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 2);

    let has_waiting = results.iter().any(|s| s.state == AgentState::Idle);
    assert!(!has_waiting, "tier 5 Idle should be rejected as downgrade from Working");
    Ok(())
}

/// Tier 5 can escalate from Idle to Prompt (e.g. detecting a
/// setup dialog on screen while tier 1 only saw idle).
#[tokio::test]
async fn tier5_can_escalate_to_prompt() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![(Duration::from_millis(50), AgentState::Idle)])),
        Box::new(MockDetector::new(
            5,
            vec![(
                Duration::from_millis(100),
                AgentState::Prompt { prompt: empty_prompt(PromptKind::Permission) },
            )],
        )),
    ];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    let has_prompt = results.iter().any(|s| matches!(s.state, AgentState::Prompt { .. }));
    assert!(has_prompt, "tier 5 Prompt should be accepted as escalation from Idle");
    Ok(())
}

/// Regression: screen detector transitions from Setup(security_notes) to
/// Permission(trust) when the user dismisses one dialog and the next appears.
/// This must NOT be blocked by prompt_supersedes — Setup and Permission are
/// sequential dialogs, not concurrent events.
#[tokio::test]
async fn setup_to_permission_transition_accepted() -> anyhow::Result<()> {
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![Box::new(MockDetector::new(
        5,
        vec![
            (
                Duration::from_millis(50),
                AgentState::Prompt {
                    prompt: PromptContext::new(PromptKind::Setup).with_subtype("security_notes"),
                },
            ),
            (
                Duration::from_millis(100),
                AgentState::Prompt {
                    prompt: PromptContext::new(PromptKind::Permission).with_subtype("trust"),
                },
            ),
        ],
    ))];

    let results = run_composite(detectors, Duration::from_millis(500)).await?;

    assert!(results.len() >= 2, "expected both Setup and Permission: {results:?}");
    let has_permission = results.iter().any(|s| {
        matches!(
            s.state,
            AgentState::Prompt { prompt: PromptContext { kind: PromptKind::Permission, .. } }
        )
    });
    assert!(has_permission, "Permission prompt must not be blocked by preceding Setup prompt");
    Ok(())
}

/// Regression: Claude fires both `ToolBefore(ExitPlanMode)` → Prompt(Plan) and
/// `Notification(permission_prompt)` → Prompt(Permission) for the same user-facing
/// plan approval moment. When the permission notification arrives after the
/// ToolBefore event, the composite detector must not let the generic
/// Permission prompt overwrite the more specific Plan prompt.
#[tokio::test]
async fn plan_prompt_not_overwritten_by_permission_prompt() -> anyhow::Result<()> {
    // Simulate tier 1 emitting Plan prompt then Permission prompt in quick succession.
    let detectors: Vec<Box<dyn crate::driver::Detector>> = vec![Box::new(MockDetector::new(
        1,
        vec![
            (
                Duration::from_millis(50),
                AgentState::Prompt { prompt: empty_prompt(PromptKind::Plan) },
            ),
            (
                Duration::from_millis(10),
                AgentState::Prompt { prompt: empty_prompt(PromptKind::Permission) },
            ),
        ],
    ))];

    let results = run_composite(detectors, Duration::from_millis(300)).await?;

    // The final settled state should be Plan prompt, not Permission prompt.
    let last = results.last().expect("expected at least one state emission");
    assert!(
        matches!(
            last.state,
            AgentState::Prompt { prompt: PromptContext { kind: PromptKind::Plan, .. } }
        ),
        "expected final state to be Plan prompt, got {:?}",
        last.state,
    );
    Ok(())
}
