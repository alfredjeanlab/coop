// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::time::Duration;

use super::{GraceCheck, IdleGraceTimer};

#[test]
fn new_timer_has_no_pending() {
    let timer = IdleGraceTimer::new(Duration::from_secs(60));
    assert!(!timer.is_pending());
    assert_eq!(timer.check(0), GraceCheck::NotPending);
}

#[test]
fn trigger_sets_pending() {
    let mut timer = IdleGraceTimer::new(Duration::from_secs(60));
    timer.trigger(100);
    assert!(timer.is_pending());
}

#[test]
fn immediate_check_returns_waiting() {
    let mut timer = IdleGraceTimer::new(Duration::from_secs(60));
    timer.trigger(100);
    assert_eq!(timer.check(100), GraceCheck::Waiting);
}

#[test]
fn log_growth_invalidates() {
    let mut timer = IdleGraceTimer::new(Duration::from_secs(60));
    timer.trigger(100);
    assert_eq!(timer.check(200), GraceCheck::Invalidated);
}

#[test]
fn cancel_resets_pending() {
    let mut timer = IdleGraceTimer::new(Duration::from_secs(60));
    timer.trigger(100);
    timer.cancel();
    assert!(!timer.is_pending());
    assert_eq!(timer.check(100), GraceCheck::NotPending);
}

#[test]
fn zero_duration_confirms_immediately() {
    let mut timer = IdleGraceTimer::new(Duration::ZERO);
    timer.trigger(42);
    assert_eq!(timer.check(42), GraceCheck::Confirmed);
}
