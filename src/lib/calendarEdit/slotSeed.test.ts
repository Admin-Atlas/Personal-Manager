// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A slot on the grid as a new event's fields (#884, C17): the slot's own times in this computer's
// zone, Busy for a timed one and Free for an all-day one, and everything else a new event's.

import { describe, expect, it } from "vitest";
import { DEFAULT_LENGTH_MS, newEventSeed } from "./eventDraft";
import { slotSeed } from "./slotSeed";

const NOW = new Date(2026, 9, 12, 8, 20);
const ZONE = "Europe/London";

describe("slotSeed", () => {
  it("a timed slot keeps its times in the device zone and is Busy", () => {
    const seed = slotSeed(
      {
        kind: "timed",
        start: { day: "2026-10-14", min: 555 },
        end: { day: "2026-10-14", min: 615 },
      },
      NOW,
      ZONE,
    );
    expect(seed.time).toEqual({
      kind: "timed",
      start_date: "2026-10-14",
      start_time: "09:15",
      start_zone: ZONE,
      end_date: "2026-10-14",
      end_time: "10:15",
      end_zone: ZONE,
    });
    expect(seed.show_as).toBe("busy");
  });

  it("a slot that runs past midnight ends on the next day", () => {
    const seed = slotSeed(
      {
        kind: "timed",
        start: { day: "2026-10-14", min: 1395 },
        end: { day: "2026-10-15", min: 15 },
      },
      NOW,
      ZONE,
    );
    expect(seed.time).toMatchObject({
      start_date: "2026-10-14",
      start_time: "23:15",
      end_date: "2026-10-15",
      end_time: "00:15",
    });
  });

  it("an all-day slot is Free, over its days", () => {
    const seed = slotSeed({ kind: "all_day", first: "2026-10-14", last: "2026-10-16" }, NOW, ZONE);
    expect(seed.time).toEqual({ kind: "all_day", first_day: "2026-10-14", last_day: "2026-10-16" });
    expect(seed.show_as).toBe("free");
  });

  it("everything else is a new event's", () => {
    const seed = slotSeed({ kind: "all_day", first: "2026-10-14", last: "2026-10-14" }, NOW, ZONE);
    const fresh = newEventSeed("2026-10-14", NOW, ZONE, DEFAULT_LENGTH_MS);
    expect({ ...seed, time: fresh.time, show_as: fresh.show_as }).toEqual(fresh);
    expect(seed.summary).toBe("");
  });
});
