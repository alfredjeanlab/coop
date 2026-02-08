// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

//! Integration tests for CompositeDetector tier resolution with MockDetector.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use coop::driver::grace::IdleGraceTimer;
use coop::driver::{AgentState, CompositeDetector, DetectedState, ExitStatus};
use coop::test_support::MockDetector;

/// Helper: run a CompositeDetector with given detectors and collect emitted states.
async fn run_composite(
    detectors: Vec<Box<dyn coop::driver::Detector>>,
    grace_duration: Duration,
    activity_counter: Arc<AtomicU64>,
    collect_timeout: Duration,
) -> anyhow::Result<Vec<DetectedState>> {
    let (output_tx, mut output_rx) = mpsc::channel(64);
    let grace_timer = IdleGraceTimer::new(grace_duration);
    let composite = CompositeDetector {
        tiers: detectors,
        grace_timer,
    };

    let activity_fn: Arc<dyn Fn() -> u64 + Send + Sync> = {
        let counter = Arc::clone(&activity_counter);
        Arc::new(move || counter.load(Ordering::Relaxed))
    };
    let grace_deadline = Arc::new(parking_lot::Mutex::new(None));
    let shutdown = CancellationToken::new();

    let sd = shutdown.clone();
    tokio::spawn(async move {
        composite
            .run(output_tx, activity_fn, grace_deadline, sd)
            .await;
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

// ---------------------------------------------------------------------------
// higher_confidence_wins
// ---------------------------------------------------------------------------

#[tokio::test]
async fn higher_confidence_wins() -> anyhow::Result<()> {
    // Tier 1 (high confidence) emits Working after 50ms
    // Tier 3 (low confidence) emits WaitingForInput after 100ms
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    // Should get Working from tier 1; WaitingForInput from tier 3 should go to grace
    // (not immediately accepted since tier 3 < tier 1 confidence and it's idle)
    assert!(!results.is_empty(), "expected at least one state emission");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 1);

    // WaitingForInput should NOT be in results within 500ms (grace is 60s)
    let has_waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(
        !has_waiting,
        "WaitingForInput from lower tier should be gated by grace"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// lower_confidence_accepted_immediately_for_non_idle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lower_confidence_accepted_immediately_for_non_idle() -> anyhow::Result<()> {
    // Tier 1 emits Starting (initial is Starting so this is deduped)
    // Tier 3 emits Working after 50ms — non-idle from lower confidence accepted immediately
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![
        Box::new(MockDetector::new(1, vec![])),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    assert!(!results.is_empty(), "expected Working from tier 3");
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[0].tier, 3);
    Ok(())
}

// ---------------------------------------------------------------------------
// lower_confidence_idle_triggers_grace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lower_confidence_idle_triggers_grace() -> anyhow::Result<()> {
    // Tier 1 emits Working at 50ms (becomes current state at tier 1)
    // Tier 3 emits WaitingForInput at 100ms — should start grace timer
    // Grace is 2 seconds, so within 500ms it should NOT be emitted
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), AgentState::WaitingForInput)],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(2),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(500),
    )
    .await?;

    // Working should be emitted
    let working = results.iter().any(|s| s.state == AgentState::Working);
    assert!(working, "expected Working state");

    // WaitingForInput should NOT be emitted yet (grace period hasn't elapsed)
    let waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(!waiting, "WaitingForInput should be held by grace timer");
    Ok(())
}

// ---------------------------------------------------------------------------
// grace_cancelled_by_activity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grace_cancelled_by_activity() -> anyhow::Result<()> {
    let activity = Arc::new(AtomicU64::new(0));
    let activity_writer = Arc::clone(&activity);

    // Tier 1 emits Working at 50ms
    // Tier 3 emits WaitingForInput at 150ms — starts grace
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(150), AgentState::WaitingForInput)],
        )),
    ];

    // Simulate activity after 200ms (while grace is pending)
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        activity_writer.store(100, Ordering::Relaxed);
    });

    // Use a short grace period so we can verify it gets invalidated
    let results = run_composite(
        detectors,
        Duration::from_secs(2),
        activity,
        Duration::from_secs(4),
    )
    .await?;

    // WaitingForInput should NOT be emitted because activity cancelled grace
    let waiting = results
        .iter()
        .any(|s| s.state == AgentState::WaitingForInput);
    assert!(!waiting, "WaitingForInput should be cancelled by activity");
    Ok(())
}

// ---------------------------------------------------------------------------
// equal_tier_replaces_state
// ---------------------------------------------------------------------------

#[tokio::test]
async fn equal_tier_replaces_state() -> anyhow::Result<()> {
    // Same tier 2 emits Working then WaitingForInput — both accepted immediately
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![Box::new(MockDetector::new(
        2,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::WaitingForInput),
        ],
    ))];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    assert!(
        results.len() >= 2,
        "expected at least 2 states: {results:?}"
    );
    assert_eq!(results[0].state, AgentState::Working);
    assert_eq!(results[1].state, AgentState::WaitingForInput);
    Ok(())
}

// ---------------------------------------------------------------------------
// terminal_state_always_accepted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_state_always_accepted() -> anyhow::Result<()> {
    // Tier 1 emits Working
    // Tier 3 emits Exited after 100ms — terminal state accepted immediately
    let exit = AgentState::Exited {
        status: ExitStatus {
            code: Some(0),
            signal: None,
        },
    };

    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![
        Box::new(MockDetector::new(
            1,
            vec![(Duration::from_millis(50), AgentState::Working)],
        )),
        Box::new(MockDetector::new(
            3,
            vec![(Duration::from_millis(100), exit.clone())],
        )),
    ];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    let has_exited = results
        .iter()
        .any(|s| matches!(s.state, AgentState::Exited { .. }));
    assert!(
        has_exited,
        "terminal state should be accepted from any tier"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// dedup_suppresses_identical
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dedup_suppresses_identical() -> anyhow::Result<()> {
    // Same detector emits Working twice — second should be deduped
    let detectors: Vec<Box<dyn coop::driver::Detector>> = vec![Box::new(MockDetector::new(
        1,
        vec![
            (Duration::from_millis(50), AgentState::Working),
            (Duration::from_millis(100), AgentState::Working),
        ],
    ))];

    let results = run_composite(
        detectors,
        Duration::from_secs(60),
        Arc::new(AtomicU64::new(0)),
        Duration::from_millis(300),
    )
    .await?;

    // Should only get one Working emission
    let working_count = results
        .iter()
        .filter(|s| s.state == AgentState::Working)
        .count();
    assert_eq!(
        working_count, 1,
        "duplicate state should be suppressed: {results:?}"
    );
    Ok(())
}
