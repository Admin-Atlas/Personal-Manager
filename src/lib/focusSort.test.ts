// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { sortProjects, type Sort } from "./focusSort";
import type { ProjectOverview, ProjectStatus } from "./types";

/** A project overview with only the fields the sort reads; the rest are inert defaults. */
function proj(over: {
  name: string;
  status?: ProjectStatus;
  due?: string | null;
  importance?: ProjectOverview["importance"];
  auto?: ProjectOverview["auto_importance"];
  active?: string | null;
}): ProjectOverview {
  return {
    name: over.name,
    status: over.status ?? "on_track",
    doc_count: 0,
    size: null,
    importance: over.importance ?? null,
    auto_importance: over.auto ?? null,
    blocked_by: null,
    last_activity: over.active ?? null,
    milestones: [],
    governing_milestone: over.due ? { due_date: over.due } : null,
    calendar_event: null,
  } as unknown as ProjectOverview;
}

const SMART: Sort = { key: "smart", dir: "asc" };
const names = (ps: ProjectOverview[]) => ps.map((p) => p.name);

describe("Smart sort", () => {
  it("puts due-soon projects first, in date order", () => {
    // The old comparator keyed on the status bucket alone and fell through to the NAME tiebreak, so
    // inside due_soon the order was alphabetical — something due today could sit below one due next
    // week. "aaa" sorts first alphabetically and last by date, which is what pins the fix.
    const out = sortProjects(
      [
        proj({ name: "aaa", status: "due_soon", due: "2026-08-01" }),
        proj({ name: "zzz", status: "due_soon", due: "2026-07-27" }),
        proj({ name: "mmm", status: "on_track", due: "2026-07-26" }),
      ],
      SMART,
    );
    expect(names(out)).toEqual(["zzz", "aaa", "mmm"]);
  });

  it("ranks by priority once due-soon is settled", () => {
    // The whole point of the report: importance used to be a separate, mutually exclusive key, so
    // Smart never consulted it at all.
    const out = sortProjects(
      [
        proj({ name: "low", importance: "low" }),
        proj({ name: "high", importance: "high" }),
        proj({ name: "medium", importance: "medium" }),
      ],
      SMART,
    );
    expect(names(out)).toEqual(["high", "medium", "low"]);
  });

  it("counts structural auto-importance as priority, as the card does", () => {
    const out = sortProjects(
      [proj({ name: "none" }), proj({ name: "auto-high", auto: "high" })],
      SMART,
    );
    expect(names(out)).toEqual(["auto-high", "none"]);
  });

  it("falls back to most-recently-active, sinking projects never touched", () => {
    const out = sortProjects(
      [
        proj({ name: "never" }),
        proj({ name: "old", active: "2026-01-01" }),
        proj({ name: "fresh", active: "2026-07-25" }),
      ],
      SMART,
    );
    expect(names(out)).toEqual(["fresh", "old", "never"]);
  });

  it("applies the keys in order: due-soon beats priority, priority beats activity", () => {
    const out = sortProjects(
      [
        proj({ name: "high-but-not-due", importance: "high", active: "2026-07-25" }),
        proj({ name: "due-and-unimportant", status: "due_soon", due: "2026-07-27" }),
        proj({ name: "medium", importance: "medium", active: "2026-07-24" }),
      ],
      SMART,
    );
    expect(names(out)).toEqual(["due-and-unimportant", "high-but-not-due", "medium"]);
  });

  it("reverses the whole chain when the direction flips", () => {
    const out = sortProjects(
      [
        proj({ name: "due", status: "due_soon", due: "2026-07-27" }),
        proj({ name: "high", importance: "high" }),
      ],
      { key: "smart", dir: "desc" },
    );
    expect(names(out)).toEqual(["high", "due"]);
  });

  it("keeps name as a stable tiebreak when every key ties", () => {
    const out = sortProjects([proj({ name: "b" }), proj({ name: "a" })], SMART);
    expect(names(out)).toEqual(["a", "b"]);
  });
});
