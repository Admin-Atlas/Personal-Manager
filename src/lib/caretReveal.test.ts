// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { nextScrollTop } from "./caretReveal";

// A 10-line box showing 5 lines at a time: content 200px, viewport 100px, line 20px.
const box = (over: Partial<Parameters<typeof nextScrollTop>[0]>) =>
  nextScrollTop({
    scrollTop: 0,
    clientHeight: 100,
    caretBottom: 20,
    lineHeight: 20,
    maxScrollTop: 100,
    ...over,
  });

describe("nextScrollTop", () => {
  it("moves nothing when the caret is comfortably inside the view", () => {
    // Scrolled to 40, so lines 3-7 are showing; the caret is on line 5, nowhere near either edge.
    // Calling this after every edit must not fight someone who has scrolled deliberately. The
    // measurement is above `clientHeight`, so it is a real one rather than the floor below.
    expect(box({ scrollTop: 40, caretBottom: 120 })).toBe(40);
  });

  it("scrolls down just enough, plus a line of slack", () => {
    // Caret on line 6 (bottom at 120) with 0..100 visible: 120 + 20 of padding - 100 of viewport.
    expect(box({ scrollTop: 0, caretBottom: 120 })).toBe(40);
  });

  it("scrolls up to a caret above the fold, again with slack", () => {
    // Caret on line 7 (top at 120) while scrolled to 180 → its line, less one line of padding.
    expect(box({ scrollTop: 180, caretBottom: 140, maxScrollTop: 200 })).toBe(100);
  });

  // `caretBottom` comes from `scrollHeight`, which cannot report below the element's own height. So
  // every caret in the first screenful measures as exactly `clientHeight` — it is not a position,
  // it is the floor — and reading it as one is what made pressing Enter on line 2 of a note scroll
  // the note down and take line 1 off the screen.
  describe("a measurement sitting on the scrollHeight floor", () => {
    it("does not scroll down to make room for a line that is already showing", () => {
      // The regression: caret on line 2 of a note scrolled to the top. Treated as a real position
      // this reads as "the line ends exactly at the bottom edge" and asks for a line of slack below
      // it — one line down, and the first line of the note is gone.
      expect(box({ scrollTop: 0, caretBottom: 100 })).toBe(0);
    });

    it("pulls a scrolled note back to the top, not to somewhere in the middle", () => {
      // The other half, and the case the whole function was written for: undo puts the caret back
      // near the top of a note you had scrolled down. The floor cannot say which line, but it does
      // say "within the first screenful", and the top of the box is the one offset that shows every
      // line in it.
      expect(box({ scrollTop: 100, caretBottom: 100 })).toBe(0);
      expect(box({ scrollTop: 100, caretBottom: 20 })).toBe(0);
    });

    it("still trusts a measurement that clears the floor", () => {
      // One pixel past `clientHeight` is a genuine measurement again, and the ordinary minimum-scroll
      // rule takes over — no special case bleeding past the boundary.
      expect(box({ scrollTop: 0, caretBottom: 101 })).toBe(21);
    });
  });

  it("never asks for a scroll the box cannot reach", () => {
    // The caret on the very last line asks for padding that does not exist below it. Clamping is
    // what stops that becoming a scrollTop the browser silently rounds, leaving the reveal a no-op
    // that still reads as done.
    expect(box({ scrollTop: 80, caretBottom: 200, maxScrollTop: 100 })).toBe(100);
    expect(box({ scrollTop: 0, caretBottom: 20, maxScrollTop: 0 })).toBe(0);
  });

  it("never returns a negative offset", () => {
    // The first line asks to be shown one line ABOVE itself, which does not exist.
    expect(box({ scrollTop: 0, caretBottom: 20 })).toBe(0);
    expect(box({ scrollTop: 10, caretBottom: 20 })).toBe(0);
  });

  it("copes with a box that does not scroll at all", () => {
    // A short note: content fits, so maxScrollTop is 0 (or negative, mid-resize) and every answer
    // has to be 0 rather than a positive offset that would blank the box.
    expect(box({ scrollTop: 0, caretBottom: 40, clientHeight: 100, maxScrollTop: -20 })).toBe(0);
  });
});
