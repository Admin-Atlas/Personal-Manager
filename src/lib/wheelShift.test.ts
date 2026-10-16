// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  horizontalPixels,
  isHorizontalGesture,
  SWIPE_CARRY_MS,
  SWIPE_GAP_MS,
  SWIPE_IDLE,
  swipeStep,
  WHEEL_STEP_PX,
  withinRect,
} from "./wheelShift";

const S = WHEEL_STEP_PX;

/** One event: its travel, when it was raised, and when it was handled (by default, at once). */
type Ev = [px: number, raisedAt: number, seenAt?: number];

/** Feed events through a swipe from idle; the days moved, and where it ended. */
function run(events: Ev[]) {
  let state = SWIPE_IDLE;
  const steps: number[] = [];
  for (const [px, raisedAt, seenAt = raisedAt] of events) {
    const r = swipeStep(state, { px, raisedAt, seenAt });
    state = r.state;
    if (r.step !== 0) steps.push(r.step);
  }
  return { steps, state };
}

/** A trackpad swipe: `n` events of `px` each, one frame apart, starting at `at`. */
const swipe = (px: number, n: number, at: number): Ev[] =>
  Array.from({ length: n }, (_, i) => [px, at + i * 16]);

/** The time just after the last event of `events`, plus `gap`. */
const after = (events: Ev[], gap: number) => events[events.length - 1][1] + gap;

describe("swipeStep — one swipe moves one day", () => {
  it("moves nothing until the swipe has travelled far enough", () => {
    expect(run([[10, 0]]).steps).toEqual([]);
    expect(run([[10, 0]]).state.carry).toBe(10);
  });

  it("gathers a trackpad's stream of small deltas into one day", () => {
    expect(run(swipe(10, 6, 0)).steps).toEqual([1]); // 60px of travel at a 55px threshold
  });

  it("moves ONE day however far or fast the swipe runs on", () => {
    // Bobby, 16-10-2026: a flick jumped three days, and then more as the swipe ran on.
    expect(run(swipe(40, 30, 0)).steps).toEqual([1]);
    expect(run([[S * 40, 0]]).steps).toEqual([1]);
    expect(run(swipe(-90, 20, 0)).steps).toEqual([-1]);
  });

  it("swallows the momentum that runs on after the fingers lift", () => {
    // Decaying travel, still a frame apart: the same swipe, so no second day.
    const fling: Ev[] = [80, 60, 40, 25, 12, 6, 3, 1].map((px, i) => [px, i * 16]);
    expect(run(fling).steps).toEqual([1]);
  });

  it("moves another day on the next swipe, after a pause", () => {
    const first = swipe(30, 10, 0);
    const second = swipe(30, 10, after(first, SWIPE_GAP_MS + 1));
    expect(run([...first, ...second]).steps).toEqual([1, 1]);
  });

  it("treats a pause no longer than the gap as the same swipe", () => {
    const first = swipe(30, 4, 0);
    expect(run([...first, ...swipe(30, 4, after(first, SWIPE_GAP_MS))]).steps).toEqual([1]);
  });

  it("doesn't take a backlog held up by drawing the new day for a new swipe", () => {
    // The step at 16ms takes 400ms to draw. The event raised at 32 is handled after it, and the
    // engine hands over everything raised since merged into one event, stamped with its newest time.
    // By raised times alone that reads as a 368ms pause, and a second day would move.
    const held: Ev[] = [
      [30, 0],
      [30, 16],
      [20, 32, 416],
      [150, 400, 417],
      [30, 416, 418],
    ];
    expect(run(held).steps).toEqual([1]);
  });

  it("moves a day back on a flick back, without waiting for the swipe to end", () => {
    // An overshoot undone straight away, even while the first flick's momentum is still arriving:
    // 120px back, past twice the threshold.
    const there = swipe(30, 4, 0);
    const back = swipe(-30, 4, after(there, 16));
    expect(run([...there, ...back]).steps).toEqual([1, -1]);
  });

  it("doesn't take the recoil as a swipe stops for a flick back", () => {
    // A trackpad can report a little travel back as the fingers stop: a day each way would flash
    // the dates forward and back.
    expect(
      run([
        [120, 0],
        [80, 16],
        [30, 32],
        [-60, 48],
      ]).steps,
    ).toEqual([1]);
    expect(
      run([
        [120, 0],
        [80, 16],
        [-20, 32],
        [-20, 48],
        [-20, 64],
      ]).steps,
    ).toEqual([1]);
  });

  it("doesn't take a wobble back in a swipe's momentum for a flick back", () => {
    // Travel back, then on again the same way, three times: each run back starts from nothing, so
    // 120px back in all never adds up to a day.
    const wobbly: Ev[] = [
      [80, 0],
      [30, 16],
      [-40, 32],
      [25, 48],
      [-40, 64],
      [20, 80],
      [-40, 96],
      [12, 112],
    ];
    expect(run(wobbly).steps).toEqual([1]);
  });

  it("spends the travel on the day, so the next swipe starts from nothing", () => {
    expect(run(swipe(30, 4, 0)).state.carry).toBe(0);
  });

  it("moves a day every second notch of a tilt wheel under the threshold, clicked slowly", () => {
    // macOS WebKit reports a notch as 40px. Notches half a second (or two) apart are separate swipes,
    // and the travel short of a day carries from one to the next.
    const notches = (gap: number) => [40, 40, 40, 40].map((px, i): Ev => [px, i * gap]);
    expect(run(notches(500)).steps).toEqual([1, 1]);
    expect(run(notches(2000)).steps).toEqual([1, 1]);
    expect(run([[S, 0]]).steps).toEqual([1]); // one notch of 55px or more is one day
    // Notches faster than the gap, or a held tilt, are one swipe: one day.
    expect(run([40, 40, 40, 40].map((px, i): Ev => [px, i * 100])).steps).toEqual([1]);
  });

  it("forgets travel short of a day after a while", () => {
    // Stray sideways nudges spread across a session never add up to a day.
    expect(
      run([
        [40, 0],
        [40, SWIPE_CARRY_MS + 1],
      ]).steps,
    ).toEqual([]);
  });

  it("lets a swipe back cancel travel that hadn't yet moved a day", () => {
    expect(run([...swipe(25, 2, 0), ...swipe(-25, 2, 32)]).state.carry).toBe(0);
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
    expect(run([[horizontalPixels({ deltaX: 4, deltaMode: 1 }), 0]]).steps).toEqual([1]);
    expect(run([[4, 0]]).steps).toEqual([]); // the old maths
  });

  it("counts a PAGE delta as a whole swipe rather than one pixel", () => {
    // A swipe moves one day whatever its size; this keeps the sign and makes the size count.
    expect(run([[horizontalPixels({ deltaX: -1, deltaMode: 2 }), 0]]).steps).toEqual([-1]);
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
