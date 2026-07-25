// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Upcoming grid's stored prefs. Only the parts with a decision in them are worth pinning: the
// day-count clamp, what happens to a "full" (24h) range stored by a build that still offered it,
// this pane's OWN hour windows (kept apart from the Calendar tab's), and the reset that Settings
// still owns now that the controls themselves live on the Focus tab.

import { describe, it, expect, beforeEach } from "vitest";
import {
  FOCUS_UPCOMING_DAY_CHOICES,
  FOCUS_UPCOMING_RANGES,
  clampFocusUpcomingDays,
  focusViewPrefsAreDefault,
  readFocusUpcomingBounds,
  readFocusUpcomingDays,
  readFocusUpcomingRange,
  resetFocusViewPrefs,
  writeFocusLayout,
  writeFocusUpcomingBounds,
  writeFocusUpcomingDays,
} from "./focusPrefs";
import { readRangeBounds, writeRangeBounds } from "./calendarPrefs";

beforeEach(() => {
  localStorage.clear();
});

describe("the Upcoming hour window", () => {
  it("no longer offers 24h in this pane", () => {
    expect(FOCUS_UPCOMING_RANGES).toEqual(["work", "day"]);
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

describe("the Upcoming pane's own hour windows", () => {
  it("does not share a store with the Calendar tab, in either direction", () => {
    // This pane is ~26rem tall, so a Work window that suits it is not the one that suits a
    // full-page week grid. Narrowing one must leave the other exactly where it was.
    writeRangeBounds({ work: { startHour: 8.5, endHour: 17.5 } });
    writeFocusUpcomingBounds({ work: { startHour: 10, endHour: 14 } });

    expect(readRangeBounds().work).toEqual({ startHour: 8.5, endHour: 17.5 });
    expect(readFocusUpcomingBounds().work).toEqual({ startHour: 10, endHour: 14 });
  });

  it("validates a stored window, dropping one that is unusable", () => {
    // Same validator as the Calendar tab's — an inverted / sub-1h window is no window at all.
    writeFocusUpcomingBounds({ work: { startHour: 18, endHour: 9 } });
    expect(readFocusUpcomingBounds().work).toBeUndefined();
    // …and unparseable storage reads as "no custom window", never as a throw.
    localStorage.setItem("pm.focus.upcoming.bounds", "{not json");
    expect(readFocusUpcomingBounds()).toEqual({});
  });
});

describe("the Focus view prefs as a whole (Settings' Reset Focus)", () => {
  it("reports default until something moves, and reset puts every part back", () => {
    expect(focusViewPrefsAreDefault()).toBe(true);

    // Each of the four keys on its own is enough to make the tab non-default, so the Reset link
    // appears for any of them — not just the one Settings used to show a control for.
    writeFocusLayout("vertical");
    expect(focusViewPrefsAreDefault()).toBe(false);
    resetFocusViewPrefs();
    expect(focusViewPrefsAreDefault()).toBe(true);

    writeFocusUpcomingDays(1);
    expect(focusViewPrefsAreDefault()).toBe(false);
    resetFocusViewPrefs();
    expect(focusViewPrefsAreDefault()).toBe(true);

    writeFocusUpcomingBounds({ day: { startHour: 6, endHour: 22 } });
    expect(focusViewPrefsAreDefault()).toBe(false);
    resetFocusViewPrefs();
    expect(focusViewPrefsAreDefault()).toBe(true);
    expect(readFocusUpcomingBounds()).toEqual({});
  });
});
