// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use super::*;

#[test]
fn feed_plain_text() {
    let mut screen = Screen::new(80, 24);
    screen.feed(b"hello world");
    let snap = screen.snapshot();
    assert!(snap.lines[0].contains("hello world"));
    assert_eq!(snap.sequence, 1);
}

#[test]
fn feed_ansi_color() {
    let mut screen = Screen::new(80, 24);
    // Red text "hi" then reset
    screen.feed(b"\x1b[31mhi\x1b[0m");
    let snap = screen.snapshot();
    assert!(snap.lines[0].contains("hi"));
}

#[test]
fn alt_screen_toggle() {
    let mut screen = Screen::new(80, 24);
    assert!(!screen.is_alt_screen());

    // Enter alt screen
    screen.feed(b"\x1b[?1049h");
    assert!(screen.is_alt_screen());

    // Leave alt screen
    screen.feed(b"\x1b[?1049l");
    assert!(!screen.is_alt_screen());
}

#[test]
fn resize() {
    let mut screen = Screen::new(80, 24);
    screen.resize(40, 10);
    let snap = screen.snapshot();
    assert_eq!(snap.cols, 40);
    assert_eq!(snap.rows, 10);
}

#[test]
fn changed_flag() {
    let mut screen = Screen::new(80, 24);
    assert!(!screen.changed());

    screen.feed(b"x");
    assert!(screen.changed());

    screen.clear_changed();
    assert!(!screen.changed());
}

#[test]
fn empty_feed_is_noop() {
    let mut screen = Screen::new(80, 24);
    screen.feed(b"");
    assert!(!screen.changed());
    assert_eq!(screen.seq(), 0);
}

#[test]
fn cursor_position() {
    let mut screen = Screen::new(80, 24);
    screen.feed(b"abc");
    let snap = screen.snapshot();
    assert_eq!(snap.cursor.col, 3);
    assert_eq!(snap.cursor.row, 0);
}
