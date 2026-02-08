// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use std::sync::Arc;

use super::{build_detectors, nudge_encoder, respond_encoder};

#[test]
fn build_detectors_without_config_returns_one_tier() -> anyhow::Result<()> {
    let detectors = build_detectors(Arc::new(|| None), Arc::new(|| 0), None, None)?;
    assert_eq!(detectors.len(), 1);
    assert_eq!(detectors[0].tier(), 4);
    Ok(())
}

#[test]
fn build_detectors_config_without_snapshot_fn_errors() {
    let result = build_detectors(
        Arc::new(|| None),
        Arc::new(|| 0),
        Some(std::path::Path::new("/nonexistent/config.json")),
        None,
    );
    assert!(result.is_err());
}

#[test]
fn nudge_encoder_returns_none() {
    assert!(nudge_encoder().is_none());
}

#[test]
fn respond_encoder_returns_none() {
    assert!(respond_encoder().is_none());
}
