// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which stretch of the mirror a calendar surface reads (#1047): the months around what it shows,
// moved only near an edge; the months after the live band it still has to load; and what the view
// says while they, or a calendar's history, aren't in.

import { describe, expect, it } from "vitest";
import {
  beyondLiveBand,
  laterToLoad,
  nextWindow,
  rangeHintText,
  windowAround,
  windowRead,
} from "./calendarWindow";

const local = (y: number, m: number, d = 1) => new Date(y, m - 1, d);

describe("windowAround and nextWindow", () => {
  it("covers whole local months around a day", () => {
    const w = windowAround(local(2026, 11, 5), 2, 3);
    expect(w.start).toEqual(local(2026, 9));
    expect(w.end).toEqual(local(2027, 3));
  });

  it("keeps the window while what's on show stays inside its margin, and moves it near an edge", () => {
    const w = windowAround(local(2026, 11, 5), 18, 18);
    const day = (y: number, m: number, d: number) => ({
      start: local(y, m, d),
      end: local(y, m, d + 1),
    });
    expect(nextWindow(w, day(2027, 6, 30), local(2027, 6, 30), 18, 18, 6)).toBe(w);
    expect(nextWindow(w, day(2026, 1, 1), local(2026, 1, 1), 18, 18, 6)).toBe(w);
    const later = nextWindow(w, day(2027, 12, 1), local(2027, 12, 1), 18, 18, 6);
    expect(later).not.toBe(w);
    expect(later.start).toEqual(local(2026, 6));
    // The window runs May 2025 to May 2028: November 2025 is still six months in; October isn't.
    expect(nextWindow(w, day(2025, 11, 30), local(2025, 11, 30), 18, 18, 6)).toBe(w);
    const earlier = nextWindow(w, day(2025, 10, 15), local(2025, 10, 15), 18, 18, 6);
    expect(earlier.end).toEqual(local(2027, 5));
  });

  // Review of #1047: a Year view shows months up to eleven away from its cursor, so the window is
  // measured against the whole year on show; the cursor alone left January to April out.
  it("moves for a year on show whose cursor is still inside the margin", () => {
    const w = windowAround(local(2026, 11, 5), 18, 18);
    const year2025 = { start: local(2025, 1), end: local(2026, 1) };
    const moved = nextWindow(w, year2025, local(2025, 11, 1), 18, 18, 6);
    expect(moved).not.toBe(w);
    expect(moved.start.getTime()).toBeLessThanOrEqual(year2025.start.getTime());
    expect(moved.end.getTime()).toBeGreaterThanOrEqual(year2025.end.getTime());
  });

  it("reads a day of slack either side", () => {
    const w = windowAround(local(2026, 11, 5), 0, 0);
    const { from, to } = windowRead(w);
    expect(new Date(from).getTime()).toBe(local(2026, 11).getTime() - 86_400_000);
    expect(new Date(to).getTime()).toBe(local(2026, 12).getTime() + 86_400_000);
  });
});

describe("laterToLoad", () => {
  const bandEnd = "2027-12-01T00:00:00Z";

  it("asks for nothing while the window is inside the live band", () => {
    const w = { start: local(2027, 3), end: local(2027, 6) };
    expect(laterToLoad(w, bandEnd, new Set())).toBeNull();
  });

  it("asks for the months past the band as one stretch, and only those not yet loaded", () => {
    const w = { start: local(2027, 11), end: local(2028, 3) };
    const need = laterToLoad(w, bandEnd, new Set());
    expect(need?.from).toBe("2027-12-01T00:00:00.000Z");
    expect(need?.months[0]).toBe("2027-12");
    expect(need?.months).toContain("2028-02");
    const loaded = new Set(need?.months ?? []);
    expect(laterToLoad(w, bandEnd, loaded)).toBeNull();
    // One more month on: only that one.
    const next = laterToLoad({ start: local(2028, 1), end: local(2028, 6) }, bandEnd, loaded);
    expect(next?.months[0]).toBe(next?.from.slice(0, 7));
    expect(next?.months.every((m) => !loaded.has(m))).toBe(true);
  });

  it("asks for nothing it can't read", () => {
    expect(laterToLoad({ start: local(2030, 1), end: local(2030, 2) }, "", new Set())).toBeNull();
  });
});

describe("beyondLiveBand and the hint", () => {
  const overview = {
    mirror_start: "2026-10-01T00:00:00Z",
    mirror_end: "2027-12-01T00:00:00Z",
    history_ready: true,
  };

  it("compares local days, so a month inside the band never reads as outside it", () => {
    expect(
      beyondLiveBand(local(2026, 10), local(2026, 11), overview.mirror_start, overview.mirror_end),
    ).toEqual({
      before: false,
      after: false,
    });
    expect(
      beyondLiveBand(local(2026, 9), local(2026, 10), overview.mirror_start, overview.mirror_end)
        .before,
    ).toBe(true);
    expect(
      beyondLiveBand(local(2027, 12), local(2028, 1), overview.mirror_start, overview.mirror_end)
        .after,
    ).toBe(true);
  });

  it("says the later months are loading or couldn't be, and history while it downloads", () => {
    const later = { start: local(2028, 1), end: local(2028, 2) };
    const before = { start: local(2025, 1), end: local(2025, 2) };
    const inside = { start: local(2027, 1), end: local(2027, 2) };
    expect(rangeHintText(overview, later, "loading")).toMatch(/^Loading these months/);
    expect(rangeHintText(overview, later, "failed")).toMatch(/^Couldn't load these months/);
    expect(rangeHintText(overview, later, "idle")).toBeNull();
    expect(rangeHintText(overview, before, "idle")).toBeNull();
    expect(rangeHintText({ ...overview, history_ready: false }, before, "idle")).toMatch(
      /older events are still downloading/,
    );
    expect(rangeHintText({ ...overview, history_ready: false }, inside, "loading")).toBeNull();
  });
});
