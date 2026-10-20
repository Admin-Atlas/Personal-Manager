// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A new event's repeat, as pure functions (#884, C11): the Repeat row's value, the Custom dialog's
// fields and the checks they get before anything is asked of the backend. A rule's words and the
// ready-made rules come from the backend (the repeat menu's reads), so nothing here says a rule in
// words except the dialog's own labels: its monthly choices and its yearly line, built from the same
// English day and month names the backend uses.
//
// The checks run in the backend's order and say the backend's words, so the dialog's Done refuses
// exactly what a save would; the backend still has the final say (a rule with no occurrence before
// its end date, a first day the clocks skip), said under the Repeat row.

import type { DayCode, NthCode, RepeatRule, RepeatStart, TimeDraft } from "../types";
import { isoToDate } from "../dateField";
import { formatDateOnly } from "../format";

/** Every 1 to this many days, weeks, months or years. */
export const MAX_INTERVAL = 99;
/** Google allows at most this many occurrences of a repeating event. */
export const MAX_OCCURRENCES = 730;

/** The Repeat row's value: no repeat, a rule from the editor's set, or a repeating event's rule PM
 *  can only show, kept unless the user replaces it. */
export type RepeatValue =
  { kind: "once" } | { kind: "rule"; rule: RepeatRule } | { kind: "kept"; words: string };

export const ONCE: RepeatValue = { kind: "once" };

export type RepeatUnit = "day" | "week" | "month" | "year";
export type MonthlyChoice =
  { by: "day"; day: number } | { by: "weekday"; nth: NthCode; weekday: DayCode };
export type YearlyChoice =
  { by: "date" } | { by: "weekday"; month: number; nth: NthCode; weekday: DayCode };

/** What the Custom dialog's fields hold. Numbers stay as typed until checked. */
export interface RepeatForm {
  unit: RepeatUnit;
  interval: string;
  days: DayCode[];
  monthly: MonthlyChoice;
  yearly: YearlyChoice;
  ends: "never" | "on_date" | "after";
  /** The end date, YYYY-MM-DD. */
  date: string;
  count: string;
}

export type RepeatProblem =
  "interval" | "no_weekdays" | "count" | "too_many" | "no_end_date" | "ends_before_start";

/** The days of the week, Monday first (PM's weeks, and the order the backend writes them in). */
export const WEEKDAYS: readonly { code: DayCode; name: string }[] = [
  { code: "mon", name: "Monday" },
  { code: "tue", name: "Tuesday" },
  { code: "wed", name: "Wednesday" },
  { code: "thu", name: "Thursday" },
  { code: "fri", name: "Friday" },
  { code: "sat", name: "Saturday" },
  { code: "sun", name: "Sunday" },
];

const DAY_ORDER: readonly DayCode[] = WEEKDAYS.map((d) => d.code);

const MONTHS = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

const NTHS: readonly NthCode[] = ["first", "second", "third", "fourth"];

/** A day's full English name, as the backend's words say it. */
export const dayName = (code: DayCode) => WEEKDAYS[DAY_ORDER.indexOf(code)].name;

/** `date` (YYYY-MM-DD) as a local calendar day, or null when it isn't a real one. */
const dayOf = (date: string) => isoToDate(date);

const daysInMonth = (y: number, month0: number) => new Date(y, month0 + 1, 0).getDate();

/** The day of the week `date` (YYYY-MM-DD) falls on. */
export function dayCodeOf(date: string): DayCode {
  const d = dayOf(date);
  // Sunday is 0 to `Date`, last in PM's week.
  return d ? DAY_ORDER[(d.getDay() + 6) % 7] : "mon";
}

/** Which of its month's weekdays `date` is: first to fourth, or null for a fifth (Google offers none). */
export function nthOf(date: string): NthCode | null {
  const d = dayOf(date);
  return d ? (NTHS[Math.floor((d.getDate() - 1) / 7)] ?? null) : null;
}

/** Whether `date` is in its month's last seven days, so it is that month's last of its weekday. */
export function inLastWeek(date: string): boolean {
  const d = dayOf(date);
  return !!d && d.getDate() + 7 > daysInMonth(d.getFullYear(), d.getMonth());
}

/** `date` (YYYY-MM-DD) three months on, kept to that month's last day (31-01 gives 30-04). */
function threeMonthsOn(date: string): string {
  const d = dayOf(date);
  if (!d) return date;
  const y = d.getFullYear();
  const m = d.getMonth() + 3;
  const day = Math.min(d.getDate(), daysInMonth(y, m));
  const out = new Date(y, m, day);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${out.getFullYear()}-${pad(out.getMonth() + 1)}-${pad(out.getDate())}`;
}

/** Days sorted Monday first, each once. */
const weekOrder = (days: readonly DayCode[]) => DAY_ORDER.filter((code) => days.includes(code));

/** The Custom dialog's fields for `rule`, or Google's defaults with no rule (weekly on the start's
 *  weekday, ending three months on, or after 13 times: probe P-R4). The fields a rule doesn't set
 *  keep those defaults, so switching the unit in the dialog offers something sensible. */
export function formFromRule(rule: RepeatRule | null, startDate: string): RepeatForm {
  const start = dayOf(startDate);
  const form: RepeatForm = {
    unit: "week",
    interval: "1",
    days: [dayCodeOf(startDate)],
    monthly: { by: "day", day: start ? start.getDate() : 1 },
    yearly: { by: "date" },
    ends: "never",
    date: threeMonthsOn(startDate),
    count: "13",
  };
  if (!rule) return form;
  form.interval = String(rule.interval);
  const p = rule.pattern;
  switch (p.kind) {
    case "daily":
      form.unit = "day";
      break;
    case "weekly":
      form.unit = "week";
      form.days = weekOrder(p.days);
      break;
    case "monthly_day":
      form.unit = "month";
      form.monthly = { by: "day", day: p.day };
      break;
    case "monthly_weekday":
      form.unit = "month";
      form.monthly = { by: "weekday", nth: p.nth, weekday: p.weekday };
      break;
    case "yearly_date":
      form.unit = "year";
      break;
    case "yearly_weekday":
      form.unit = "year";
      form.yearly = { by: "weekday", month: p.month, nth: p.nth, weekday: p.weekday };
      break;
  }
  const e = rule.ends;
  if (e.kind === "on_date") {
    form.ends = "on_date";
    form.date = e.date;
  } else if (e.kind === "after") {
    form.ends = "after";
    form.count = String(e.count);
  }
  return form;
}

/** A whole number typed in a field, or null. */
function wholeNumber(text: string): number | null {
  const t = text.trim();
  return /^\d+$/.test(t) ? Number(t) : null;
}

/** The rule the dialog's fields make, or the first thing wrong with them, in the order the backend
 *  checks a rule: the interval, the weekdays, the count, then the end date (on the start day is
 *  fine). */
export function ruleFromForm(
  form: RepeatForm,
  startDate: string,
): { ok: true; rule: RepeatRule } | { ok: false; problem: RepeatProblem } {
  const interval = wholeNumber(form.interval);
  if (interval === null || interval < 1 || interval > MAX_INTERVAL) {
    return { ok: false, problem: "interval" };
  }
  if (form.unit === "week" && form.days.length === 0) return { ok: false, problem: "no_weekdays" };
  let ends: RepeatRule["ends"] = { kind: "never" };
  if (form.ends === "after") {
    const count = wholeNumber(form.count);
    if (count === null || count < 1) return { ok: false, problem: "count" };
    if (count > MAX_OCCURRENCES) return { ok: false, problem: "too_many" };
    ends = { kind: "after", count };
  } else if (form.ends === "on_date") {
    if (!dayOf(form.date)) return { ok: false, problem: "no_end_date" };
    // ISO dates compare as strings.
    if (form.date < startDate) return { ok: false, problem: "ends_before_start" };
    ends = { kind: "on_date", date: form.date };
  }
  let pattern: RepeatRule["pattern"];
  switch (form.unit) {
    case "day":
      pattern = { kind: "daily" };
      break;
    case "week":
      pattern = { kind: "weekly", days: weekOrder(form.days) };
      break;
    case "month":
      pattern =
        form.monthly.by === "day"
          ? { kind: "monthly_day", day: form.monthly.day }
          : { kind: "monthly_weekday", nth: form.monthly.nth, weekday: form.monthly.weekday };
      break;
    case "year":
      pattern =
        form.yearly.by === "date"
          ? { kind: "yearly_date" }
          : {
              kind: "yearly_weekday",
              month: form.yearly.month,
              nth: form.yearly.nth,
              weekday: form.yearly.weekday,
            };
      break;
  }
  return { ok: true, rule: { pattern, interval, ends } };
}

/** What's wrong with the dialog's fields, in words. The backend's own words for the same limits
 *  (`SpecError`), which a test pins to these. */
export function repeatProblemText(problem: RepeatProblem): string {
  switch (problem) {
    case "interval":
      return `Repeat every 1 to ${MAX_INTERVAL}.`;
    case "no_weekdays":
      return "Pick at least one day of the week.";
    case "count":
      return "It has to happen at least once.";
    case "too_many":
      return `Google allows at most ${MAX_OCCURRENCES} occurrences of a repeating event.`;
    case "no_end_date":
      return "Pick an end date.";
    case "ends_before_start":
      return "The end date is before the first occurrence.";
  }
}

const sameMonthly = (a: MonthlyChoice, b: MonthlyChoice) =>
  a.by === "day"
    ? b.by === "day" && a.day === b.day
    : b.by === "weekday" && a.nth === b.nth && a.weekday === b.weekday;

/** The Monthly choices for a start date, Google's way: "on day 13", "on the second Tuesday", and "on
 *  the last Tuesday" in the month's last seven days; never a fifth. `current` joins them when it
 *  isn't one (a rule read from Google), so the dialog never drops it. */
export function monthlyChoices(startDate: string, current?: MonthlyChoice): MonthlyChoice[] {
  const start = dayOf(startDate);
  const weekday = dayCodeOf(startDate);
  const out: MonthlyChoice[] = [{ by: "day", day: start ? start.getDate() : 1 }];
  const nth = nthOf(startDate);
  if (nth) out.push({ by: "weekday", nth, weekday });
  if (inLastWeek(startDate)) out.push({ by: "weekday", nth: "last", weekday });
  if (current && !out.some((c) => sameMonthly(c, current))) out.push(current);
  return out;
}

/** A Monthly choice as Google's dialog says it: "Monthly on day 13", "Monthly on the last Friday". */
export function monthlyLabel(c: MonthlyChoice): string {
  return c.by === "day"
    ? `Monthly on day ${c.day}`
    : `Monthly on the ${c.nth} ${dayName(c.weekday)}`;
}

/** `date` (YYYY-MM-DD) as a day and an English month, as the backend says a yearly rule's date:
 *  "13 October". */
export function formatDayMonth(date: string): string {
  const d = dayOf(date);
  return d ? `${d.getDate()} ${MONTHS[d.getMonth()]}` : date;
}

/** The Custom dialog's yearly line: "Annually on 13 October" on the start's own date, or a weekday
 *  rule's own words (Google's dialog offers no yearly choices, so this is text). */
export function yearlyLabel(c: YearlyChoice, startDate: string): string {
  return c.by === "date"
    ? `Annually on ${formatDayMonth(startDate)}`
    : `Annually on the ${c.nth} ${dayName(c.weekday)} of ${MONTHS[c.month - 1] ?? ""}`;
}

/** Whether two rules mean the same (weekday order aside). */
export function sameRule(a: RepeatRule, b: RepeatRule): boolean {
  if (a.interval !== b.interval) return false;
  const ea = a.ends;
  const eb = b.ends;
  const ends =
    ea.kind === "never"
      ? eb.kind === "never"
      : ea.kind === "on_date"
        ? eb.kind === "on_date" && ea.date === eb.date
        : eb.kind === "after" && ea.count === eb.count;
  if (!ends) return false;
  const pa = a.pattern;
  const pb = b.pattern;
  switch (pa.kind) {
    case "daily":
    case "yearly_date":
      return pb.kind === pa.kind;
    case "weekly":
      // A day listed twice is still one day.
      return pb.kind === "weekly" && weekOrder(pa.days).join() === weekOrder(pb.days).join();
    case "monthly_day":
      return pb.kind === "monthly_day" && pa.day === pb.day;
    case "monthly_weekday":
      return pb.kind === "monthly_weekday" && pa.nth === pb.nth && pa.weekday === pb.weekday;
    case "yearly_weekday":
      return (
        pb.kind === "yearly_weekday" &&
        pa.month === pb.month &&
        pa.nth === pb.nth &&
        pa.weekday === pb.weekday
      );
  }
}

/** Whether two Repeat row values mean the same. */
export function sameRepeat(a: RepeatValue, b: RepeatValue): boolean {
  switch (a.kind) {
    case "once":
      return b.kind === "once";
    case "rule":
      return b.kind === "rule" && sameRule(a.rule, b.rule);
    case "kept":
      return b.kind === "kept" && a.words === b.words;
  }
}

/** A copy of `rule` sharing nothing with it, for what a save sends. */
export function copyRule(rule: RepeatRule): RepeatRule {
  const p = rule.pattern;
  return {
    pattern: p.kind === "weekly" ? { ...p, days: [...p.days] } : { ...p },
    interval: rule.interval,
    ends: { ...rule.ends },
  };
}

/** A rule after its event's start date moves from `from` to `to`: a rule whose day came from the
 *  start date follows it (weekly on that one weekday, monthly on its day or its numbered or last
 *  weekday), as the 10-09 design has it (probe P-R5); any other rule stays as it is, the same object.
 *  An end date before `to` is left alone: the Repeat row then says so. */
export function followStartDate(rule: RepeatRule, from: string, to: string): RepeatRule {
  const fromDay = dayOf(from);
  const toDay = dayOf(to);
  if (from === to || !fromDay || !toDay) return rule;
  const p = rule.pattern;
  const wdFrom = dayCodeOf(from);
  const wdTo = dayCodeOf(to);
  switch (p.kind) {
    case "weekly":
      if (p.days.length === 1 && p.days[0] === wdFrom) {
        return { ...rule, pattern: { kind: "weekly", days: [wdTo] } };
      }
      return rule;
    case "monthly_day":
      if (p.day === fromDay.getDate()) {
        return { ...rule, pattern: { kind: "monthly_day", day: toDay.getDate() } };
      }
      return rule;
    case "monthly_weekday": {
      if (p.weekday !== wdFrom) return rule;
      const nthFrom = nthOf(from);
      if (p.nth !== "last" && p.nth === nthFrom) {
        return {
          ...rule,
          pattern: { kind: "monthly_weekday", nth: nthOf(to) ?? "last", weekday: wdTo },
        };
      }
      if (p.nth === "last" && inLastWeek(from)) {
        // Not in its month's last week, the new day is one of its first four.
        const nth = inLastWeek(to) ? "last" : (nthOf(to) ?? "last");
        return { ...rule, pattern: { kind: "monthly_weekday", nth, weekday: wdTo } };
      }
      return rule;
    }
    default:
      // "Annually on its own date" follows by itself; the rest name their days outright.
      return rule;
  }
}

/** The start the backend reads a rule against, from the editor's time (with where the first
 *  occurrence ends, so a moved start whose end the clocks skip is said too); null while a date is
 *  empty. */
export function repeatStartOf(time: TimeDraft): RepeatStart | null {
  if (time.kind === "all_day") {
    return time.first_day ? { kind: "all_day", date: time.first_day } : null;
  }
  if (!time.start_date || !time.end_date) return null;
  return {
    kind: "timed",
    date: time.start_date,
    time: time.start_time,
    zone: time.start_zone,
    end: { date: time.end_date, time: time.end_time, zone: time.end_zone },
  };
}

/** The first day an event's time covers (YYYY-MM-DD): where a rule's own days are read from. */
export const startDateOf = (time: TimeDraft) =>
  time.kind === "timed" ? time.start_date : time.first_day;

/** The line for a first occurrence the rule moved off the day asked for (A27): "Starts on Wed 14-10,
 *  the first day this repeat allows." The editor's own date stays as typed; the save moves it. */
export function movedStartText(starts: string): string {
  const short = dayName(dayCodeOf(starts)).slice(0, 3);
  return `Starts on ${short} ${formatDateOnly(starts)}, the first day this repeat allows.`;
}
