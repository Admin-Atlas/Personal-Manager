// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The two reduced-motion signals, and the precedence between them. Pure DOM — no mocks — because the
// helper deliberately reads the `data-reduced-motion` stamp rather than React context, so there is
// no provider to stand up. jsdom's `matchMedia` is absent by default, which is itself one of the
// cases under test (the helper must not throw when the query is unavailable).

import { afterEach, describe, expect, it, vi } from "vitest";

import { prefersReducedMotion, scrollBehavior } from "./motion";

/** Stand in for an OS preference. jsdom ships no `matchMedia`, so absence is the default state. */
function osPrefers(reduce: boolean | null) {
  if (reduce === null) {
    // @ts-expect-error deleting an optional global for the "query unavailable" case
    delete window.matchMedia;
    return;
  }
  vi.stubGlobal(
    "matchMedia",
    vi.fn((q: string) => ({
      matches: reduce && q.includes("prefers-reduced-motion: reduce"),
      media: q,
      addEventListener: () => {},
      removeEventListener: () => {},
    })),
  );
}

afterEach(() => {
  delete document.documentElement.dataset.reducedMotion;
  vi.unstubAllGlobals();
});

describe("prefersReducedMotion", () => {
  it("is true under the in-app stamp", () => {
    osPrefers(false);
    document.documentElement.dataset.reducedMotion = "on";
    expect(prefersReducedMotion()).toBe(true);
  });

  it("is true under the OS preference alone, with the app setting untouched", () => {
    // The signal PM could never see before: an OS-level preference on Windows/WebView2, where the
    // engine does not suppress programmatic smooth scrolling for us.
    osPrefers(true);
    expect(document.documentElement.dataset.reducedMotion).toBeUndefined();
    expect(prefersReducedMotion()).toBe(true);
  });

  it("is false when neither signal asks for it", () => {
    osPrefers(false);
    expect(prefersReducedMotion()).toBe(false);
  });

  it("does not throw when matchMedia is unavailable", () => {
    osPrefers(null);
    expect(() => prefersReducedMotion()).not.toThrow();
    expect(prefersReducedMotion()).toBe(false);
  });

  it("reads the stamp LIVE, not once at module load", () => {
    // The whole point of a plain function over a cached constant: a mid-session toggle must land
    // without a reload, and every call site reads at scroll time.
    osPrefers(false);
    expect(prefersReducedMotion()).toBe(false);
    document.documentElement.dataset.reducedMotion = "on";
    expect(prefersReducedMotion()).toBe(true);
    delete document.documentElement.dataset.reducedMotion;
    expect(prefersReducedMotion()).toBe(false);
  });

  it('ignores a stamp that is not exactly "on"', () => {
    osPrefers(false);
    document.documentElement.dataset.reducedMotion = "off";
    expect(prefersReducedMotion()).toBe(false);
  });
});

describe("scrollBehavior", () => {
  it('is "smooth" only when nothing asks for reduced motion', () => {
    osPrefers(false);
    expect(scrollBehavior()).toBe("smooth");
  });

  it('is "auto" under either signal', () => {
    osPrefers(false);
    document.documentElement.dataset.reducedMotion = "on";
    expect(scrollBehavior()).toBe("auto");

    delete document.documentElement.dataset.reducedMotion;
    osPrefers(true);
    expect(scrollBehavior()).toBe("auto");
  });

  it("honours an explicit false regardless — a caller's own instant-jump reason always wins", () => {
    osPrefers(false);
    expect(scrollBehavior(false)).toBe("auto");
    document.documentElement.dataset.reducedMotion = "on";
    expect(scrollBehavior(false)).toBe("auto");
  });

  it("passes a caller's true through to the preference check (the calendar's cursor-move flag)", () => {
    // MonthView/YearView pass "is this a cursor move or a first paint?" — orthogonal to motion prefs,
    // so a true must still be vetoed by the preference and a false must never become smooth.
    osPrefers(false);
    expect(scrollBehavior(true)).toBe("smooth");
    document.documentElement.dataset.reducedMotion = "on";
    expect(scrollBehavior(true)).toBe("auto");
  });
});
