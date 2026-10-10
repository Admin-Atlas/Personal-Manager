// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every case names its zone, so none depends on the zone the tests run in.

import { describe, expect, it } from "vitest";

import { resolveWallTime, wallInstant, wallTimeOf } from "./wallTime";

describe("wallTime", () => {
  it("reads an instant in a named zone", () => {
    const at = new Date("2026-07-01T08:00:00Z");
    expect(wallTimeOf(at, "Europe/London")).toEqual({ date: "2026-07-01", time: "09:00" });
    expect(wallTimeOf(at, "America/New_York")).toEqual({ date: "2026-07-01", time: "04:00" });
    expect(wallTimeOf(at, "Asia/Kolkata")).toEqual({ date: "2026-07-01", time: "13:30" });
    // Across midnight, the date moves with the zone.
    expect(wallTimeOf(new Date("2026-07-01T23:30:00Z"), "Asia/Tokyo")).toEqual({
      date: "2026-07-02",
      time: "08:30",
    });
  });

  it("resolves an ordinary wall time to one instant", () => {
    expect(resolveWallTime("2026-07-01", "09:00", "Europe/London")).toEqual({
      kind: "ok",
      instant: new Date("2026-07-01T08:00:00Z"),
    });
    expect(wallInstant("2026-01-15", "09:00", "Europe/London")).toEqual(
      new Date("2026-01-15T09:00:00Z"),
    );
  });

  it("refuses a time the clocks skip", () => {
    // UK clocks go forward at 01:00 GMT on 29 March 2026: 01:00–01:59 never happens.
    expect(resolveWallTime("2026-03-29", "01:30", "Europe/London")).toEqual({ kind: "gap" });
    expect(wallInstant("2026-03-29", "01:30", "Europe/London")).toBeNull();
    // New York's skipped hour is 02:00–02:59 on 8 March 2026.
    expect(resolveWallTime("2026-03-08", "02:15", "America/New_York")).toEqual({ kind: "gap" });
  });

  it("gives both instants for a time the clocks pass twice, the first taken", () => {
    // UK clocks go back at 02:00 BST on 25 October 2026: 01:30 happens in BST, then again in GMT.
    expect(resolveWallTime("2026-10-25", "01:30", "Europe/London")).toEqual({
      kind: "ambiguous",
      instant: new Date("2026-10-25T00:30:00Z"),
      later: new Date("2026-10-25T01:30:00Z"),
    });
    expect(wallInstant("2026-10-25", "01:30", "Europe/London")).toEqual(
      new Date("2026-10-25T00:30:00Z"),
    );
    // Just outside the repeated hour, one instant again.
    expect(resolveWallTime("2026-10-25", "02:00", "Europe/London").kind).toBe("ok");
  });

  it("says what it couldn't read", () => {
    expect(resolveWallTime("2026-02-30", "09:00", "Europe/London")).toEqual({
      kind: "invalid",
      what: "date",
    });
    expect(resolveWallTime("2026-07-01", "24:00", "Europe/London")).toEqual({
      kind: "invalid",
      what: "time",
    });
    for (const zone of ["Mars/Olympus", ""]) {
      expect(resolveWallTime("2026-07-01", "09:00", zone)).toEqual({
        kind: "invalid",
        what: "zone",
      });
    }
  });
});
