// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use serde::{Deserialize, Serialize};

/// Opaque terminal screen backed by an avt virtual terminal.
pub struct Screen {
    vt: avt::Vt,
    seq: u64,
    changed: bool,
    alt_screen: bool,
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

impl Screen {
    /// Create a new screen with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            vt: avt::Vt::new(cols as usize, rows as usize),
            seq: 0,
            changed: false,
            alt_screen: false,
        }
    }

    /// Feed raw bytes from the PTY into the virtual terminal.
    pub fn feed(&mut self, data: &[u8]) {
        if data.is_empty() {
            return;
        }

        // Track alt screen transitions from raw escape sequences since
        // avt::Vt doesn't expose the active buffer type.
        if data
            .windows(ALT_SCREEN_ON.len())
            .any(|w| w == ALT_SCREEN_ON)
        {
            self.alt_screen = true;
        }
        if data
            .windows(ALT_SCREEN_OFF.len())
            .any(|w| w == ALT_SCREEN_OFF)
        {
            self.alt_screen = false;
        }

        let s = String::from_utf8_lossy(data);
        let _ = self.vt.feed_str(&s);
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
            cursor: CursorPosition {
                row: cursor.row as u16,
                col: cursor.col as u16,
            },
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
