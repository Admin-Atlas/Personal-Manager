// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared T2 fixture (#884, C11 §1.5) as the webview reads it: every row a repeat rule of the
// shapes `types.ts` names, against a start of its own shape. Rust holds the same file to its own
// rule table, so a rule the two sides read differently fails one of them.

import { describe, expect, it } from "vitest";
import fixture from "./repeatRules.fixture.json";
import type { DayCode, NthCode, RepeatRule, RepeatStart } from "../types";

interface Row {
  rule: string;
  words: string;
  spec: RepeatRule;
}

const DAYS: readonly string[] = [
  "mon",
  "tue",
  "wed",
  "thu",
  "fri",
  "sat",
  "sun",
] satisfies DayCode[];
const NTHS: readonly string[] = ["first", "second", "third", "fourth", "last"] satisfies NthCode[];

/** Whether `rule` is one the backend reads: each kind with exactly its own fields, in range. */
function readable(rule: RepeatRule): boolean {
  const p = rule.pattern;
  const pattern = (() => {
    switch (p.kind) {
      case "daily":
      case "yearly_date":
        return true;
      case "weekly":
        return p.days.length > 0 && p.days.every((d) => DAYS.includes(d));
      case "monthly_day":
        return Number.isInteger(p.day) && p.day >= 1 && p.day <= 31;
      case "monthly_weekday":
        return NTHS.includes(p.nth) && DAYS.includes(p.weekday);
      case "yearly_weekday":
        return p.month >= 1 && p.month <= 12 && NTHS.includes(p.nth) && DAYS.includes(p.weekday);
      default:
        return false;
    }
  })();
  const e = rule.ends;
  const ends =
    e.kind === "never" ||
    (e.kind === "on_date" && /^\d{4}-\d{2}-\d{2}$/.test(e.date)) ||
    (e.kind === "after" && Number.isInteger(e.count) && e.count >= 1 && e.count <= 730);
  return (
    pattern && ends && Number.isInteger(rule.interval) && rule.interval >= 1 && rule.interval <= 99
  );
}

describe("the shared repeat-rule fixture", () => {
  const rows = fixture.rows as Row[];
  const start = fixture.start as RepeatStart;

  it("reads against the T2 start", () => {
    expect(start).toEqual({
      kind: "timed",
      date: "2026-10-13",
      time: "09:00",
      zone: "Europe/London",
    });
  });

  it("holds every T2 row as a rule the backend reads, with Google's words", () => {
    expect(rows).toHaveLength(16);
    for (const row of rows) {
      expect(row.rule.startsWith("RRULE:FREQ="), row.rule).toBe(true);
      expect(row.words.length, row.rule).toBeGreaterThan(0);
      expect(readable(row.spec), row.rule).toBe(true);
    }
    expect(new Set(rows.map((r) => r.rule)).size).toBe(rows.length);
  });
});
