// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use super::*;

#[test]
fn parse_tmux_spec() -> anyhow::Result<()> {
    let spec: AttachSpec = "tmux:my-session".parse()?;
    assert_eq!(
        spec,
        AttachSpec::Tmux {
            session: "my-session".to_string()
        }
    );
    Ok(())
}

#[test]
fn parse_screen_spec() -> anyhow::Result<()> {
    let spec: AttachSpec = "screen:my-session".parse()?;
    assert_eq!(
        spec,
        AttachSpec::Screen {
            session: "my-session".to_string()
        }
    );
    Ok(())
}

#[test]
fn parse_unknown_prefix() {
    let result: Result<AttachSpec, _> = "docker:foo".parse();
    let err = result.err();
    assert!(err.is_some());
    assert!(err
        .as_ref()
        .is_some_and(|e| e.to_string().contains("unknown backend")));
}

#[test]
fn parse_empty_name() {
    let result: Result<AttachSpec, _> = "tmux:".parse();
    let err = result.err();
    assert!(err.is_some());
    assert!(err
        .as_ref()
        .is_some_and(|e| e.to_string().contains("empty")));
}

#[test]
fn parse_no_colon() {
    let result: Result<AttachSpec, _> = "tmux".parse();
    assert!(result.is_err());
}
