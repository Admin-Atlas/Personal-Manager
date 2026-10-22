// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How far a sideways swipe moves the day grids (Settings > General). Pinned: the one-day default
// nobody's swipe changes from, what a stored value from a newer or broken build falls back to, and
// "All the days on screen" following whichever grid is swiped. Also the calendar a new event starts
// on (#884, C10), kept beside the hidden set in the encrypted store.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { getPref, setPref } from "./ipc";
import {
  readHidden,
  readLastCreate,
  readSwipeDays,
  swipeDays,
  writeHidden,
  writeLastCreate,
  writeSwipeDays,
} from "./calendarPrefs";
import { __resetStoredPrefs, hydrateStoredPrefs } from "./storedPrefs";

vi.mock("./ipc", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => undefined),
}));

beforeEach(() => {
  localStorage.clear();
  __resetStoredPrefs();
  vi.mocked(getPref).mockReset();
  vi.mocked(setPref).mockReset();
  vi.mocked(getPref).mockImplementation(async () => null);
  vi.mocked(setPref).mockImplementation(async () => undefined);
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

describe("the calendar a new event starts on", () => {
  it("last create round-trips beside hidden in calendar_ui", async () => {
    vi.mocked(getPref).mockImplementation(async (key: string) =>
      key === "calendar_ui" ? JSON.stringify({ hidden: ["gcal:me@x.com:work"] }) : null,
    );
    await hydrateStoredPrefs();
    expect(readLastCreate()).toBeNull();

    writeLastCreate("gcal:me@x.com:me@x.com");
    expect(readLastCreate()).toBe("gcal:me@x.com:me@x.com");
    // The hidden set is still there, in the same row: one blob, merged, never replaced.
    expect([...readHidden()]).toEqual(["gcal:me@x.com:work"]);
    expect(setPref).toHaveBeenLastCalledWith(
      "calendar_ui",
      JSON.stringify({ hidden: ["gcal:me@x.com:work"], last_create: "gcal:me@x.com:me@x.com" }),
    );

    // And the other way round: hiding a calendar keeps the last create.
    writeHidden(new Set(["gcal:me@x.com:work", "gcal:me@x.com:home"]));
    expect(readLastCreate()).toBe("gcal:me@x.com:me@x.com");
  });

  it("reads nothing it can't use as none", async () => {
    for (const bad of [42, "", null, ["gcal:me@x.com:me@x.com"]]) {
      __resetStoredPrefs();
      vi.mocked(getPref).mockImplementation(async (key: string) =>
        key === "calendar_ui" ? JSON.stringify({ last_create: bad }) : null,
      );
      await hydrateStoredPrefs();
      expect(readLastCreate()).toBeNull();
    }
  });
});
