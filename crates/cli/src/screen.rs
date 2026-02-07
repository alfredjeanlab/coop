// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

use serde::{Deserialize, Serialize};

/// Opaque terminal screen backed by a VTE.
///
/// Actual fields are added when the `avt` dependency is introduced.
pub struct Screen {
    _private: (),
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen").finish()
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
