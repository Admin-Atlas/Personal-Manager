// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { buildWeekRows, chipsShown, MONTH_MAX_BAND_LANES, weekStartMonday } from "./useMonthGrid";
import { dayKey, DRAFT_EVENT_ID } from "../../../lib/calendar-layout";

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

  // A timed event across midnight is still one bar here (Month keeps bars; only Day/Week fill the
  // hours). A bar cut off by the grid's own edge must say it continues, or it reads as the event's
  // start (and Month prints a timed bar's start time there).
  const timed = (startIso: string, endIso: string) => ({
    id: "e",
    calendar_id: "c",
    summary: "Night shift",
    description: null,
    location: null,
    start: startIso,
    end: endIso,
    all_day: false,
    html_link: null,
    uid: null,
  });

  it("keeps a timed event across midnight as a bar, continuing past the grid's edges", () => {
    // November 2026's grid starts on Mon 26-10; the event began the night before.
    const gridStart = new Date(2026, 9, 26);
    const before = buildWeekRows(gridStart, 6, [
      timed("2026-10-25T22:00:00", "2026-10-26T02:00:00"),
    ]);
    expect(before[0].bands).toHaveLength(1);
    expect(before[0].bands[0].continuesLeft).toBe(true);
    // And one that runs past the grid's last day (Sun 6-12).
    const after = buildWeekRows(gridStart, 6, [
      timed("2026-12-06T22:00:00", "2026-12-07T02:00:00"),
    ]);
    expect(after[5].bands[0].continuesRight).toBe(true);
    // Inside the grid, the ends are its own.
    const inside = buildWeekRows(gridStart, 6, [
      timed("2026-11-03T22:00:00", "2026-11-04T02:00:00"),
    ]);
    const bar = inside.flatMap((w) => w.bands)[0];
    expect([bar.continuesLeft, bar.continuesRight]).toEqual([false, false]);
  });

  // The new event's ghost (#884, C10c) is never folded into "+N more" as it's typed: past the lane
  // cap it would only add to the count. It takes the top lane instead.
  it("gives the new event's ghost band the top lane, under three bands already there", () => {
    const span = (id: string, start: string, end: string) => ({
      ...timed(start, end),
      id,
      all_day: true,
    });
    const weeks = buildWeekRows(start, 1, [
      span("a", "2026-07-13", "2026-07-18"),
      span("b", "2026-07-13", "2026-07-18"),
      span("c", "2026-07-13", "2026-07-18"),
      span(DRAFT_EVENT_ID, "2026-07-15", "2026-07-17"),
    ]);
    const lanes = new Map(weeks[0].bands.map((b) => [b.ev.id, b.lane]));
    expect(lanes.get(DRAFT_EVENT_ID)).toBe(0);
    expect(weeks[0].bands).toHaveLength(MONTH_MAX_BAND_LANES);
    // The band it put out of sight is counted instead.
    expect(weeks[0].cells[0].hiddenBands).toBe(1);
  });

  // The stream runs for months: the ghost's top lane is its own weeks', so a week it never touches
  // keeps its bands where they were, none folded away and no row added.
  it("leaves a week the ghost band doesn't touch as it was", () => {
    const span = (id: string, start: string, end: string) => ({
      ...timed(start, end),
      id,
      all_day: true,
    });
    const before = [
      span("a", "2026-07-13", "2026-07-18"),
      span("b", "2026-07-13", "2026-07-18"),
      span("c", "2026-07-13", "2026-07-18"),
    ];
    const plain = buildWeekRows(start, 3, before);
    // Two weeks on, timed across midnight: a band, as a start moved to 23:30 makes it.
    const weeks = buildWeekRows(start, 3, [
      ...before,
      { ...timed("2026-07-28T23:30:00", "2026-07-29T00:30:00"), id: DRAFT_EVENT_ID },
    ]);
    const lanes = new Map(weeks[0].bands.map((b) => [b.ev.id, b.lane]));
    expect([lanes.get("a"), lanes.get("b"), lanes.get("c")]).toEqual([0, 1, 2]);
    expect(weeks[0].cells.map((c) => c.hiddenBands)).toEqual([0, 0, 0, 0, 0, 0, 0]);
    expect(weeks[0].laneCount).toBe(plain[0].laneCount);
    // And the ghost has the top lane in its own week.
    expect(weeks[2].bands.map((b) => [b.ev.id, b.lane])).toEqual([[DRAFT_EVENT_ID, 0]]);
  });

  // The same for bands that run into the ghost's days: kept out of the top lane for their whole run,
  // they'd leave it empty and fold one away in every week they cover. They give it up only in the
  // ghost's own week.
  it("leaves the weeks the ghost band doesn't touch as they were, for bands that reach its days", () => {
    const span = (id: string, start: string, end: string) => ({
      ...timed(start, end),
      id,
      all_day: true,
    });
    // From two weeks before the ghost's week to the week after it.
    const through = [
      span("a", "2026-07-13", "2026-08-06"),
      span("b", "2026-07-13", "2026-08-06"),
      span("c", "2026-07-13", "2026-08-06"),
    ];
    const plain = buildWeekRows(start, 4, through);
    const weeks = buildWeekRows(start, 4, [
      ...through,
      { ...timed("2026-07-28T23:30:00", "2026-07-29T00:30:00"), id: DRAFT_EVENT_ID },
    ]);
    for (const w of [0, 1, 3]) {
      const lanes = new Map(weeks[w].bands.map((b) => [b.ev.id, b.lane]));
      expect([lanes.get("a"), lanes.get("b"), lanes.get("c")]).toEqual([0, 1, 2]);
      expect(weeks[w].cells.map((c) => c.hiddenBands)).toEqual([0, 0, 0, 0, 0, 0, 0]);
      expect(weeks[w].laneCount).toBe(plain[w].laneCount);
    }
    // In its own week the ghost has the top lane, and the band it puts out of sight is counted.
    expect(weeks[2].bands.find((b) => b.ev.id === DRAFT_EVENT_ID)?.lane).toBe(0);
    expect(weeks[2].cells[0].hiddenBands).toBe(1);
  });
});

describe("chipsShown", () => {
  const chip = (id: string) => ({
    id,
    calendar_id: "c",
    summary: id,
    description: null,
    location: null,
    start: "2026-07-15T09:00:00",
    end: "2026-07-15T10:00:00",
    all_day: false,
    html_link: null,
    uid: null,
  });
  const ids = (evs: { id: string }[]) => evs.map((e) => e.id);

  it("draws the first that fit, and the ghost in the last place when it would fold away", () => {
    const day = [chip("a"), chip("b"), chip("c"), chip("d"), chip(DRAFT_EVENT_ID)];
    expect(ids(chipsShown(day, 3))).toEqual(["a", "b", DRAFT_EVENT_ID]);
    // Already in reach, it stays where it sorts.
    expect(ids(chipsShown([chip(DRAFT_EVENT_ID), ...day.slice(0, 3)], 3))).toEqual([
      DRAFT_EVENT_ID,
      "a",
      "b",
    ]);
    // Without a ghost, the first that fit.
    expect(ids(chipsShown(day.slice(0, 4), 3))).toEqual(["a", "b", "c"]);
    expect(chipsShown(day, 0)).toEqual([]);
  });
});
