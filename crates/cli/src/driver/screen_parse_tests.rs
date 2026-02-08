// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use crate::screen::{CursorPosition, ScreenSnapshot};

use super::{
    classify, compile_config, AgentState, Detector, ScreenParser, ScreenPatternConfig,
    ScreenPatterns,
};

fn make_snapshot(lines: Vec<&str>) -> ScreenSnapshot {
    ScreenSnapshot {
        lines: lines.into_iter().map(String::from).collect(),
        cols: 80,
        rows: 24,
        alt_screen: false,
        cursor: CursorPosition { row: 0, col: 0 },
        sequence: 0,
    }
}

fn test_config() -> ScreenPatternConfig {
    ScreenPatternConfig {
        prompt_pattern: Some(r"^\$ $".to_string()),
        working_patterns: vec!["Compiling".to_string(), "Building".to_string()],
        error_patterns: vec![r"^error:".to_string(), r"^ERROR".to_string()],
    }
}

#[test]
fn compile_valid_config() -> anyhow::Result<()> {
    let config = test_config();
    let patterns = compile_config(&config)?;
    assert!(patterns.prompt.is_some());
    assert_eq!(patterns.working.len(), 2);
    assert_eq!(patterns.error.len(), 2);
    Ok(())
}

#[test]
fn compile_invalid_regex_returns_error() {
    let config = ScreenPatternConfig {
        prompt_pattern: Some(r"[invalid".to_string()),
        working_patterns: vec![],
        error_patterns: vec![],
    };
    assert!(compile_config(&config).is_err());
}

#[test]
fn classify_detects_error() -> anyhow::Result<()> {
    let patterns = compile_config(&test_config())?;
    let snapshot = make_snapshot(vec!["some output", "error: something failed", "$ "]);
    let result = classify(&patterns, &snapshot);
    assert!(matches!(result, Some(AgentState::Error { .. })));
    Ok(())
}

#[test]
fn classify_detects_prompt() -> anyhow::Result<()> {
    let patterns = compile_config(&test_config())?;
    let snapshot = make_snapshot(vec!["some output", "$ "]);
    let result = classify(&patterns, &snapshot);
    assert_eq!(result, Some(AgentState::WaitingForInput));
    Ok(())
}

#[test]
fn classify_detects_working() -> anyhow::Result<()> {
    let patterns = compile_config(&test_config())?;
    let snapshot = make_snapshot(vec!["   Compiling foo v0.1.0", ""]);
    let result = classify(&patterns, &snapshot);
    assert_eq!(result, Some(AgentState::Working));
    Ok(())
}

#[test]
fn classify_returns_none_when_no_match() -> anyhow::Result<()> {
    let patterns = compile_config(&test_config())?;
    let snapshot = make_snapshot(vec!["just some text", "nothing special"]);
    let result = classify(&patterns, &snapshot);
    assert_eq!(result, None);
    Ok(())
}

#[test]
fn error_takes_priority_over_prompt() -> anyhow::Result<()> {
    let patterns = compile_config(&test_config())?;
    let snapshot = make_snapshot(vec!["error: build failed", "$ "]);
    let result = classify(&patterns, &snapshot);
    assert!(matches!(result, Some(AgentState::Error { .. })));
    Ok(())
}

#[test]
fn tier_returns_5() {
    use std::sync::Arc;

    let patterns = ScreenPatterns {
        prompt: None,
        working: vec![],
        error: vec![],
    };
    let parser = ScreenParser::new(patterns, Arc::new(|| make_snapshot(vec![])));
    assert_eq!(parser.tier(), 5);
}

#[test]
fn deserialize_config_from_json() -> anyhow::Result<()> {
    let json = r#"{
        "prompt_pattern": "^\\$ $",
        "working_patterns": ["Compiling"],
        "error_patterns": ["^error:"]
    }"#;
    let config: ScreenPatternConfig = serde_json::from_str(json)?;
    assert_eq!(config.prompt_pattern.as_deref(), Some(r"^\$ $"));
    assert_eq!(config.working_patterns.len(), 1);
    assert_eq!(config.error_patterns.len(), 1);
    Ok(())
}
