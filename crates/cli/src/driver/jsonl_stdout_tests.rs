// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use serde_json::json;

use super::JsonlParser;

#[test]
fn parses_complete_json_lines() {
    let mut parser = JsonlParser::new();
    let data = b"{\"a\":1}\n{\"b\":2}\n";
    let results = parser.feed(data);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], json!({"a": 1}));
    assert_eq!(results[1], json!({"b": 2}));
}

#[test]
fn buffers_partial_lines() {
    let mut parser = JsonlParser::new();

    let results = parser.feed(b"{\"a\":");
    assert!(results.is_empty());

    let results = parser.feed(b"1}\n");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], json!({"a": 1}));
}

#[test]
fn skips_non_json_lines() {
    let mut parser = JsonlParser::new();
    let data = b"not json\n{\"valid\":true}\ngarbage\n";
    let results = parser.feed(data);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], json!({"valid": true}));
}

#[test]
fn empty_input_returns_nothing() {
    let mut parser = JsonlParser::new();
    let results = parser.feed(b"");
    assert!(results.is_empty());
}

#[test]
fn no_trailing_newline_buffers() {
    let mut parser = JsonlParser::new();
    let results = parser.feed(b"{\"pending\":true}");
    assert!(results.is_empty());
    // The buffered content is flushed on next newline
    let results = parser.feed(b"\n");
    assert_eq!(results.len(), 1);
}
