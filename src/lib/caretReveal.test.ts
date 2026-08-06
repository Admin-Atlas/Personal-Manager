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
    // Line 3 of 5 visible, nowhere near either edge — calling this after every edit must not fight
    // someone who has scrolled deliberately.
    expect(box({ scrollTop: 0, caretBottom: 60 })).toBe(0);
  });

  it("scrolls down just enough, plus a line of slack", () => {
    // Caret on line 6 (bottom at 120) with 0..100 visible: 120 + 20 of padding - 100 of viewport.
    expect(box({ scrollTop: 0, caretBottom: 120 })).toBe(40);
  });

  it("scrolls up to a caret above the fold, again with slack", () => {
    // Caret on line 2 (top at 20) while scrolled to 100 → its line, less one line of padding.
    expect(box({ scrollTop: 100, caretBottom: 40 })).toBe(0);
    expect(box({ scrollTop: 100, caretBottom: 100 })).toBe(60);
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
