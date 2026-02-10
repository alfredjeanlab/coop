// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

use serde::{Deserialize, Serialize};

/// Standard VT100 terminal width.
pub const DEFAULT_COLS: u16 = 80;
/// Standard VT100 terminal height.
pub const DEFAULT_ROWS: u16 = 24;

/// Opaque terminal screen backed by an avt virtual terminal.
pub struct Screen {
    vt: avt::Vt,
    seq: u64,
    changed: bool,
    alt_screen: bool,
    /// Buffer for incomplete UTF-8 trailing bytes between `feed()` calls.
    utf8_buf: [u8; 3],
    utf8_buf_len: u8,
    /// Buffer for trailing bytes that may form an incomplete escape sequence
    /// across `feed()` calls (max sequence length is 8: `\x1b[?1049h`).
    esc_buf: [u8; 7],
    esc_buf_len: u8,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen")
            .field("seq", &self.seq)
            .field("changed", &self.changed)
            .field("alt_screen", &self.alt_screen)
            .finish()
    }
}

/// DECSET alternate screen buffer enable.
const ALT_SCREEN_ON: &[u8] = b"\x1b[?1049h";
/// DECRST alternate screen buffer disable.
const ALT_SCREEN_OFF: &[u8] = b"\x1b[?1049l";

/// Scan `data` for alt screen on/off sequences, updating `alt_screen`.
fn scan_alt_screen(data: &[u8], alt_screen: &mut bool) {
    if data.len() < ALT_SCREEN_ON.len() {
        return;
    }
    if data.windows(ALT_SCREEN_ON.len()).any(|w| w == ALT_SCREEN_ON) {
        *alt_screen = true;
    }
    if data.windows(ALT_SCREEN_OFF.len()).any(|w| w == ALT_SCREEN_OFF) {
        *alt_screen = false;
    }
}

/// Returns the number of trailing bytes that form an incomplete UTF-8 sequence.
///
/// Scans backwards from the end of `data` looking for a leading byte whose
/// expected sequence length exceeds the bytes available.  Returns 0 when the
/// tail is complete (or pure ASCII).
fn incomplete_utf8_tail_len(data: &[u8]) -> usize {
    let len = data.len();
    for i in 1..=len.min(3) {
        let byte = data[len - i];
        if byte < 0x80 {
            // ASCII — no incomplete sequence possible.
            return 0;
        }
        if byte >= 0xC0 {
            // Leading byte: check whether the sequence is complete.
            let expected = if byte < 0xE0 {
                2
            } else if byte < 0xF0 {
                3
            } else {
                4
            };
            return if i < expected { i } else { 0 };
        }
        // Continuation byte (0x80..0xBF) — keep scanning backwards.
    }
    // Only continuation bytes found with no leading byte — not a valid partial
    // sequence, let lossy handle it.
    0
}

impl Screen {
    /// Create a new screen with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            vt: avt::Vt::new(cols as usize, rows as usize),
            seq: 0,
            changed: false,
            alt_screen: false,
            utf8_buf: [0; 3],
            utf8_buf_len: 0,
            esc_buf: [0; 7],
            esc_buf_len: 0,
        }
    }

    /// Feed raw bytes from the PTY into the virtual terminal.
    pub fn feed(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Prepend any buffered incomplete UTF-8 bytes from the previous call.
        let buf_len = self.utf8_buf_len as usize;
        let owned: Vec<u8>;
        let input = if buf_len == 0 {
            data
        } else {
            owned = [&self.utf8_buf[..buf_len], data].concat();
            self.utf8_buf_len = 0;
            &owned
        };

        // Track alt screen transitions from raw escape sequences since
        // avt::Vt doesn't expose the active buffer type.
        //
        // To detect sequences split across PTY read boundaries, we
        // prepend the esc_buf tail from the previous call to the start
        // of input, scan that combined region, then also scan the full
        // input.  Finally, buffer the last 7 bytes for next time.
        let esc_len = self.esc_buf_len as usize;
        if esc_len > 0 {
            // Build a small region: [esc_buf tail | first bytes of input]
            // that is large enough to complete any split sequence.
            let take = input.len().min(ALT_SCREEN_ON.len());
            let mut bridge = [0u8; 15]; // 7 + 8
            bridge[..esc_len].copy_from_slice(&self.esc_buf[..esc_len]);
            bridge[esc_len..esc_len + take].copy_from_slice(&input[..take]);
            let region = &bridge[..esc_len + take];
            scan_alt_screen(region, &mut self.alt_screen);
        }
        scan_alt_screen(input, &mut self.alt_screen);

        // Buffer the last 7 bytes for cross-boundary detection next call.
        let tail_len = input.len().min(7);
        self.esc_buf[..tail_len].copy_from_slice(&input[input.len() - tail_len..]);
        self.esc_buf_len = tail_len as u8;

        // Split off any incomplete UTF-8 trailing bytes to buffer for next call.
        let tail = incomplete_utf8_tail_len(input);
        let (to_feed, to_buffer) = input.split_at(input.len() - tail);

        if !to_buffer.is_empty() {
            self.utf8_buf[..to_buffer.len()].copy_from_slice(to_buffer);
            self.utf8_buf_len = to_buffer.len() as u8;
        }

        if !to_feed.is_empty() {
            let s = String::from_utf8_lossy(to_feed);
            let _ = self.vt.feed_str(&s);
        }

        self.seq += 1;
        self.changed = true;
    }

    /// Capture a point-in-time snapshot of the screen contents.
    pub fn snapshot(&self) -> ScreenSnapshot {
        let (cols, rows) = self.vt.size();
        let cursor = self.vt.cursor();
        let lines: Vec<String> = self.vt.view().map(|line| line.text()).collect();

        ScreenSnapshot {
            lines,
            cols: cols as u16,
            rows: rows as u16,
            alt_screen: self.alt_screen,
            cursor: CursorPosition { row: cursor.row as u16, col: cursor.col as u16 },
            sequence: self.seq,
        }
    }

    /// Whether the terminal is in alt screen mode.
    pub fn is_alt_screen(&self) -> bool {
        self.alt_screen
    }

    /// Whether the screen has been updated since the last `clear_changed`.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// Clear the changed flag.
    pub fn clear_changed(&mut self) {
        self.changed = false;
    }

    /// Current sequence number, incremented on each `feed`.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Resize the virtual terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.vt.resize(cols as usize, rows as usize);
    }
}

/// Point-in-time capture of the terminal screen contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    pub lines: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    pub alt_screen: bool,
    pub cursor: CursorPosition,
    pub sequence: u64,
}

/// Row and column position of the terminal cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorPosition {
    pub row: u16,
    pub col: u16,
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
