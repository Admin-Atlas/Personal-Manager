// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { hourRowHeight, resolveRangeBounds, sunriseSunsetBounds } from "./calendarGeom";
import {
  DAY_FALLBACK,
  FULL_BOUNDS,
  WORK_DEFAULT,
  sanitizeBounds,
  type RangeBounds,
} from "./calendarPrefs";

const noCustom: Partial<Record<"work" | "day" | "full", RangeBounds>> = {};

describe("sanitizeBounds", () => {
  it("rounds to the nearest half hour", () => {
    expect(sanitizeBounds({ startHour: 8.4, endHour: 17.8 })).toEqual({
      startHour: 8.5,
      endHour: 18,
    });
  });

  it("clamps to [0,23.5] / [0.5,24]", () => {
    expect(sanitizeBounds({ startHour: -3, endHour: 40 })).toEqual({ startHour: 0, endHour: 24 });
  });

  it("rejects a window under an hour, and an inverted one", () => {
    expect(sanitizeBounds({ startHour: 9, endHour: 9.5 })).toBeNull();
    expect(sanitizeBounds({ startHour: 18, endHour: 9 })).toBeNull();
  });

  it("rejects non-finite input", () => {
    expect(sanitizeBounds({ startHour: "x", endHour: 10 })).toBeNull();
    expect(sanitizeBounds({ startHour: NaN, endHour: 10 })).toBeNull();
  });
});

describe("resolveRangeBounds", () => {
  const date = new Date(2026, 6, 13); // fixed local day

  it("uses the fixed defaults for work / 24h", () => {
    expect(resolveRangeBounds("work", noCustom, null, date)).toEqual(WORK_DEFAULT);
    expect(resolveRangeBounds("full", noCustom, null, date)).toEqual(FULL_BOUNDS);
  });

  it("a custom override wins over the default", () => {
    const custom = { work: { startHour: 6, endHour: 22 } };
    expect(resolveRangeBounds("work", custom, null, date)).toEqual({ startHour: 6, endHour: 22 });
  });

  it("Day falls back when no location is known", () => {
    expect(resolveRangeBounds("day", noCustom, null, date)).toEqual(DAY_FALLBACK);
  });

  it("Day always yields a valid, ordered window (whatever the runner timezone)", () => {
    const b = resolveRangeBounds("day", noCustom, [51.51, -0.13], date);
    expect(b.startHour).toBeGreaterThanOrEqual(0);
    expect(b.endHour).toBeLessThanOrEqual(24);
    expect(b.endHour - b.startHour).toBeGreaterThanOrEqual(1);
  });
});

describe("sunriseSunsetBounds", () => {
  it("returns null without coordinates", () => {
    expect(sunriseSunsetBounds(new Date(2026, 6, 13), null)).toBeNull();
  });

  it("returns null on a polar day (sun never sets)", () => {
    // ~80°N at the June solstice → the sun stays up all day.
    expect(sunriseSunsetBounds(new Date(2026, 5, 21), [80, 0])).toBeNull();
  });

  it("produces whole-hour, ordered bounds when the sun rises and sets", () => {
    const b = sunriseSunsetBounds(new Date(2026, 6, 13), [51.51, -0.13]);
    if (b) {
      expect(Number.isInteger(b.startHour)).toBe(true);
      expect(Number.isInteger(b.endHour)).toBe(true);
      expect(b.endHour).toBeGreaterThan(b.startHour);
    }
  });
});

describe("hourRowHeight", () => {
  it("stretches the framed window to fill the pane exactly", () => {
    // The point of the Work/Day/24h choice: a narrower window means taller rows.
    expect(hourRowHeight(360, 9, 20)).toBe(40);
    expect(hourRowHeight(360, 12, 20)).toBe(30);
  });

  it("never returns a row thinner than the floor", () => {
    expect(hourRowHeight(240, 24, 20)).toBe(20); // 10px would fit; the grid scrolls instead
  });

  it("keeps two wide windows apart in a short pane once the floor is lowered", () => {
    // The bug this fixes. In a ~26rem card the calendar's 20px floor swallows the difference
    // between a 17h daylight window and a 24h one — both bottom out, so the toggle looks like it
    // only re-aims the scroll. A floor the embed can actually reach keeps them distinct.
    const shortPane = 340;
    expect(hourRowHeight(shortPane, 17, 20)).toBe(hourRowHeight(shortPane, 24, 20));
    expect(hourRowHeight(shortPane, 17, 12)).toBeGreaterThan(hourRowHeight(shortPane, 24, 12));
  });

  it("falls back to a legible height before the pane has been measured", () => {
    expect(hourRowHeight(0, 9, 20)).toBe(40);
    expect(hourRowHeight(Number.NaN, 9, 12)).toBe(24);
  });

  it("never divides by a degenerate window", () => {
    expect(Number.isFinite(hourRowHeight(360, 0, 20))).toBe(true);
    expect(hourRowHeight(360, 0, 20)).toBe(360);
  });
});
