// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The event editor's draft, as pure functions (#884). The editor opens on Google's fresh copy
// (`EventForEdit`, never the mirror), `seedDraft` turns that into the fields it edits, and
// `toChanges` sends back only what the user changed: the backend drops anything Google already
// holds, but sending an untouched field at all would risk overwriting a change made meanwhile.
//
// `checkDraft` catches what can be caught before a round trip (an end before its start, a time the
// clocks skip), mirroring the backend's `calendar_write::time`, which has the final say. `rebaseDraft`
// carries a draft onto a newer copy after a conflict: the user's edits stay, except where Google
// changed the same field, where Google's value wins and the editor says so.

import type {
  EventForEdit,
  EventPatchDraft,
  EventVisibility,
  FieldPermissions,
  ShowAs,
  TimeDraft,
} from "../types";
import { resolveWallTime, wallInstant, wallTimeOf } from "../wallTime";
import { durationLabel, timeChoices } from "./timeSlots";

/** What the editor's fields hold. */
export interface EditorFields {
  summary: string;
  location: string;
  description: string;
  time: TimeDraft;
  show_as: ShowAs;
  visibility: EventVisibility;
}

export type EditorField = keyof EditorFields;

/** Every field, in the order the editor shows them. */
export const EDITOR_FIELDS: readonly EditorField[] = [
  "summary",
  "time",
  "location",
  "description",
  "show_as",
  "visibility",
];

/** The editor's starting fields: Google's copy as it is. */
export function seedDraft(event: EventForEdit): EditorFields {
  return {
    summary: event.summary,
    location: event.location,
    description: event.description,
    time: { ...event.time },
    show_as: event.show_as,
    visibility: event.visibility,
  };
}

/** A textarea hands back `\n` whatever Google stored, so `\r\n` and `\n` are the same text: without
 *  this, opening an event whose description has Windows line endings and saving the title would send
 *  the description too. */
const sameText = (a: string, b: string) => a.replace(/\r\n?/g, "\n") === b.replace(/\r\n?/g, "\n");

function sameTime(a: TimeDraft, b: TimeDraft): boolean {
  if (a.kind === "all_day" && b.kind === "all_day") {
    return a.first_day === b.first_day && a.last_day === b.last_day;
  }
  if (a.kind === "timed" && b.kind === "timed") {
    return (
      a.start_date === b.start_date &&
      a.start_time === b.start_time &&
      a.start_zone === b.start_zone &&
      a.end_date === b.end_date &&
      a.end_time === b.end_time &&
      a.end_zone === b.end_zone
    );
  }
  return false;
}

/** Whether `field` holds the same value in `a` and `b`. */
export function sameField(field: EditorField, a: EditorFields, b: EditorFields): boolean {
  switch (field) {
    case "summary":
    case "location":
    case "description":
      return sameText(a[field], b[field]);
    case "time":
      return sameTime(a.time, b.time);
    case "show_as":
    case "visibility":
      return a[field] === b[field];
  }
}

/** The fields `draft` changed from `base`, in editor order. */
export function changedFields(base: EditorFields, draft: EditorFields): EditorField[] {
  return EDITOR_FIELDS.filter((f) => !sameField(f, base, draft));
}

/** What a save sends: the changed fields only, everything else left out (untouched). */
export function toChanges(base: EditorFields, draft: EditorFields): EventPatchDraft {
  const changes: EventPatchDraft = {};
  for (const f of changedFields(base, draft)) {
    switch (f) {
      case "summary":
      case "location":
      case "description":
        changes[f] = draft[f];
        break;
      case "time":
        changes.time = { ...draft.time };
        break;
      case "show_as":
        changes.show_as = draft.show_as;
        break;
      case "visibility":
        changes.visibility = draft.visibility;
        break;
    }
  }
  return changes;
}

/** Whether the editor may change `field` (the permission names match the fields). */
export function canEdit(permissions: FieldPermissions, field: EditorField): boolean {
  return permissions[field];
}

/** Something that stops a save before it is sent. */
export type DraftProblem =
  | { kind: "end_before_start" }
  /** The clocks skip that time in that zone. */
  | { kind: "gap"; half: "start" | "end"; zone: string; time: string }
  | { kind: "unreadable"; half: "start" | "end"; what: "zone" | "date" | "time" };

export interface DraftCheck {
  problems: DraftProblem[];
  /** Halves the user changed to a time the clocks pass twice that night: the save takes the first. */
  ambiguous: ("start" | "end")[];
}

/** Google's exact instants for the copy the base was seeded from (`EventForEdit` carries them). */
export interface HeldTimes {
  start_at: string | null;
  end_at: string | null;
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

/** What would stop `draft` saving, judged as the backend will judge it:
 *  - a time the user didn't change isn't sent, so it can't stop a save (a 0-minute event, or a zone
 *    this webview doesn't know, never blocks a title edit);
 *  - a half left as Google holds it keeps Google's exact instant (`held`, from the base's
 *    `EventForEdit`). Without one, either occurrence of a repeated hour is allowed, so the check only
 *    fails when no choice works;
 *  - a half the user changed takes its first occurrence, and a skipped time is refused. */
export function checkDraft(base: EditorFields, draft: EditorFields, held?: HeldTimes): DraftCheck {
  const problems: DraftProblem[] = [];
  const ambiguous: ("start" | "end")[] = [];
  if (sameTime(base.time, draft.time)) return { problems, ambiguous };
  const t = draft.time;
  if (t.kind === "all_day") {
    if (!ISO_DATE.test(t.first_day) || !ISO_DATE.test(t.last_day)) {
      problems.push({
        kind: "unreadable",
        half: ISO_DATE.test(t.first_day) ? "end" : "start",
        what: "date",
      });
    } else if (t.last_day < t.first_day) {
      problems.push({ kind: "end_before_start" });
    }
    return { problems, ambiguous };
  }
  const was = base.time.kind === "timed" ? base.time : null;
  const instants = (half: "start" | "end"): number[] | null => {
    const [date, time, zone] =
      half === "start"
        ? [t.start_date, t.start_time, t.start_zone]
        : [t.end_date, t.end_time, t.end_zone];
    const untouched =
      was !== null &&
      (half === "start"
        ? was.start_date === date && was.start_time === time && was.start_zone === zone
        : was.end_date === date && was.end_time === time && was.end_zone === zone);
    if (untouched) {
      // The save keeps Google's own instant for this half, so nothing about it can be refused.
      const exact = Date.parse((half === "start" ? held?.start_at : held?.end_at) ?? "");
      if (!Number.isNaN(exact)) return [exact];
      const r = resolveWallTime(date, time, zone);
      if (r.kind === "ok") return [r.instant.getTime()];
      if (r.kind === "ambiguous") return [r.instant.getTime(), r.later.getTime()];
      return null; // unreadable here, but Google's node is sent as it is
    }
    const r = resolveWallTime(date, time, zone);
    switch (r.kind) {
      case "ok":
        return [r.instant.getTime()];
      case "ambiguous":
        ambiguous.push(half);
        return [r.instant.getTime()];
      case "gap":
        problems.push({ kind: "gap", half, zone, time });
        return null;
      case "invalid":
        problems.push({ kind: "unreadable", half, what: r.what });
        return null;
    }
  };
  const start = instants("start");
  const end = instants("end");
  if (start && end && Math.max(...end) <= Math.min(...start)) {
    problems.push({ kind: "end_before_start" });
  }
  return { problems, ambiguous };
}

type Timed = Extract<TimeDraft, { kind: "timed" }>;

/** The instant one half of a timed draft names, as a save would read it: Google's exact instant while
 *  the half is as Google holds it (it may be the second of a repeated hour), else its first
 *  occurrence. `null` for a time the clocks skip, or one that can't be read. */
export function halfInstant(
  base: EditorFields,
  t: Timed,
  half: "start" | "end",
  held?: HeldTimes,
): number | null {
  const was = base.time.kind === "timed" ? base.time : null;
  const [date, time, zone] =
    half === "start"
      ? [t.start_date, t.start_time, t.start_zone]
      : [t.end_date, t.end_time, t.end_zone];
  const untouched =
    was !== null &&
    (half === "start"
      ? was.start_date === date && was.start_time === time && was.start_zone === zone
      : was.end_date === date && was.end_time === time && was.end_zone === zone);
  if (untouched) {
    const exact = Date.parse((half === "start" ? held?.start_at : held?.end_at) ?? "");
    if (!Number.isNaN(exact)) return exact;
  }
  return wallInstant(date, time, zone)?.getTime() ?? null;
}

/** One choice in the End time list. */
export interface EndChoice {
  value: string;
  /** The time, with the event's length when it ends on the day it starts: "10:30 (30 mins)". */
  label: string;
  /** Whether it ends the event after its start (false only for the current end, kept on offer). */
  after: boolean;
}

/** The End time list for a timed draft: only times after the start, each labelled with the length
 *  the event would have when it ends on the start's day, as Google's own list does. `heldEnd` (the end
 *  Google holds) is offered like any other time, and the current end always stays in the list even
 *  when it isn't after the start (a select must hold its value; Google can hold a 0-minute event), so
 *  the live problem line, not a silently changed field, says what's wrong. A start that can't be read
 *  (a skipped time) leaves the list unfiltered: the problem is the start's, and is reported there. */
export function endTimeChoices(
  base: EditorFields,
  t: Timed,
  held?: HeldTimes,
  heldEnd?: string,
): EndChoice[] {
  const all = timeChoices(t.end_time, heldEnd);
  const start = halfInstant(base, t, "start", held);
  const startDay = start === null ? null : wallDateOf(start, t.end_zone);
  if (start === null || startDay === null) {
    return all.map((hm) => ({ value: hm, label: hm, after: true }));
  }
  const sameDay = t.end_date === startDay;
  const out: EndChoice[] = [];
  for (const hm of all) {
    const at = halfInstant(base, { ...t, end_time: hm }, "end", held);
    if (at !== null && at > start) {
      out.push({
        value: hm,
        label: sameDay ? `${hm} (${durationLabel(at - start)})` : hm,
        after: true,
      });
    } else if (hm === t.end_time) {
      out.push({ value: hm, label: hm, after: false });
    }
  }
  return out;
}

/** The date `ms` falls on in `zone`, or `null` for a zone this webview doesn't know. */
function wallDateOf(ms: number, zone: string): string | null {
  try {
    return wallTimeOf(new Date(ms), zone).date;
  } catch {
    return null;
  }
}

/** The first day the End date can be: the start's day as the END zone reads it, since a start just
 *  after midnight in London is still the day before in New York. A start too late for any end on
 *  that day (23:45 or later) makes it the day after: offering that day would let the field show a
 *  date the event can't end on. */
export function endDateMin(base: EditorFields, t: Timed, held?: HeldTimes): string {
  const start = halfInstant(base, t, "start", held);
  const day = (start === null ? null : wallDateOf(start, t.end_zone)) ?? t.start_date;
  if (start !== null && firstEndOnItsDate(base, { ...t, end_date: day }, held) === null) {
    const [y, m, d] = day.split("-").map(Number);
    return new Date(Date.UTC(y, m - 1, d + 1)).toISOString().slice(0, 10);
  }
  return day;
}

/** The length an event gets when its own can't be kept: a whole hour, as All day switched off gives. */
export const DEFAULT_LENGTH_MS = 60 * 60_000;

const QUARTER_MS = 15 * 60_000;

/** `t` with its end put `lengthMs` after its start, in the end's zone (unchanged if the start, or
 *  the end's zone, can't be read). A wall time names whole minutes, and in an hour the clocks pass
 *  twice it names only the FIRST pass (as every reader, and the backend, resolve it), so an end that
 *  falls in the second pass would read back an hour early, maybe before the start: it moves on, a
 *  quarter hour at a time, to the first wall time that reads back where it should (02:00 GMT on the
 *  night London's clocks go back). The event is a little longer, never shorter. */
export function endAfterStart(
  base: EditorFields,
  t: Timed,
  lengthMs: number,
  held?: HeldTimes,
): Timed {
  const start = halfInstant(base, t, "start", held);
  if (start === null) return t;
  const target = Math.ceil((start + Math.max(lengthMs, 60_000)) / 60_000) * 60_000;
  try {
    let at = target;
    // A fold is at most a few hours long; past that, keep the last try rather than loop.
    for (let i = 0; i < 16; i++) {
      const end = wallTimeOf(new Date(at), t.end_zone);
      const back = wallInstant(end.date, end.time, t.end_zone)?.getTime();
      if (back === undefined || back >= target || i === 15) {
        return { ...t, end_date: end.date, end_time: end.time };
      }
      at = Math.floor(at / QUARTER_MS) * QUARTER_MS + QUARTER_MS;
    }
  } catch {
    // A zone this webview doesn't know.
  }
  return t;
}

/** The first end on `t`'s own end date that comes after its start: the time that end date can keep.
 *  `null` when that whole date is over before the start (a start at 23:50 on it). */
export function firstEndOnItsDate(base: EditorFields, t: Timed, held?: HeldTimes): Timed | null {
  const first = endTimeChoices(base, t, held).find((c) => c.after);
  return first ? { ...t, end_time: first.value } : null;
}

/** Whether a timed draft's end comes after its start (an unreadable half counts as fine here: the
 *  problem line names it). */
export function endsAfterStart(base: EditorFields, t: Timed, held?: HeldTimes): boolean {
  const start = halfInstant(base, t, "start", held);
  const end = halfInstant(base, t, "end", held);
  return start === null || end === null || end > start;
}

/** A draft carried onto Google's newer copy after a conflict. */
export interface Rebased {
  /** The new base, for the next diff. */
  base: EditorFields;
  draft: EditorFields;
  /** Fields the user had changed that Google changed too: Google's value is now in the draft. */
  overwritten: EditorField[];
}

/** Carry `draft` (made against `oldBase`) onto `newBase`, Google's copy now. A field only the user
 *  changed keeps the user's value; a field only Google changed takes Google's; a field both changed
 *  takes Google's (it can't be saved over unseen) and is listed, unless both made the same change. */
export function rebaseDraft(
  oldBase: EditorFields,
  newBase: EditorFields,
  draft: EditorFields,
): Rebased {
  const next: EditorFields = { ...newBase, time: { ...newBase.time } };
  const overwritten: EditorField[] = [];
  for (const f of EDITOR_FIELDS) {
    const mine = !sameField(f, oldBase, draft);
    if (!mine) continue;
    const theirs = !sameField(f, oldBase, newBase);
    if (!theirs) {
      setField(next, f, draft);
    } else if (!sameField(f, draft, newBase)) {
      overwritten.push(f);
    }
  }
  return { base: newBase, draft: next, overwritten };
}

function setField(target: EditorFields, field: EditorField, from: EditorFields) {
  switch (field) {
    case "summary":
    case "location":
    case "description":
      target[field] = from[field];
      break;
    case "time":
      target.time = { ...from.time };
      break;
    case "show_as":
      target.show_as = from.show_as;
      break;
    case "visibility":
      target.visibility = from.visibility;
      break;
  }
}
