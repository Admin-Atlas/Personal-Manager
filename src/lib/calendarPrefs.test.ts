// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How far a sideways swipe moves the day grids (Settings > General). Pinned: the one-day default
// nobody's swipe changes from, what a stored value from a newer or broken build falls back to, and
// "All the days on screen" following whichever grid is swiped.

import { describe, it, expect, beforeEach } from "vitest";
import { readSwipeDays, swipeDays, writeSwipeDays } from "./calendarPrefs";

beforeEach(() => {
  localStorage.clear();
});

describe("how far a swipe moves", () => {
  it("moves one day until Settings says otherwise", () => {
    expect(readSwipeDays()).toBe(1);
    expect(swipeDays(7)).toBe(1);
  });

  it("remembers a number of days, and all the days on screen", () => {
    writeSwipeDays(3);
    expect(readSwipeDays()).toBe(3);
    expect(swipeDays(7)).toBe(3);
    writeSwipeDays("view");
    expect(readSwipeDays()).toBe("view");
  });

  it("moves a whole page for 'all the days on screen', whatever the grid shows", () => {
    expect(swipeDays(7, "view")).toBe(7); // Week
    expect(swipeDays(3, "view")).toBe(3); // a 3-day Day view, or Upcoming's 3 days
    expect(swipeDays(1, "view")).toBe(1);
    // Never a swipe that moves nothing, even from a caller with no days on screen.
    expect(swipeDays(0, "view")).toBe(1);
  });

  it("falls back to one day for a value it doesn't know", () => {
    for (const bad of ["0", "8", "2.5", "-1", "week", ""]) {
      localStorage.setItem("pm.calendar.swipeDays", bad);
      expect(readSwipeDays()).toBe(1);
    }
  });
});
