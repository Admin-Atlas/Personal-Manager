// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Upcoming grid's stored prefs. Only the parts with a decision in them are worth pinning: the
// day-count clamp, and what happens to a "full" (24h) range stored by a build that still offered it.

import { describe, it, expect, beforeEach } from "vitest";
import {
  FOCUS_UPCOMING_DAY_CHOICES,
  FOCUS_UPCOMING_RANGES,
  clampFocusUpcomingDays,
  readFocusUpcomingDays,
  readFocusUpcomingRange,
  writeFocusUpcomingDays,
} from "./focusPrefs";

beforeEach(() => {
  localStorage.clear();
});

describe("the Upcoming hour window", () => {
  it("no longer offers 24h in this pane", () => {
    expect(FOCUS_UPCOMING_RANGES.map((r) => r.value)).toEqual(["work", "day"]);
  });

  it("lands a stored 24h choice on Day rather than on nothing", () => {
    // Anyone who picked 24h before it was withdrawn would otherwise open a control with no segment
    // selected — and a grid framed by a range the control can't show.
    localStorage.setItem("pm.focus.upcoming.range", "full");
    expect(readFocusUpcomingRange()).toBe("day");
  });

  it("keeps a stored choice that is still offered, and defaults to Day", () => {
    localStorage.setItem("pm.focus.upcoming.range", "work");
    expect(readFocusUpcomingRange()).toBe("work");
    localStorage.clear();
    expect(readFocusUpcomingRange()).toBe("day");
  });
});

describe("the Upcoming day count", () => {
  it("offers exactly the counts the clamp accepts", () => {
    expect(FOCUS_UPCOMING_DAY_CHOICES).toEqual([1, 2, 3, 4]);
    for (const n of FOCUS_UPCOMING_DAY_CHOICES) {
      expect(clampFocusUpcomingDays(n)).toBe(n);
    }
  });

  it("clamps anything outside the window", () => {
    expect(clampFocusUpcomingDays(0)).toBe(1);
    expect(clampFocusUpcomingDays(-4)).toBe(1);
    expect(clampFocusUpcomingDays(9)).toBe(4);
    expect(clampFocusUpcomingDays(2.4)).toBe(2);
  });

  it("round-trips through storage, defaulting to 3", () => {
    expect(readFocusUpcomingDays()).toBe(3);
    writeFocusUpcomingDays(2);
    expect(readFocusUpcomingDays()).toBe(2);
    // An out-of-range value is clamped on the way in, so nothing unreadable can be stored.
    writeFocusUpcomingDays(99);
    expect(readFocusUpcomingDays()).toBe(4);
  });
});
