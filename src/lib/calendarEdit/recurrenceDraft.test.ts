// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Repeat row's value and the Custom dialog's fields (#884, C11 §4): every T2 rule (the shared
// fixture Rust holds to the same table) round-trips through the dialog's fields, the dialog refuses
// what the backend refuses in the backend's own words, its monthly choices are Google's for the start
// date, and a rule whose day came from the start date follows the start when it moves.

import { describe, expect, it } from "vitest";
import fixture from "./repeatRules.fixture.json";
import type { DayCode, RepeatRule } from "../types";
import {
  formatDayMonth,
  formFromRule,
  followStartDate,
  monthlyChoices,
  monthlyLabel,
  movedStartText,
  repeatProblemText,
  repeatingStartOf,
  repeatStartOf,
  ruleFromForm,
  sameRepeat,
  sameRule,
  yearlyLabel,
  type RepeatForm,
  type RepeatProblem,
} from "./recurrenceDraft";

const T2 = "2026-10-13"; // a Tuesday, the second of its month
const rows = fixture.rows as { rule: string; words: string; spec: RepeatRule }[];

const weekly = (days: DayCode[]): RepeatRule => ({
  pattern: { kind: "weekly", days },
  interval: 1,
  ends: { kind: "never" },
});

/** The dialog's defaults on 13-10-2026, with `patch` typed over them. */
const form = (patch: Partial<RepeatForm> = {}): RepeatForm => ({
  ...formFromRule(null, T2),
  ...patch,
});

const problemOf = (f: RepeatForm, start = T2): RepeatProblem | null => {
  const r = ruleFromForm(f, start);
  return r.ok ? null : r.problem;
};

describe("the Custom dialog's fields", () => {
  it("round trips every T2 rule", () => {
    expect(rows).toHaveLength(16);
    for (const row of rows) {
      const back = ruleFromForm(formFromRule(row.spec, T2), T2);
      expect(back, row.rule).toEqual({ ok: true, rule: row.spec });
    }
  });

  it("opens on Google's defaults: weekly on the start's day, three months, 13 times", () => {
    expect(formFromRule(null, T2)).toEqual({
      unit: "week",
      interval: "1",
      days: ["tue"],
      monthly: { by: "day", day: 13 },
      yearly: { by: "date" },
      ends: "never",
      date: "2027-01-13",
      count: "13",
    });
    // Three months from the 31st is the last day of a shorter month.
    expect(formFromRule(null, "2026-11-30").date).toBe("2027-02-28");
  });

  it("After 730 is accepted and 731 is refused with Google's words", () => {
    expect(ruleFromForm(form({ ends: "after", count: "730" }), T2)).toEqual({
      ok: true,
      rule: { ...weekly(["tue"]), ends: { kind: "after", count: 730 } },
    });
    expect(problemOf(form({ ends: "after", count: "731" }))).toBe("too_many");
    expect(repeatProblemText("too_many")).toBe(
      "Google allows at most 730 occurrences of a repeating event.",
    );
  });

  it("After 0 is refused", () => {
    expect(problemOf(form({ ends: "after", count: "0" }))).toBe("count");
    expect(problemOf(form({ ends: "after", count: "" }))).toBe("count");
  });

  it("an end date before the start is refused; on the start day it is accepted", () => {
    expect(problemOf(form({ ends: "on_date", date: "2026-10-12" }))).toBe("ends_before_start");
    expect(ruleFromForm(form({ ends: "on_date", date: T2 }), T2)).toEqual({
      ok: true,
      rule: { ...weekly(["tue"]), ends: { kind: "on_date", date: T2 } },
    });
  });

  it("an empty end date asks for one", () => {
    expect(problemOf(form({ ends: "on_date", date: "" }))).toBe("no_end_date");
    expect(problemOf(form({ ends: "on_date", date: "2026-02-30" }))).toBe("no_end_date");
    // Only when it ends on a date: Never doesn't read the field.
    expect(problemOf(form({ ends: "never", date: "" }))).toBeNull();
  });

  it("no weekday is refused", () => {
    expect(problemOf(form({ days: [] }))).toBe("no_weekdays");
    // Another unit doesn't need any.
    expect(problemOf(form({ days: [], unit: "day" }))).toBeNull();
  });

  it("interval 0 and 100 are refused, 1 and 99 accepted", () => {
    expect(problemOf(form({ interval: "0" }))).toBe("interval");
    expect(problemOf(form({ interval: "100" }))).toBe("interval");
    expect(problemOf(form({ interval: "1" }))).toBeNull();
    expect(problemOf(form({ interval: " 99 " }))).toBeNull();
  });

  it("a non-number is refused", () => {
    expect(problemOf(form({ interval: "two" }))).toBe("interval");
    expect(problemOf(form({ interval: "1.5" }))).toBe("interval");
    expect(problemOf(form({ ends: "after", count: "ten" }))).toBe("count");
  });

  it("refuses in the backend's order: the interval first, the end date last", () => {
    expect(problemOf(form({ interval: "0", days: [], ends: "on_date", date: "2026-01-01" }))).toBe(
      "interval",
    );
    expect(problemOf(form({ days: [], ends: "after", count: "0" }))).toBe("no_weekdays");
  });

  it("writes weekdays Monday first, each once", () => {
    expect(ruleFromForm(form({ days: ["fri", "mon", "fri"] }), T2)).toEqual({
      ok: true,
      rule: weekly(["mon", "fri"]),
    });
  });

  // §1.3's words, literally: the dialog refuses with what the backend says of the same rule.
  it("the problem words match the backend's", () => {
    const words: Record<RepeatProblem, string> = {
      interval: "Repeat every 1 to 99.",
      no_weekdays: "Pick at least one day of the week.",
      count: "It has to happen at least once.",
      too_many: "Google allows at most 730 occurrences of a repeating event.",
      no_end_date: "Pick an end date.",
      ends_before_start: "The end date is before the first occurrence.",
    };
    for (const [problem, text] of Object.entries(words)) {
      expect(repeatProblemText(problem as RepeatProblem)).toBe(text);
    }
  });
});

describe("the monthly and yearly choices", () => {
  it("monthly choices follow the start date", () => {
    expect(monthlyChoices("2026-10-13").map(monthlyLabel)).toEqual([
      "Monthly on day 13",
      "Monthly on the second Tuesday",
    ]);
    expect(monthlyChoices("2026-10-27").map(monthlyLabel)).toEqual([
      "Monthly on day 27",
      "Monthly on the fourth Tuesday",
      "Monthly on the last Tuesday",
    ]);
    // The 31st is a fifth Saturday, which Google never offers: only the last.
    expect(monthlyChoices("2026-10-31").map(monthlyLabel)).toEqual([
      "Monthly on day 31",
      "Monthly on the last Saturday",
    ]);
  });

  it("a rule read from Google keeps its monthly choice", () => {
    const lastFriday = { by: "weekday" as const, nth: "last" as const, weekday: "fri" as const };
    expect(monthlyChoices(T2, lastFriday).map(monthlyLabel)).toEqual([
      "Monthly on day 13",
      "Monthly on the second Tuesday",
      "Monthly on the last Friday",
    ]);
    // One of the start's own isn't listed twice.
    expect(monthlyChoices(T2, { by: "day", day: 13 })).toHaveLength(2);
  });

  it("says the monthly and yearly rules as the backend does", () => {
    for (const row of rows) {
      const p = row.spec.pattern;
      if (row.spec.interval !== 1 || row.spec.ends.kind !== "never") continue;
      if (p.kind === "monthly_day") expect(monthlyLabel({ by: "day", ...p })).toBe(row.words);
      if (p.kind === "monthly_weekday") {
        expect(monthlyLabel({ by: "weekday", nth: p.nth, weekday: p.weekday })).toBe(row.words);
      }
      if (p.kind === "yearly_date") expect(yearlyLabel({ by: "date" }, T2)).toBe(row.words);
      if (p.kind === "yearly_weekday") {
        expect(yearlyLabel({ by: "weekday", ...p }, T2)).toBe(row.words);
      }
    }
    expect(formatDayMonth(T2)).toBe("13 October");
    expect(formatDayMonth("2028-02-29")).toBe("29 February");
  });

  it("a yearly weekday rule is kept while the unit stays year", () => {
    const rule: RepeatRule = {
      pattern: { kind: "yearly_weekday", month: 10, nth: "second", weekday: "tue" },
      interval: 1,
      ends: { kind: "never" },
    };
    const f = formFromRule(rule, T2);
    expect(f.unit).toBe("year");
    // Changing the interval keeps it; the unit and back again keeps it too.
    expect(ruleFromForm({ ...f, interval: "2" }, T2)).toEqual({
      ok: true,
      rule: { ...rule, interval: 2 },
    });
    expect(ruleFromForm({ ...f, unit: "month" }, T2)).toMatchObject({
      ok: true,
      rule: { pattern: { kind: "monthly_day", day: 13 } },
    });
    expect(ruleFromForm({ ...f, unit: "year" }, T2)).toEqual({ ok: true, rule });
  });
});

describe("comparing rules", () => {
  it("sameRule ignores weekday order", () => {
    expect(sameRule(weekly(["wed", "mon"]), weekly(["mon", "wed"]))).toBe(true);
    expect(sameRule(weekly(["mon"]), weekly(["mon", "wed"]))).toBe(false);
    expect(sameRule(weekly(["mon"]), { ...weekly(["mon"]), interval: 2 })).toBe(false);
    expect(
      sameRule(weekly(["mon"]), { ...weekly(["mon"]), ends: { kind: "after", count: 2 } }),
    ).toBe(false);
  });

  it("sameRepeat tells no repeat, a rule and a kept one apart", () => {
    const rule = { kind: "rule" as const, rule: weekly(["mon"]) };
    expect(sameRepeat({ kind: "once" }, { kind: "once" })).toBe(true);
    expect(sameRepeat({ kind: "once" }, rule)).toBe(false);
    expect(sameRepeat(rule, { kind: "rule", rule: weekly(["mon"]) })).toBe(true);
    expect(sameRepeat({ kind: "kept", words: "Repeats" }, { kind: "kept", words: "Repeats" })).toBe(
      true,
    );
  });
});

describe("a rule following its start date", () => {
  const monthly = (nth: "first" | "second" | "third" | "fourth" | "last", weekday: "tue" | "wed") =>
    ({
      pattern: { kind: "monthly_weekday", nth, weekday },
      interval: 1,
      ends: { kind: "never" },
    }) satisfies RepeatRule;

  it("followStartDate moves a one-day weekly rule, not a two-day one", () => {
    expect(followStartDate(weekly(["tue"]), T2, "2026-10-14")).toEqual(weekly(["wed"]));
    const two = weekly(["mon", "tue"]);
    expect(followStartDate(two, T2, "2026-10-14")).toBe(two);
    // A weekly rule on another day than the start's isn't the start's.
    const other = weekly(["fri"]);
    expect(followStartDate(other, T2, "2026-10-14")).toBe(other);
  });

  it("followStartDate moves the second Tuesday to the second Wednesday", () => {
    expect(followStartDate(monthly("second", "tue"), T2, "2026-10-14")).toEqual(
      monthly("second", "wed"),
    );
    // A day of the month follows the day.
    const day13: RepeatRule = { ...weekly([]), pattern: { kind: "monthly_day", day: 13 } };
    expect(followStartDate(day13, T2, "2026-10-14").pattern).toEqual({
      kind: "monthly_day",
      day: 14,
    });
    // A fifth has no number: it becomes the last.
    expect(followStartDate(monthly("second", "tue"), T2, "2026-12-30")).toEqual(
      monthly("last", "wed"),
    );
  });

  it("followStartDate keeps last when both dates are in their month's last week", () => {
    expect(followStartDate(monthly("last", "tue"), "2026-10-27", "2026-10-28")).toEqual(
      monthly("last", "wed"),
    );
    // Out of the last week, it takes the new day's number.
    expect(followStartDate(monthly("last", "tue"), "2026-10-27", "2026-10-14")).toEqual(
      monthly("second", "wed"),
    );
  });

  it("leaves an end date before the new start for the Repeat row to say", () => {
    const ends: RepeatRule = { ...weekly(["mon", "wed"]), ends: { kind: "on_date", date: T2 } };
    expect(followStartDate(ends, T2, "2026-11-03")).toBe(ends);
  });
});

describe("the start a rule is read against", () => {
  it("is the timed start with its end, or the first day", () => {
    expect(
      repeatStartOf({
        kind: "timed",
        start_date: T2,
        start_time: "09:00",
        start_zone: "Europe/London",
        end_date: T2,
        end_time: "10:00",
        end_zone: "America/New_York",
      }),
    ).toEqual({
      kind: "timed",
      date: T2,
      time: "09:00",
      zone: "Europe/London",
      end: { date: T2, time: "10:00", zone: "America/New_York" },
    });
    expect(repeatStartOf({ kind: "all_day", first_day: T2, last_day: T2 })).toEqual({
      kind: "all_day",
      date: T2,
    });
    expect(repeatStartOf({ kind: "all_day", first_day: "", last_day: "" })).toBeNull();
  });

  // C11 critique 2 M6: a new rule for a repeating event is read from its first day, at this
  // occurrence's time and zone; a yearly one on its own date from this occurrence's.
  it("is the repeating event's first day for its new rule, this date for a yearly one", () => {
    const time = {
      kind: "timed" as const,
      start_date: T2,
      start_time: "09:00",
      start_zone: "America/New_York",
      end_date: T2,
      end_time: "10:00",
      end_zone: "America/New_York",
    };
    expect(repeatingStartOf(weekly(["tue"]), "2026-10-06", time)).toEqual({
      kind: "timed",
      date: "2026-10-06",
      time: "09:00",
      zone: "America/New_York",
    });
    const yearly: RepeatRule = {
      pattern: { kind: "yearly_date" },
      interval: 1,
      ends: { kind: "never" },
    };
    expect(repeatingStartOf(yearly, "2026-10-06", time)).toEqual(repeatStartOf(time));
    expect(
      repeatingStartOf(weekly(["tue"]), "2026-10-06", {
        kind: "all_day",
        first_day: T2,
        last_day: T2,
      }),
    ).toEqual({ kind: "all_day", date: "2026-10-06" });
    expect(repeatingStartOf(weekly(["tue"]), "2026-10-06", { ...time, start_date: "" })).toBeNull();
  });

  it("says where a moved start falls", () => {
    expect(movedStartText("2026-10-14")).toMatch(
      /^Starts on Wed 14-10(-2026)?, the first day this repeat allows\.$/,
    );
  });
});
