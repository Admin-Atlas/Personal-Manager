// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  accumulateShift,
  horizontalPixels,
  isHorizontalGesture,
  WHEEL_STEP_PX,
  withinRect,
} from "./wheelShift";

const S = WHEEL_STEP_PX;

describe("accumulateShift", () => {
  it("emits nothing until the threshold is crossed", () => {
    const r = accumulateShift(0, 10);
    expect(r.steps).toBe(0);
    expect(r.carry).toBe(10);
  });

  it("accumulates a trackpad's stream of small deltas into one step", () => {
    let carry = 0;
    let total = 0;
    for (let i = 0; i < 6; i++) {
      const r = accumulateShift(carry, 10);
      carry = r.carry;
      total += r.steps;
    }
    expect(total).toBe(1); // 60px of travel at a 55px threshold
  });

  it("turns one notch of a tilt wheel into exactly one day", () => {
    expect(accumulateShift(0, S).steps).toBe(1);
    expect(accumulateShift(0, -S).steps).toBe(-1);
  });

  it("keeps the remainder so a slow swipe doesn't lose travel", () => {
    const r = accumulateShift(0, S + 20);
    expect(r.steps).toBe(1);
    expect(r.carry).toBe(20);
  });

  it("caps a fling so the view can't leap most of a month at once", () => {
    expect(accumulateShift(0, S * 40).steps).toBe(3);
    expect(accumulateShift(0, -S * 40).steps).toBe(-3);
  });

  it("drops the carry when it capped, so the discard can't fire on the NEXT flick", () => {
    // Keeping the remainder of a capped fling would leave hundreds of pixels banked, and the next
    // unrelated nudge would immediately jump another 3 days.
    expect(accumulateShift(0, S * 40).carry).toBe(0);
  });

  it("reverses cleanly — a swipe back undoes a swipe forward", () => {
    const fwd = accumulateShift(0, S);
    const back = accumulateShift(fwd.carry, -S);
    expect(fwd.steps + back.steps).toBe(0);
  });
});

describe("horizontalPixels — the threshold is in pixels, so the delta must be too", () => {
  it("passes a pixel delta through untouched", () => {
    expect(horizontalPixels({ deltaX: 120, deltaMode: 0 })).toBe(120);
    expect(horizontalPixels({ deltaX: -37.5, deltaMode: 0 })).toBe(-37.5);
  });

  it("scales a LINE delta, which is otherwise ~55x too small to ever cross a step", () => {
    // A line-mode notch reports deltaX: 1. Compared raw against WHEEL_STEP_PX it takes 55 notches
    // to move one day, which is indistinguishable from the gesture not working at all.
    expect(horizontalPixels({ deltaX: 1, deltaMode: 1 })).toBe(16);
    expect(accumulateShift(0, horizontalPixels({ deltaX: 4, deltaMode: 1 })).steps).toBe(1);
    expect(accumulateShift(0, { deltaX: 4, deltaMode: 1 }.deltaX).steps).toBe(0); // the old maths
  });

  it("keeps a PAGE delta to a sane jump rather than an unbounded one", () => {
    // Capped downstream at MAX_STEPS_PER_EVENT regardless; this just keeps the sign and magnitude
    // meaningful instead of treating "one page" as one pixel.
    expect(accumulateShift(0, horizontalPixels({ deltaX: -1, deltaMode: 2 })).steps).toBe(-3);
  });
});

describe("withinRect — ownership survives the re-render the step itself causes", () => {
  const grid = { left: 100, right: 500, top: 60, bottom: 400 };

  it("owns a pointer inside the region", () => {
    expect(withinRect(grid, 300, 200)).toBe(true);
    expect(withinRect(grid, 100, 60)).toBe(true); // top-left corner is inside
  });

  it("disowns a pointer outside it, on either axis", () => {
    expect(withinRect(grid, 99, 200)).toBe(false);
    expect(withinRect(grid, 300, 401)).toBe(false);
  });

  it("treats the far edges as outside, so two side-by-side regions never both claim a pixel", () => {
    expect(withinRect(grid, 500, 200)).toBe(false);
    expect(withinRect(grid, 300, 400)).toBe(false);
  });

  it("never owns a zero-extent box — an unmounted region reports 0×0 at the origin", () => {
    // Without this, a hidden or not-yet-laid-out grid would claim every wheel event in the
    // top-left corner of the window.
    const collapsed = { left: 0, right: 0, top: 0, bottom: 0 };
    expect(withinRect(collapsed, 0, 0)).toBe(false);
    expect(withinRect({ left: 10, right: 10, top: 0, bottom: 50 }, 10, 25)).toBe(false);
  });
});

describe("isHorizontalGesture", () => {
  const g = (
    deltaX: number,
    deltaY: number,
    mod: Partial<{ ctrlKey: boolean; shiftKey: boolean }> = {},
  ) => isHorizontalGesture({ deltaX, deltaY, ctrlKey: false, shiftKey: false, ...mod });

  it("is true for a clearly sideways gesture", () => {
    expect(g(40, 2)).toBe(true);
    expect(g(-40, 0)).toBe(true);
  });

  it("is false for a vertical scroll, including a trackpad's cross-axis noise", () => {
    expect(g(0, 40)).toBe(false);
    expect(g(3, 40)).toBe(false);
  });

  it("is false on a tie, so a perfect diagonal never steals a vertical scroll", () => {
    expect(g(20, 20)).toBe(false);
  });

  it("leaves Ctrl (zoom) and Shift (native sideways scroll) to their existing handlers", () => {
    expect(g(40, 0, { ctrlKey: true })).toBe(false);
    expect(g(40, 0, { shiftKey: true })).toBe(false);
  });
});
