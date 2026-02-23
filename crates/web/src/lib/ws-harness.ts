// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * WsMessageHarness — test and debug harness for PTY replay dedup.
 *
 * Processes a sequence of Replay and Pty WebSocket messages through a
 * `ReplayGate` and records every write decision. Detects gaps (byte ranges
 * never written) and overlaps (byte ranges written more than once).
 *
 * Importable by the main app for shadow-recording WS traffic during
 * debugging sessions.
 *
 * SPDX-License-Identifier: BUSL-1.1
 * Copyright (c) 2026 Alfred Jean LLC
 */

import { ReplayGate } from "./replay-gate";

export interface WriteRecord {
  kind: "replay" | "pty";
  offset: number;
  dataLen: number;
  skip: number;
  written: number;
  dropped: boolean;
  isFirst?: boolean;
  gateAfter: number;
}

export interface Gap {
  from: number;
  to: number;
}
export interface Overlap {
  from: number;
  to: number;
}

export interface StreamIssue {
  kind: "gaps" | "overlaps" | "stale_leak" | "large_drop_rate";
  message: string;
  count: number;
}

/** Range of bytes actually committed to the output buffer. */
interface WrittenRange {
  from: number;
  to: number;
}

export class WsMessageHarness {
  gate = new ReplayGate();
  resets = 0;
  writes: WriteRecord[] = [];

  private buf: number[] = [];
  private ranges: WrittenRange[] = [];

  /** Feed a Replay message. */
  replay(data: Uint8Array | string, offset: number, nextOffset: number): void {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const action = this.gate.onReplay(bytes.length, offset, nextOffset);
    if (!action) {
      this.writes.push({
        kind: "replay",
        offset,
        dataLen: bytes.length,
        skip: 0,
        written: 0,
        dropped: true,
        gateAfter: this.gate.offset(),
      });
      return;
    }
    if (action.isFirst) {
      this.buf = [];
      this.ranges = [];
      this.resets++;
    }
    const written = bytes.length - action.skip;
    for (let i = action.skip; i < bytes.length; i++) this.buf.push(bytes[i]);
    const rangeStart = nextOffset - bytes.length + action.skip;
    if (written > 0) this.ranges.push({ from: rangeStart, to: rangeStart + written });
    this.writes.push({
      kind: "replay",
      offset,
      dataLen: bytes.length,
      skip: action.skip,
      written,
      dropped: false,
      isFirst: action.isFirst,
      gateAfter: this.gate.offset(),
    });
  }

  /** Feed a Pty broadcast message. */
  pty(data: Uint8Array | string, offset: number): void {
    const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
    const skip = this.gate.onPty(bytes.length, offset);
    if (skip === null) {
      this.writes.push({
        kind: "pty",
        offset,
        dataLen: bytes.length,
        skip: 0,
        written: 0,
        dropped: true,
        gateAfter: this.gate.offset(),
      });
      return;
    }
    const written = bytes.length - skip;
    for (let i = skip; i < bytes.length; i++) this.buf.push(bytes[i]);
    const rangeStart = offset + skip;
    if (written > 0) this.ranges.push({ from: rangeStart, to: rangeStart + written });
    this.writes.push({
      kind: "pty",
      offset,
      dataLen: bytes.length,
      skip,
      written,
      dropped: false,
      gateAfter: this.gate.offset(),
    });
  }

  /** Simulate a WebSocket reconnect (gate reset). */
  reconnect(): void {
    this.gate.reset();
    this.buf = [];
    this.ranges = [];
  }

  /** Raw output bytes. */
  output(): Uint8Array {
    return new Uint8Array(this.buf);
  }

  /** Output as UTF-8 string. */
  outputStr(): string {
    return new TextDecoder().decode(this.output());
  }

  /** Detect gaps — byte ranges that were never written. */
  gaps(): Gap[] {
    const merged = mergeRanges(this.ranges);
    const result: Gap[] = [];
    for (let i = 1; i < merged.length; i++) {
      if (merged[i].from > merged[i - 1].to) {
        result.push({ from: merged[i - 1].to, to: merged[i].from });
      }
    }
    return result;
  }

  /** Detect overlaps — byte ranges written more than once. */
  overlaps(): Overlap[] {
    if (this.ranges.length < 2) return [];
    const sorted = [...this.ranges].sort((a, b) => a.from - b.from || a.to - b.to);
    const result: Overlap[] = [];
    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i].from < sorted[i - 1].to) {
        result.push({ from: sorted[i].from, to: Math.min(sorted[i - 1].to, sorted[i].to) });
      }
    }
    return mergeRanges(result) as Overlap[];
  }

  /** Diagnose all detectable stream health issues. Empty array = healthy. */
  diagnose(): StreamIssue[] {
    const issues: StreamIssue[] = [];

    const g = this.gaps();
    if (g.length > 0) {
      const totalBytes = g.reduce((sum, gap) => sum + (gap.to - gap.from), 0);
      issues.push({
        kind: "gaps",
        message: `${g.length} gap(s) detected — ${totalBytes} bytes missing from PTY stream`,
        count: g.length,
      });
    }

    const o = this.overlaps();
    if (o.length > 0) {
      issues.push({
        kind: "overlaps",
        message: `${o.length} overlap(s) detected — duplicate data written (stale-gate race)`,
        count: o.length,
      });
    }

    // Stale leak: non-dropped PTY writes that occurred before the most recent
    // isFirst replay (writes accepted by stale gate before reconnect reset).
    let lastResetIdx = -1;
    for (let i = this.writes.length - 1; i >= 0; i--) {
      if (this.writes[i].isFirst) {
        lastResetIdx = i;
        break;
      }
    }
    if (lastResetIdx > 0) {
      const leaked = this.writes.filter(
        (w: WriteRecord, i: number) => i < lastResetIdx && !w.dropped && w.kind === "pty",
      );
      // Only flag PTY writes whose offset would be covered by the post-reset replay
      const resetReplay = this.writes[lastResetIdx];
      const replayEnd = resetReplay.offset + resetReplay.dataLen;
      const staleLeaks = leaked.filter(
        (w) => w.offset + w.written > resetReplay.offset && w.offset < replayEnd,
      );
      if (staleLeaks.length > 0) {
        issues.push({
          kind: "stale_leak",
          message: `${staleLeaks.length} PTY write(s) leaked through stale gate before reconnect reset`,
          count: staleLeaks.length,
        });
      }
    }

    // Large drop rate: if >20% of PTY writes were dropped, something is wrong.
    const ptyWrites = this.writes.filter((w) => w.kind === "pty");
    if (ptyWrites.length >= 5) {
      const dropped = ptyWrites.filter((w) => w.dropped).length;
      const rate = dropped / ptyWrites.length;
      if (rate > 0.2) {
        issues.push({
          kind: "large_drop_rate",
          message: `${Math.round(rate * 100)}% of PTY writes dropped (${dropped}/${ptyWrites.length})`,
          count: dropped,
        });
      }
    }

    return issues;
  }
}

function mergeRanges(ranges: { from: number; to: number }[]): { from: number; to: number }[] {
  if (ranges.length === 0) return [];
  const sorted = [...ranges].sort((a, b) => a.from - b.from);
  const merged: { from: number; to: number }[] = [{ ...sorted[0] }];
  for (let i = 1; i < sorted.length; i++) {
    const last = merged[merged.length - 1];
    if (sorted[i].from <= last.to) {
      last.to = Math.max(last.to, sorted[i].to);
    } else {
      merged.push({ ...sorted[i] });
    }
  }
  return merged;
}
