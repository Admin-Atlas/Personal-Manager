// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An event's notifications in the editor (#884, C13), as pure functions. Google holds each one as a
// way to send it and a number of minutes before the event: a timed event's count back from its
// start, an all-day event's from 00:00 on its first day, so "1 day before at 09:00" is 1440 − 540 =
// 900 (INFERRED, live probe E9b). These turn minutes into the words and fields the editor shows, and
// back. The backend checks every list again before it goes (its `check_reminders`); a notification
// Google holds that PM wouldn't make itself is shown as it is and kept unless the user removes it.

import type { Reminder, RemindersDraft } from "../types";

/** Google's limits on an event's own notifications. */
export const MAX_REMINDERS = 5;
/** Four weeks, in minutes. */
export const MAX_MINUTES = 40320;

export type ReminderMethod = "popup" | "email";
export const isKnownMethod = (m: string): m is ReminderMethod => m === "popup" || m === "email";

/** The ways PM sets a notification, by the names Google's menu gives them. */
export const METHODS: readonly { value: ReminderMethod; label: string }[] = [
  { value: "popup", label: "Notification" },
  { value: "email", label: "Email" },
];

export type TimedUnit = "minutes" | "hours" | "days" | "weeks";
export type AllDayUnit = "days" | "weeks";

const UNIT_MINUTES: Readonly<Record<TimedUnit, number>> = {
  minutes: 1,
  hours: 60,
  days: 1440,
  weeks: 10080,
};

const UNIT_WORDS: Readonly<Record<TimedUnit, [string, string]>> = {
  minutes: ["minute", "minutes"],
  hours: ["hour", "hours"],
  days: ["day", "days"],
  weeks: ["week", "weeks"],
};

/** A unit's word for `n` of it: singular at 1. */
export const unitWord = (n: number, unit: TimedUnit) => UNIT_WORDS[unit][n === 1 ? 0 : 1];

export const TIMED_UNITS: readonly TimedUnit[] = ["minutes", "hours", "days", "weeks"];
export const ALL_DAY_UNITS: readonly AllDayUnit[] = ["days", "weeks"];

export interface TimedParts {
  n: number;
  unit: TimedUnit;
}

/** 90 → {90, minutes}; 120 → {2, hours}; 1440 → {1, days}; 10080 → {1, weeks}: the largest whole
 *  unit. */
export function timedParts(minutes: number): TimedParts {
  if (minutes > 0) {
    for (const unit of ["weeks", "days", "hours"] as const) {
      if (minutes % UNIT_MINUTES[unit] === 0) return { n: minutes / UNIT_MINUTES[unit], unit };
    }
  }
  return { n: minutes, unit: "minutes" };
}

export const timedMinutes = (p: TimedParts) => p.n * UNIT_MINUTES[p.unit];

export interface AllDayParts {
  n: number;
  unit: AllDayUnit;
  /** `HH:MM`. */
  at: string;
}

const hhmm = (m: number) =>
  `${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;

/** Minutes before 00:00 of the event's first day (INFERRED, E9b). 900 → {1, days, "09:00"};
 *  1440 → {1, days, "00:00"}; 9540 → {1, weeks, "09:00"}; 0 → {0, days, "00:00"}; a negative value
 *  Google may hold (−540) → {0, days, "09:00"} ("on the day"). Never fails: any whole number reads as
 *  something. */
export function allDayParts(minutes: number): AllDayParts {
  if (minutes <= 0) return { n: 0, unit: "days", at: hhmm(-minutes % 1440) };
  const days = Math.ceil(minutes / 1440);
  const at = hhmm(days * 1440 - minutes);
  return days % 7 === 0 ? { n: days / 7, unit: "weeks", at } : { n: days, unit: "days", at };
}

/** n·(1 or 7)·1440 − the time's minutes. */
export function allDayMinutes(p: AllDayParts): number {
  const [h, m] = p.at.split(":").map(Number);
  return p.n * (p.unit === "weeks" ? 7 : 1) * 1440 - (h * 60 + m);
}

/** "30 minutes before", "1 hour before, by email", "1 day before at 09:00", "On the day at 09:00",
 *  "When it starts". An unknown method: the time, then "(sent a way PM can't change)". */
export function reminderText(r: Reminder, allDay: boolean): string {
  let when: string;
  if (allDay) {
    const p = allDayParts(r.minutes);
    when =
      p.n === 0 ? `On the day at ${p.at}` : `${p.n} ${unitWord(p.n, p.unit)} before at ${p.at}`;
  } else if (r.minutes === 0) {
    when = "When it starts";
  } else if (r.minutes < 0) {
    // Google's range starts at 0, so only an app writing past it leaves one.
    const p = timedParts(-r.minutes);
    when = `${p.n} ${unitWord(p.n, p.unit)} after it starts`;
  } else {
    const p = timedParts(r.minutes);
    when = `${p.n} ${unitWord(p.n, p.unit)} before`;
  }
  if (r.method === "popup") return when;
  if (r.method === "email") return `${when}, by email`;
  return `${when} (sent a way PM can't change)`;
}

/** A row's count as typed: a whole number, or null. */
export function parseCount(text: string): number | null {
  const t = text.trim();
  return /^\d+$/.test(t) ? Number(t) : null;
}

/** Why a row can't be committed, or null: not a whole number (null), none for an all-day event (it
 *  counts from the midnight before it, so the day itself, at any time of it, would be after that:
 *  0 days at 09:00 is −540), or past four weeks. */
export function rowProblem(minutes: number | null, allDay: boolean): string | null {
  if (minutes === null || !Number.isInteger(minutes)) return "Use a whole number.";
  if (allDay && minutes <= 0) return "Pick at least 1 day before.";
  if (minutes < 0) return "Use a whole number.";
  if (minutes > MAX_MINUTES) return "Google allows notifications up to 4 weeks before.";
  return null;
}

/** Whether the editor can show `r` as fields: a way PM sends, in Google's range (and at least a day
 *  before on an all-day event). Anything else Google holds is one line, kept or removed. */
export const isEditable = (r: Reminder, allDay: boolean) =>
  isKnownMethod(r.method) && rowProblem(r.minutes, allDay) === null;

/** The same notification twice on one list: Google would keep one or refuse the save. */
export const ALREADY_LISTED = "Already on the list.";

const sameOne = (a: Reminder, b: Reminder) => a.method === b.method && a.minutes === b.minutes;

/** Whether `r` is on `list`. */
export const listed = (list: readonly Reminder[], r: Reminder) => list.some((x) => sameOne(x, r));

const sortKey = (r: Reminder) => `${r.method}\u001f${String(r.minutes).padStart(12, "0")}`;

/** The same notifications, in any order (mirrors the backend's `reminders_eq`): the defaults are
 *  the defaults, whatever list came with them. */
export function sameReminders(a: RemindersDraft, b: RemindersDraft): boolean {
  if (a.use_default || b.use_default) return a.use_default === b.use_default;
  if (a.overrides.length !== b.overrides.length) return false;
  const x = a.overrides.map(sortKey).sort();
  const y = b.overrides.map(sortKey).sort();
  return x.every((k, i) => k === y[i]);
}

/** The calendar's defaults, as a value: a new one each time, so no draft shares it with another
 *  (C13 critique C1-L13). */
export const calendarDefaults = (): RemindersDraft => ({ use_default: true, overrides: [] });

/** A copy sharing nothing with `r`. */
export const copyReminders = (r: RemindersDraft): RemindersDraft => ({
  use_default: r.use_default,
  overrides: r.overrides.map((o) => ({ ...o })),
});

/** A new row, as Google web starts one (INFERRED): "30 minutes before" on a timed event, "1 day
 *  before at 09:00" on an all-day one; the next free one when that's on the list already. */
export function newRow(list: readonly Reminder[], allDay: boolean): Reminder {
  const tries = allDay ? [900, 2340, 3780, 9540, 1440, 2880] : [30, 10, 60, 1440, 0, 15, 120];
  const minutes = tries.find((m) => !listed(list, { method: "popup", minutes: m })) ?? tries[0];
  return { method: "popup", minutes };
}

/** "Work's" in a sentence, or "the calendar's" (`start`: "The calendar's") when it has no name. */
function owner(name: string | null, start = false): string {
  if (name) return `${name}'s`;
  return start ? "The calendar's" : "the calendar's";
}

export const defaultsText = (name: string | null) => `${owner(name, true)} default notifications:`;

export const noDefaultsText = (name: string | null) =>
  `${name ?? "The calendar"} has no default notifications.`;

export const allDayDefaultsText = (name: string | null) =>
  `${owner(name, true)} default notifications for all-day events.`;

/** The button that puts the defaults back. */
export const defaultsButtonLabel = (name: string | null) => `Use ${owner(name)} defaults`;

/** Said under the list after a switch between timed and all-day put the defaults back. */
export const kindSwitchText = (name: string | null, allDay: boolean) =>
  `Notifications went back to ${owner(name)} defaults for ${allDay ? "all-day" : "timed"} events.`;

/** What a kind switch that replaced a list says under it (`remindersAfterKindSwitch`): the list went
 *  back to the calendar's defaults, or to the one the event has in Google. */
export type KindSwitchNote = "defaults" | "event";

/** A kind switch's note in words, on `name`'s calendar, the event now all-day or not. Back to the
 *  event's own list in Google over one the user made (C13 review round 1), or to the defaults. */
export const kindNoteText = (note: KindSwitchNote, name: string | null, allDay: boolean) =>
  note === "event"
    ? "Notifications went back to the ones this event has in Google."
    : kindSwitchText(name, allDay);
