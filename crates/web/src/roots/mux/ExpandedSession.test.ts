// SPDX-License-Identifier: BUSL-1.1
// Copyright (c) 2026 Alfred Jean LLC

import { afterEach, describe, expect, it, vi } from "vitest";
import { handleReplayReady } from "./ExpandedSession";

describe("handleReplayReady", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("enables stdin and sets ready synchronously", () => {
    vi.stubGlobal("requestAnimationFrame", vi.fn());

    const term = { options: { disableStdin: true }, focus: vi.fn() };
    const setReady = vi.fn();

    handleReplayReady(term, setReady);

    expect(term.options.disableStdin).toBe(false);
    expect(setReady).toHaveBeenCalledWith(true);
  });

  it("defers focus to next animation frame (after React render)", () => {
    const rafCallbacks: (() => void)[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
      rafCallbacks.push(cb);
      return 0;
    });

    const term = { options: { disableStdin: true }, focus: vi.fn() };
    const setReady = vi.fn();

    handleReplayReady(term, setReady);

    // Focus must NOT be called synchronously — the terminal is still
    // invisible at this point because React hasn't re-rendered yet.
    expect(term.focus).not.toHaveBeenCalled();
    expect(rafCallbacks).toHaveLength(1);

    // Simulate the browser firing the animation frame (after React paint)
    for (const cb of rafCallbacks) cb();

    expect(term.focus).toHaveBeenCalledOnce();
  });

  it("calls setReady before scheduling focus", () => {
    const order: string[] = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
      // rAF is scheduled, record that and run it
      order.push("raf-scheduled");
      cb();
      return 0;
    });

    const term = { options: { disableStdin: true }, focus: vi.fn(() => order.push("focus")) };
    const setReady = vi.fn(() => order.push("setReady"));

    handleReplayReady(term, setReady);

    // setReady must come before focus is even scheduled
    expect(order).toEqual(["setReady", "raf-scheduled", "focus"]);
  });
});
