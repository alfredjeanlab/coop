// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

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

#[test]
fn feed_split_utf8_two_byte() -> anyhow::Result<()> {
    let mut screen = Screen::new(80, 24);
    // é is U+00E9, encoded as [0xC3, 0xA9]
    screen.feed(&[0xC3]);
    screen.feed(&[0xA9]);
    let snap = screen.snapshot();
    assert!(
        snap.lines[0].contains('é'),
        "expected é, got: {}",
        snap.lines[0]
    );
    Ok(())
}

#[test]
fn feed_split_utf8_three_byte() -> anyhow::Result<()> {
    let mut screen = Screen::new(80, 24);
    // ★ is U+2605, encoded as [0xE2, 0x98, 0x85]
    screen.feed(&[0xE2]);
    screen.feed(&[0x98, 0x85]);
    let snap = screen.snapshot();
    assert!(
        snap.lines[0].contains('★'),
        "expected ★, got: {}",
        snap.lines[0]
    );
    Ok(())
}

#[test]
fn feed_split_utf8_four_byte() -> anyhow::Result<()> {
    let mut screen = Screen::new(80, 24);
    // 😀 is U+1F600, encoded as [0xF0, 0x9F, 0x98, 0x80]
    screen.feed(&[0xF0, 0x9F]);
    screen.feed(&[0x98, 0x80]);
    let snap = screen.snapshot();
    assert!(
        snap.lines[0].contains('😀'),
        "expected 😀, got: {}",
        snap.lines[0]
    );
    Ok(())
}

#[test]
fn feed_split_utf8_with_surrounding_ascii() -> anyhow::Result<()> {
    let mut screen = Screen::new(80, 24);
    // "abc" + first byte of é
    screen.feed(b"abc\xC3");
    // second byte of é + "def"
    screen.feed(b"\xA9def");
    let snap = screen.snapshot();
    assert!(
        snap.lines[0].contains("abcédef"),
        "expected abcédef, got: {}",
        snap.lines[0]
    );
    Ok(())
}
