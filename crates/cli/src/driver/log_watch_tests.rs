// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use std::io::Write;

use super::LogWatcher;

#[test]
fn reads_new_lines_from_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.jsonl");
    std::fs::write(&path, "{\"type\":\"system\"}\n{\"type\":\"assistant\"}\n")?;

    let mut watcher = LogWatcher::new(path);
    let lines = watcher.read_new_lines()?;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], r#"{"type":"system"}"#);
    assert_eq!(lines[1], r#"{"type":"assistant"}"#);
    Ok(())
}

#[test]
fn returns_empty_when_file_unchanged() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.jsonl");
    std::fs::write(&path, "{\"line\":1}\n")?;

    let mut watcher = LogWatcher::new(path);
    let _ = watcher.read_new_lines()?;

    // Second read with no changes
    let lines = watcher.read_new_lines()?;
    assert!(lines.is_empty());
    Ok(())
}

#[test]
fn handles_nonexistent_file() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("missing.jsonl");

    let mut watcher = LogWatcher::new(path);
    let lines = watcher.read_new_lines()?;
    assert!(lines.is_empty());
    Ok(())
}

#[test]
fn reports_correct_offset() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.jsonl");
    let content = "{\"a\":1}\n";
    std::fs::write(&path, content)?;

    let mut watcher = LogWatcher::new(path.clone());
    assert_eq!(watcher.offset(), 0);

    let _ = watcher.read_new_lines()?;
    assert_eq!(watcher.offset(), content.len() as u64);

    // Append more data
    let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
    write!(file, "{{\"b\":2}}\n")?;
    drop(file);

    let lines = watcher.read_new_lines()?;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], r#"{"b":2}"#);
    Ok(())
}
