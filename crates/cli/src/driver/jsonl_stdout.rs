// SPDX-License-Identifier: BUSL-1.1
// Copyright 2025 Alfred Jean LLC

/// Incremental line-buffered parser for newline-delimited JSON on stdout.
#[derive(Debug)]
pub struct JsonlParser {
    pub line_buf: Vec<u8>,
}
