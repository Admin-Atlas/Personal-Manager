// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  dateToIso,
  isRealDate,
  isoToDate,
  isoToDisplay,
  parseDisplay,
  todayIso,
  toIso,
} from "./dateField";

// A fixed "now" so the year-inference cases don't drift with the wall clock.
const NOW = new Date(2026, 6, 26); // 26 July 2026

describe("isRealDate", () => {
  it("accepts a real day", () => {
    expect(isRealDate(2026, 8, 14)).toBe(true);
  });

  it("rejects a day the month does not have", () => {
    expect(isRealDate(2026, 2, 30)).toBe(false);
    expect(isRealDate(2026, 4, 31)).toBe(false);
  });

  it("follows the leap year", () => {
    expect(isRealDate(2024, 2, 29)).toBe(true);
    expect(isRealDate(2026, 2, 29)).toBe(false);
  });

  it("rejects out-of-range components", () => {
    expect(isRealDate(2026, 13, 1)).toBe(false);
    expect(isRealDate(2026, 0, 1)).toBe(false);
    expect(isRealDate(2026, 1, 0)).toBe(false);
  });
});

describe("isoToDisplay", () => {
  it("renders full DD-MM-YYYY", () => {
    expect(isoToDisplay("2026-08-14")).toBe("14-08-2026");
  });

  it("keeps the year even in the current year — the field is edited, not just read", () => {
    const y = new Date().getFullYear();
    expect(isoToDisplay(`${y}-03-05`)).toBe(`05-03-${y}`);
  });

  it("is empty for no date and for junk", () => {
    expect(isoToDisplay("")).toBe("");
    expect(isoToDisplay("not-a-date")).toBe("");
    expect(isoToDisplay("2026-02-30")).toBe("");
  });
});

describe("parseDisplay", () => {
  it("round-trips its own display format", () => {
    expect(parseDisplay("14-08-2026", NOW)).toBe("2026-08-14");
    expect(isoToDisplay(parseDisplay("14-08-2026", NOW)!)).toBe("14-08-2026");
  });

  it("takes / and . as separators too", () => {
    expect(parseDisplay("14/08/2026", NOW)).toBe("2026-08-14");
    expect(parseDisplay("14.08.2026", NOW)).toBe("2026-08-14");
  });

  it("accepts one-digit day and month", () => {
    expect(parseDisplay("4/8/2026", NOW)).toBe("2026-08-04");
  });

  it("infers the current year when it is left off", () => {
    expect(parseDisplay("14-08", NOW)).toBe("2026-08-14");
  });

  it("expands a two-digit year into the nearest century", () => {
    expect(parseDisplay("14-08-26", NOW)).toBe("2026-08-14");
    expect(parseDisplay("14-08-99", NOW)).toBe("1999-08-14");
  });

  it("accepts a pasted ISO date unchanged — that is what PM stores and copies out", () => {
    expect(parseDisplay("2026-08-14", NOW)).toBe("2026-08-14");
  });

  it("returns '' for an empty field: clearing a deadline is a valid edit, not an error", () => {
    expect(parseDisplay("", NOW)).toBe("");
    expect(parseDisplay("   ", NOW)).toBe("");
  });

  it("returns null — distinct from '' — for text that is not a date", () => {
    expect(parseDisplay("tomorrow", NOW)).toBeNull();
    expect(parseDisplay("14", NOW)).toBeNull();
    expect(parseDisplay("32-01-2026", NOW)).toBeNull();
    expect(parseDisplay("14-13-2026", NOW)).toBeNull();
  });

  it("rejects a mixed separator rather than half-understanding it", () => {
    expect(parseDisplay("14-08/2026", NOW)).toBeNull();
  });

  it("rejects a day the month does not have", () => {
    expect(parseDisplay("30-02-2026", NOW)).toBeNull();
  });
});

describe("isoToDate / dateToIso", () => {
  it("round-trips through a LOCAL date, so the day cannot shift west of Greenwich", () => {
    const d = isoToDate("2026-01-01");
    expect(d).not.toBeNull();
    expect(d!.getFullYear()).toBe(2026);
    expect(d!.getMonth()).toBe(0);
    expect(d!.getDate()).toBe(1);
    expect(dateToIso(d!)).toBe("2026-01-01");
  });

  it("is null for an unparseable or impossible value", () => {
    expect(isoToDate("")).toBeNull();
    expect(isoToDate("14-08-2026")).toBeNull();
    expect(isoToDate("2026-02-30")).toBeNull();
  });
});

describe("todayIso", () => {
  it("names the injected day in local terms", () => {
    expect(todayIso(NOW)).toBe("2026-07-26");
  });

  it("uses the local calendar day, not the UTC one", () => {
    // 23:30 local on the 31st is already the 1st in UTC east of Greenwich, and still the 31st west
    // of it — either way `toISOString().slice(0,10)` would be wrong for someone.
    const lateNight = new Date(2026, 11, 31, 23, 30);
    expect(todayIso(lateNight)).toBe("2026-12-31");
  });
});

describe("toIso", () => {
  it("pads every component", () => {
    expect(toIso(2026, 1, 2)).toBe("2026-01-02");
  });
});
