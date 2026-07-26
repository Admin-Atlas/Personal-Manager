// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  formatDate,
  formatDateOnly,
  formatDateLocal,
  formatDateTime,
  formatSyncedShort,
  formatWhen,
} from "./format";

// Dates are round-tripped through a *local* Date so the assertions hold regardless of the runner's
// timezone: `new Date(y, m, d, 12).toISOString()` names one instant, and parsing it back lands on the
// same local calendar day (noon has ~12h of slack either side of a day boundary).
function localIso(y: number, m0: number, d: number): string {
  return new Date(y, m0, d, 12, 0, 0).toISOString();
}

describe("formatDate", () => {
  it("renders a past-year date as DD-MM-YYYY", () => {
    expect(formatDate(localIso(2024, 2, 5))).toBe("05-03-2024"); // month is 0-indexed: 2 = March
  });

  it("drops the year for a date in the current year", () => {
    const y = new Date().getFullYear();
    expect(formatDate(localIso(y, 5, 9))).toBe("09-06"); // 5 = June
  });

  it("zero-pads day and month", () => {
    expect(formatDate(localIso(2023, 0, 1))).toBe("01-01-2023");
  });

  it("returns an unparseable value unchanged", () => {
    expect(formatDate("not a date")).toBe("not a date");
    expect(formatDate("")).toBe("");
  });
});

describe("formatDateOnly", () => {
  it("parses a bare YYYY-MM-DD into the local calendar day (no UTC shift)", () => {
    // F-14: formatDate('2024-03-05') reads UTC midnight and lands a day early in UTC-negative zones;
    // formatDateOnly builds from the y/m/d fields, so it is stable in every timezone.
    expect(formatDateOnly("2024-03-05")).toBe("05-03-2024");
    expect(formatDateOnly("2023-12-31")).toBe("31-12-2023");
  });

  it("uses only the written date part of a full ISO timestamp", () => {
    expect(formatDateOnly("2024-03-05T23:30:00Z")).toBe("05-03-2024");
  });

  it("drops the year in the current year", () => {
    const y = new Date().getFullYear();
    expect(formatDateOnly(`${y}-06-09`)).toBe("09-06");
  });

  it("falls back for a non-date value", () => {
    expect(formatDateOnly("not a date")).toBe("not a date");
  });
});

describe("formatDateLocal", () => {
  it("formats a local Date's own calendar fields (no ISO round-trip)", () => {
    expect(formatDateLocal(new Date(2024, 2, 5))).toBe("05-03-2024");
  });

  it("returns empty string for an invalid Date", () => {
    expect(formatDateLocal(new Date("nope"))).toBe("");
  });
});

describe("formatDateTime", () => {
  it("is the date plus a HH:MM clock", () => {
    const rendered = formatDateTime(localIso(2024, 2, 5));
    expect(rendered.startsWith("05-03-2024 ")).toBe(true);
    expect(rendered).toMatch(/\d{2}:\d{2}(\s?[AP]M)?$/i);
  });

  it("returns an unparseable value unchanged", () => {
    expect(formatDateTime("garbage")).toBe("garbage");
  });
});

describe("formatWhen", () => {
  it("returns an unparseable value unchanged", () => {
    expect(formatWhen("garbage")).toBe("garbage");
  });
});

describe("formatSyncedShort", () => {
  // This drives the Refresh button's label, so the day boundary is the whole point: a bare clock
  // time for yesterday's sync would read as today, on the very control you press to fix that.
  const now = new Date(2026, 6, 26, 15, 0, 0); // 26 July 2026, 15:00 local

  it("shows a clock time for a sync that happened today", () => {
    expect(formatSyncedShort(new Date(2026, 6, 26, 9, 30).toISOString(), now)).toMatch(
      /^\d{2}:\d{2}(\s?[AP]M)?$/i,
    );
  });

  it("shows the date once the sync is not from today", () => {
    expect(formatSyncedShort(new Date(2026, 6, 25, 23, 59).toISOString(), now)).toBe("25-07");
  });

  it("is empty for an unparseable value", () => {
    expect(formatSyncedShort("garbage", now)).toBe("");
  });
});
