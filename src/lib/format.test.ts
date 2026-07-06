// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { formatDate, formatDateLocal, formatDateTime, formatWhen } from "./format";

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
