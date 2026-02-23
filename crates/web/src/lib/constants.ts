// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

export const MONO_FONT =
  "'Source Code Pro', 'Fira Code', 'Fira Mono', 'SF Mono', 'Cascadia Code', Menlo, Consolas, monospace";

export const THEME = {
  background: "#1e1e1e",
  foreground: "#c9d1d9",
  cursor: "#1e1e1e",
  selectionBackground: "#388bfd44",
} as const;

export const PREVIEW_FONT_SIZE = 9;
export const EXPANDED_FONT_SIZE = 14;
export const TERMINAL_FONT_SIZE = 14;

export const KEY_DEFS = [
  "enter",
  "escape",
  "tab",
  "backspace",
  "delete",
  "up",
  "down",
  "left",
  "right",
  "ctrl-c",
  "ctrl-d",
  "ctrl-z",
  "ctrl-l",
  "ctrl-a",
  "ctrl-e",
] as const;
