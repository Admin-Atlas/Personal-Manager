// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pop-up's and the delete dialog's "when": both dates for a timed event that runs into another
// day, and calendar days (not 24-hour steps) for all-day ranges. Timed values without a zone read as
// local time, and dates are formatted with the app's own helpers, so the cases hold in any zone and
// year. The file runs in Europe/London all the same, so the night the clocks go back is a 25-hour
// day wherever the tests run (CI is UTC); Node reads TZ again whenever it's set.

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { formatClock, formatDateLocal } from "../../../lib/format";
import { whenText } from "./whenText";

const ev = (start: string, end: string | null, all_day = false): CalendarEvent => ({
  id: "e",
  calendar_id: "c",
  summary: "Night shift",
  description: null,
  location: null,
  start,
  end,
  all_day,
  html_link: null,
  uid: null,
});

const day = (m0: number, d: number) => formatDateLocal(new Date(2026, m0, d));
const clock = (h: number, min = 0) => formatClock(new Date(2026, 0, 1, h, min));

beforeAll(() => {
  vi.stubEnv("TZ", "Europe/London");
});
afterAll(() => {
  vi.unstubAllEnvs();
});

describe("whenText", () => {
  it("says one date for a timed event within its day", () => {
    expect(whenText(ev("2026-10-12T09:00:00", "2026-10-12T10:30:00"))).toBe(
      `${day(9, 12)} · ${clock(9)}–${clock(10, 30)}`,
    );
  });

  it("says both dates for a timed event that runs into the next day", () => {
    expect(whenText(ev("2026-10-12T22:00:00", "2026-10-13T02:00:00"))).toBe(
      `${day(9, 12)} ${clock(22)} – ${day(9, 13)} ${clock(2)}`,
    );
  });

  it("keeps one date for an event that ends at midnight", () => {
    expect(whenText(ev("2026-10-12T20:00:00", "2026-10-13T00:00:00"))).toBe(
      `${day(9, 12)} · ${clock(20)}–${clock(0)}`,
    );
  });

  it("gives an all-day range by its first and last days", () => {
    expect(whenText(ev("2026-10-12", "2026-10-15", true))).toBe(
      `All day · ${day(9, 12)} – ${day(9, 14)}`,
    );
    expect(whenText(ev("2026-10-12", "2026-10-13", true))).toBe(`All day · ${day(9, 12)}`);
  });

  // 25-10-2026 is 25 hours long in Europe/London: stepping back 24 hours from its end used to land
  // on the same day and print "25-10 – 25-10".
  it("says one day for a one-day all-day event on the night the clocks go back", () => {
    // The zone took: otherwise this case would pass without testing anything.
    expect(new Date(2026, 9, 26).getTime() - new Date(2026, 9, 25).getTime()).toBe(25 * 3_600_000);
    expect(whenText(ev("2026-10-25", "2026-10-26", true))).toBe(`All day · ${day(9, 25)}`);
  });
});
