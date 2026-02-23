// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import type { CSSProperties } from "react";

/** Parsed ANSI span with text and SGR attributes. */
export interface AnsiSpan {
  text: string;
  bold: boolean;
  faint: boolean;
  italic: boolean;
  underline: boolean;
  strikethrough: boolean;
  inverse: boolean;
  fg: AnsiColor | null;
  bg: AnsiColor | null;
}

type AnsiColor =
  | { kind: "named"; index: number }
  | { kind: "rgb"; r: number; g: number; b: number };

// Standard ANSI 16 colors (normal 0-7, bright 8-15)
const ANSI_COLORS = [
  "#000000",
  "#cd3131",
  "#0dbc79",
  "#e5e510",
  "#2472c8",
  "#bc3fbc",
  "#11a8cd",
  "#e5e5e5",
  "#666666",
  "#f14c4c",
  "#23d18b",
  "#f5f543",
  "#3b8eea",
  "#d670d6",
  "#29b8db",
  "#ffffff",
];

function color256(n: number): string {
  if (n < 16) return ANSI_COLORS[n];
  if (n < 232) {
    // 6x6x6 cube
    const idx = n - 16;
    const b = (idx % 6) * 51;
    const g = (Math.floor(idx / 6) % 6) * 51;
    const r = Math.floor(idx / 36) * 51;
    return `rgb(${r},${g},${b})`;
  }
  // Grayscale 232-255
  const v = (n - 232) * 10 + 8;
  return `rgb(${v},${v},${v})`;
}

interface SgrState {
  bold: boolean;
  faint: boolean;
  italic: boolean;
  underline: boolean;
  strikethrough: boolean;
  inverse: boolean;
  fg: AnsiColor | null;
  bg: AnsiColor | null;
}

function resetState(): SgrState {
  return {
    bold: false,
    faint: false,
    italic: false,
    underline: false,
    strikethrough: false,
    inverse: false,
    fg: null,
    bg: null,
  };
}

function parseColor(params: number[], i: number): { color: AnsiColor | null; consumed: number } {
  const mode = params[i + 1];
  if (mode === 5 && i + 2 < params.length) {
    return { color: { kind: "named", index: params[i + 2] }, consumed: 3 };
  }
  if (mode === 2 && i + 4 < params.length) {
    return {
      color: { kind: "rgb", r: params[i + 2], g: params[i + 3], b: params[i + 4] },
      consumed: 5,
    };
  }
  return { color: null, consumed: 1 };
}

function applySgr(state: SgrState, params: number[]): void {
  if (params.length === 0) {
    Object.assign(state, resetState());
    return;
  }
  let i = 0;
  while (i < params.length) {
    const p = params[i];
    if (p === 0) {
      Object.assign(state, resetState());
    } else if (p === 1) state.bold = true;
    else if (p === 2) state.faint = true;
    else if (p === 3) state.italic = true;
    else if (p === 4) state.underline = true;
    else if (p === 7) state.inverse = true;
    else if (p === 9) state.strikethrough = true;
    else if (p === 22) {
      state.bold = false;
      state.faint = false;
    } else if (p === 23) state.italic = false;
    else if (p === 24) state.underline = false;
    else if (p === 27) state.inverse = false;
    else if (p === 29) state.strikethrough = false;
    else if (p >= 30 && p <= 37) state.fg = { kind: "named", index: p - 30 };
    else if (p === 38) {
      const r = parseColor(params, i);
      state.fg = r.color;
      i += r.consumed - 1;
    } else if (p === 39) state.fg = null;
    else if (p >= 40 && p <= 47) state.bg = { kind: "named", index: p - 40 };
    else if (p === 48) {
      const r = parseColor(params, i);
      state.bg = r.color;
      i += r.consumed - 1;
    } else if (p === 49) state.bg = null;
    else if (p >= 90 && p <= 97) state.fg = { kind: "named", index: p - 90 + 8 };
    else if (p >= 100 && p <= 107) state.bg = { kind: "named", index: p - 100 + 8 };
    i++;
  }
}

// Match ESC [ <params> m (SGR) or any other CSI sequence
// biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI escape sequences require control chars
const CSI_RE = /\x1b\[([0-9;]*)([A-Za-z])/g;
// Match OSC sequences: ESC ] ... (ST | BEL)
// biome-ignore lint/suspicious/noControlCharactersInRegex: ANSI escape sequences require control chars
const OSC_RE = /\x1b\].*?(?:\x1b\\|\x07)/g;

/** Parse a single line of ANSI-escaped text into styled spans. */
export function parseAnsiLine(line: string): AnsiSpan[] {
  const spans: AnsiSpan[] = [];
  const state = resetState();

  // Strip OSC sequences first
  const cleaned = line.replace(OSC_RE, "");

  let lastIndex = 0;
  CSI_RE.lastIndex = 0;

  for (let match = CSI_RE.exec(cleaned); match !== null; match = CSI_RE.exec(cleaned)) {
    // Emit text before this escape
    if (match.index > lastIndex) {
      const text = cleaned.slice(lastIndex, match.index);
      if (text) {
        spans.push({ text, ...state, fg: state.fg, bg: state.bg });
      }
    }
    lastIndex = CSI_RE.lastIndex;

    // Only process SGR (command 'm')
    if (match[2] === "m") {
      const params = match[1] ? match[1].split(";").map(Number) : [];
      applySgr(state, params);
    }
    // Other CSI commands (cursor movement, etc.) are stripped
  }

  // Remaining text after last escape
  if (lastIndex < cleaned.length) {
    const text = cleaned.slice(lastIndex);
    if (text) {
      spans.push({ text, ...state, fg: state.fg, bg: state.bg });
    }
  }

  // Ensure at least one span (empty line)
  if (spans.length === 0) {
    spans.push({ text: "", ...resetState() });
  }

  return spans;
}

/** Convert an AnsiSpan's attributes into CSS properties for React inline styling. */
export function spanStyle(
  span: AnsiSpan,
  theme: { foreground?: string; background?: string },
): CSSProperties | undefined {
  const style: CSSProperties = {};
  let hasStyle = false;

  let fg = span.fg;
  let bg = span.bg;

  if (span.inverse) {
    const tmp = fg;
    fg = bg;
    bg = tmp;
  }

  if (fg) {
    // Brighten named colors when bold
    let resolved: string;
    if (fg.kind === "named" && span.bold && fg.index < 8) {
      resolved = ANSI_COLORS[fg.index + 8];
    } else if (fg.kind === "named" && fg.index < 16) {
      resolved = ANSI_COLORS[fg.index];
    } else if (fg.kind === "named") {
      resolved = color256(fg.index);
    } else {
      resolved = `rgb(${fg.r},${fg.g},${fg.b})`;
    }
    style.color = resolved;
    hasStyle = true;
  } else if (span.inverse) {
    style.color = theme.background ?? "#1e1e1e";
    hasStyle = true;
  }

  if (bg) {
    const resolved =
      bg.kind === "named"
        ? bg.index < 16
          ? ANSI_COLORS[bg.index]
          : color256(bg.index)
        : `rgb(${bg.r},${bg.g},${bg.b})`;
    style.backgroundColor = resolved;
    hasStyle = true;
  } else if (span.inverse) {
    style.backgroundColor = theme.foreground ?? "#c9d1d9";
    hasStyle = true;
  }

  if (span.bold) {
    style.fontWeight = "bold";
    hasStyle = true;
  }
  if (span.faint) {
    style.opacity = 0.5;
    hasStyle = true;
  }
  if (span.italic) {
    style.fontStyle = "italic";
    hasStyle = true;
  }

  const decorations: string[] = [];
  if (span.underline) decorations.push("underline");
  if (span.strikethrough) decorations.push("line-through");
  if (decorations.length > 0) {
    style.textDecoration = decorations.join(" ");
    hasStyle = true;
  }

  return hasStyle ? style : undefined;
}
