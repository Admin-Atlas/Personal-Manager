// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";

import { hoursToHM, isHM, QUARTER_HOURS, slots, timeChoices } from "./timeSlots";

describe("time slots", () => {
  // Pinned as RangeControl had them before the lift: its Work/Day editor reads these.
  it("keeps RangeControl's hours and half-hour slots", () => {
    expect(hoursToHM(8.5)).toBe("08:30");
    expect(hoursToHM(0)).toBe("00:00");
    expect(hoursToHM(24)).toBe("24:00");
    expect(slots(0, 2)).toEqual([0, 0.5, 1, 1.5, 2]);
    expect(slots(0.5, 24)).toHaveLength(48);
  });

  it("offers the day in quarter hours", () => {
    expect(QUARTER_HOURS).toHaveLength(96);
    expect(QUARTER_HOURS.slice(0, 3)).toEqual(["00:00", "00:15", "00:30"]);
    expect(QUARTER_HOURS[QUARTER_HOURS.length - 1]).toBe("23:45");
  });

  it("slots an off-grid time in where it sorts, and nothing else", () => {
    expect(timeChoices("09:15")).toEqual(QUARTER_HOURS);
    const off = timeChoices("09:10");
    expect(off).toHaveLength(97);
    expect(off.indexOf("09:10")).toBe(off.indexOf("09:00") + 1);
    // Garbage is never offered as a choice.
    expect(timeChoices("9:10")).toEqual(QUARTER_HOURS);
    expect(timeChoices("24:00")).toEqual(QUARTER_HOURS);
  });

  it("keeps the held time on offer whatever the value is now", () => {
    const after = timeChoices("09:15", "09:10");
    expect(after).toContain("09:10");
    expect(after).toHaveLength(97);
    // Both off-grid, once each.
    expect(timeChoices("09:20", "09:10")).toHaveLength(98);
    expect(timeChoices("09:10", "09:10")).toHaveLength(97);
  });

  it("knows a 24h time of day", () => {
    expect(isHM("00:00")).toBe(true);
    expect(isHM("23:59")).toBe(true);
    expect(isHM("24:00")).toBe(false);
    expect(isHM("9:00")).toBe(false);
  });
});
