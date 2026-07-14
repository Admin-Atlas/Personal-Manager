// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { pinboardEntries } from "./calendarEntries";
import { BOARD_VERSION, type Board, type Widget } from "./types";

// The calendar's pinboard overlay draws freeform timeline entries. The load-bearing rules are the
// exclusions: a project-bound timeline must NOT contribute (its entries are already real milestones
// drawn by the milestone overlay, so including them would double-draw), an opted-out widget must not,
// and a board read straight out of the settings table must never throw into the calendar. These lock
// those in.

const board = (widgets: Widget[]): string => JSON.stringify({ version: BOARD_VERSION, widgets });

const timeline = (over: Partial<Widget> = {}): Widget => ({
  id: "w1",
  kind: "timeline",
  rect: { x: 0, y: 0, w: 3, h: 3 },
  items: [
    { id: "i1", date: "2026-07-20", label: "pitch" },
    { id: "i2", date: "2026-07-22", label: "demo" },
  ],
  ...over,
});

describe("pinboardEntries — freeform timeline entries for the calendar overlay", () => {
  it("lifts every dated entry of a freeform timeline", () => {
    expect(pinboardEntries(board([timeline()]))).toEqual([
      { widgetId: "w1", itemId: "i1", date: "2026-07-20", label: "pitch" },
      { widgetId: "w1", itemId: "i2", date: "2026-07-22", label: "demo" },
    ]);
  });

  it("skips a timeline bound to a project (its entries are real milestones — no double-draw)", () => {
    expect(pinboardEntries(board([timeline({ project: "Atlas" })]))).toEqual([]);
  });

  it("skips a timeline opted out, but shows one where the flag is unset or true (default on)", () => {
    expect(pinboardEntries(board([timeline({ showOnCalendar: false })]))).toEqual([]);
    expect(pinboardEntries(board([timeline({ showOnCalendar: true })]))).toHaveLength(2);
    expect(pinboardEntries(board([timeline()]))).toHaveLength(2); // unset → shown
  });

  it("reaches timelines inside a folder (folders never nest, so depth-1)", () => {
    const folder: Widget = {
      id: "f1",
      kind: "folder",
      rect: { x: 0, y: 0, w: 3, h: 3 },
      children: [timeline({ id: "w2" })],
    };
    expect(pinboardEntries(board([folder])).map((e) => e.widgetId)).toEqual(["w2", "w2"]);
  });

  it("ignores notes, dateless entries, and trims a label (falling back when blank)", () => {
    const note: Widget = { id: "n1", kind: "note", rect: { x: 0, y: 0, w: 3, h: 3 }, text: "hi" };
    const mixed = timeline({
      items: [
        { id: "i1", date: "", label: "dateless" },
        { id: "i2", date: "2026-07-20", label: "  spaced  " },
        { id: "i3", date: "2026-07-21", label: "   " },
      ],
    });
    expect(pinboardEntries(board([note, mixed]))).toEqual([
      { widgetId: "w1", itemId: "i2", date: "2026-07-20", label: "spaced" },
      // Blank falls back to the same word linkProject writes, so linking can't rename the row.
      { widgetId: "w1", itemId: "i3", date: "2026-07-21", label: "deadline" },
    ]);
  });

  it("yields nothing for absent, malformed, or wrong-version boards rather than throwing", () => {
    expect(pinboardEntries(null)).toEqual([]);
    expect(pinboardEntries("")).toEqual([]);
    expect(pinboardEntries("{not json")).toEqual([]);
    expect(
      pinboardEntries(JSON.stringify({ version: BOARD_VERSION + 1, widgets: [timeline()] })),
    ).toEqual([]);
    expect(pinboardEntries(JSON.stringify({ version: BOARD_VERSION } as Board))).toEqual([]);
  });

  it("skips malformed widgets/items without losing the good ones beside them", () => {
    const raw = JSON.stringify({
      version: BOARD_VERSION,
      widgets: [
        null,
        { id: "w9", kind: "timeline", rect: { x: 0, y: 0, w: 3, h: 3 }, items: [null, 7] },
        timeline({ id: "ok" }),
      ],
    });
    expect(pinboardEntries(raw).map((e) => e.widgetId)).toEqual(["ok", "ok"]);
  });
});
