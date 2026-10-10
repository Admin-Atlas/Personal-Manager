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
import { resolveWallTime } from "../wallTime";

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
