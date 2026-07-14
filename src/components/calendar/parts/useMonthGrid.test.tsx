// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { buildWeekRows, weekStartMonday } from "./useMonthGrid";
import { dayKey } from "../../../lib/calendar-layout";

describe("weekStartMonday", () => {
  it("returns the Monday of the week the date falls in", () => {
    // 2026-07-15 is a Wednesday → Monday is 2026-07-13.
    const mon = weekStartMonday(new Date(2026, 6, 15));
    expect(mon.getDay()).toBe(1);
    expect(dayKey(mon)).toBe(dayKey(new Date(2026, 6, 13)));
  });

  it("is idempotent on a Monday", () => {
    const mon = new Date(2026, 6, 13);
    expect(dayKey(weekStartMonday(mon))).toBe(dayKey(mon));
  });
});

describe("buildWeekRows (continuous stream)", () => {
  const start = weekStartMonday(new Date(2026, 6, 13)); // a Monday

  it("produces the requested number of Monday-first week rows of 7 consecutive days", () => {
    const weeks = buildWeekRows(start, 10, []);
    expect(weeks).toHaveLength(10);
    for (const w of weeks) {
      expect(w.cells).toHaveLength(7);
      expect(w.cells[0].date.getDay()).toBe(1); // Monday first
      for (let i = 1; i < 7; i++) {
        expect(w.cells[i].date.getTime() - w.cells[i - 1].date.getTime()).toBe(86_400_000);
      }
    }
    // Weeks are consecutive across the stream.
    expect(weeks[1].cells[0].date.getTime() - weeks[0].cells[0].date.getTime()).toBe(
      7 * 86_400_000,
    );
  });

  it("marks every day in-month when no focus month is given (the stream case)", () => {
    const weeks = buildWeekRows(start, 6, []);
    expect(weeks.every((w) => w.cells.every((c) => c.inMonth))).toBe(true);
  });

  it("marks only the focus month's days in-month (the single-grid case)", () => {
    const weeks = buildWeekRows(start, 6, [], { year: 2026, month: 6 }); // July
    for (const w of weeks) {
      for (const c of w.cells) {
        const isJuly = c.date.getFullYear() === 2026 && c.date.getMonth() === 6;
        expect(c.inMonth).toBe(isJuly);
      }
    }
  });
});
