// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

/**
 * Offset-gated dedup for PTY replay streams.
 *
 * When a WebSocket client subscribes to `pty` events and requests `replay:get`,
 * there is a race window where broadcast PTY events cover the same byte range
 * as the replay response. Writing both to the terminal corrupts the display.
 *
 * `ReplayGate` tracks the highest committed byte offset and deduplicates
 * incoming Replay and Pty messages so each byte is written exactly once.
 *
 * The Rust mirror lives at `crates/cli/src/replay_gate.rs`.
 * Changes must be mirrored and both test suites must pass.
 *
 * SPDX-License-Identifier: BUSL-1.1
 * Copyright (c) 2026 Alfred Jean LLC
 */

export interface ReplayAction {
  /** Number of leading bytes to skip (already seen). */
  skip: number;
  /** True on the very first replay (caller should reset the terminal). */
  isFirst: boolean;
}

export class ReplayGate {
  /** -1 = pending (pre-replay). Pty messages are dropped in this state. */
  private nextOffset = -1;

  /** Reset to pending state (for reconnect or refresh). */
  reset(): void {
    this.nextOffset = -1;
  }

  /** Current byte offset, or -1 if no replay has been received yet. */
  offset(): number {
    return this.nextOffset;
  }

  /**
   * Process an incoming Replay message.
   *
   * Returns a `ReplayAction` describing how many leading bytes to skip
   * and whether this is the first replay, or `null` to drop the message.
   */
  onReplay(dataLen: number, _offset: number, nextOffset: number): ReplayAction | null {
    const isFirst = this.nextOffset === -1;
    const gate = isFirst ? 0 : this.nextOffset;

    if (!isFirst && nextOffset <= gate) {
      return null; // Entirely behind the high-water mark.
    }

    const replayStart = nextOffset - dataLen;
    const skip = gate > replayStart ? gate - replayStart : 0;
    this.nextOffset = nextOffset;
    return { skip, isFirst };
  }

  /**
   * Process an incoming Pty broadcast message.
   *
   * Returns the number of leading bytes to skip (0 = write all),
   * or `null` to drop the message entirely.
   */
  onPty(dataLen: number, offset: number): number | null {
    if (this.nextOffset === -1) return null; // Pre-replay: drop
    const gate = this.nextOffset;
    const msgEnd = offset + dataLen;
    if (msgEnd <= gate) return null; // Entirely behind the high-water mark.
    const skip = gate > offset ? gate - offset : 0;
    this.nextOffset = msgEnd;
    return skip;
  }
}
