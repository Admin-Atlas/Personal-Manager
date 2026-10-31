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
// changed the same field, where Google's value wins and the editor says so. How an event repeats is
// one more field (C11): it starts as its repeating event's rule (`repeatSeed`), and a change to it is
// sent as the rule chosen or "Does not repeat".
//
// Creating an event uses the same fields: `newEventSeed` starts one, it's judged against
// `NEW_EVENT_BASE` (nothing to keep, so every half is checked), and `toNewEvent` sends all of it,
// its repeat (C11) and its guests (C12d) included.
//
// Guests are fields too (C12): the list, compared by who is on it and who is optional (a reply is
// never a change), sent as changes to Google's own list, never the list; and the three guest
// permissions. A rebase merges the guest list rather than taking Google's whole (`rebaseDraft`).
//
// So are notifications and colour (C13): the notifications go whole, compared in any order, and a
// colour is the calendar's or one of Google's eleven. Both are the user's own, so a change to them
// emails no guest.
//
// And the video call (C18): whether the event will have a Google Meet call after Save, sent as Add
// or Remove; Google emails guests about it like any shared field. A new event sends whether it
// asks for one (C18b).

import type {
  ColourChoice,
  EventForEdit,
  EventPatchDraft,
  EventVisibility,
  FieldPermissions,
  GuestForEdit,
  GuestNotice,
  GuestOp,
  GuestState,
  NewEventDraft,
  RemindersDraft,
  SeriesInfo,
  ShowAs,
  TimeDraft,
} from "../types";
import { resolveWallTime, wallInstant, wallTimeOf } from "../wallTime";
import { sameDescription } from "./descriptionHtml";
import { addressKey, changeNotice, isEmail } from "./guests";
import { calendarDefaults, copyReminders, sameReminders, type KindSwitchNote } from "./reminders";
import {
  copyRule,
  followStartDate,
  ONCE,
  sameRepeat,
  startDateOf,
  type RepeatValue,
} from "./recurrenceDraft";
import { durationLabel, timeChoices } from "./timeSlots";

/** One guest as the editor holds it: Google's entry as the editor opened it, or one the user added
 *  (no name or reply yet). */
export type GuestDraft = GuestForEdit;

/** What guests may do, Google's three "Guest permissions" boxes: change the event, invite others,
 *  see the guest list. */
export interface GuestPerms {
  modify: boolean;
  invite_others: boolean;
  see_others: boolean;
}

/** Google's own: guests may invite others and see the list, not change the event. */
const GOOGLE_PERMS: GuestPerms = { modify: false, invite_others: true, see_others: true };

/** What the editor's fields hold. */
export interface EditorFields {
  summary: string;
  location: string;
  description: string;
  time: TimeDraft;
  /** How it repeats: the Repeat row. */
  recurrence: RepeatValue;
  /** Who is on it, the organiser and the user included, in Google's order then the order added. */
  guests: GuestDraft[];
  guest_perms: GuestPerms;
  /** The event will have a Google Meet call after Save (C18). */
  meet: boolean;
  /** The calendar's default notifications, or the event's own list (C13). */
  reminders: RemindersDraft;
  /** The calendar's colour, or one of Google's eleven (C13). */
  colour: ColourChoice;
  show_as: ShowAs;
  visibility: EventVisibility;
  /** The organiser's address, on the list or not (Google's copy; none on a new event). Not a field:
   *  nothing edits it, and no guest change touches it, as the backend's `check` has it. */
  organizer_email?: string | null;
}

export type EditorField = Exclude<keyof EditorFields, "organizer_email">;

/** Every field, in the order the editor shows them: guests after the time, as Google's quick
 *  create has them (INFERRED); the video call after the guests, above the place, as Google's event
 *  page puts its Meet button (INFERRED); notifications and colour under the location, as Google's
 *  event page has them (INFERRED). */
export const EDITOR_FIELDS: readonly EditorField[] = [
  "summary",
  "time",
  "recurrence",
  "guests",
  "guest_perms",
  "meet",
  "location",
  "reminders",
  "colour",
  "description",
  "show_as",
  "visibility",
];

/** Fields that are the user's own, so a change to only them emails no guest: busy or free,
 *  notifications and colour (the backend's `PERSONAL`). */
const PERSONAL: readonly EditorField[] = ["show_as", "reminders", "colour"];

/** No colour of its own: it shows in its calendar's. */
const CALENDAR_COLOUR: ColourChoice = { kind: "calendar" };

/** A colour choice sharing nothing with `c`. */
const copyColour = (c: ColourChoice): ColourChoice =>
  c.kind === "colour" ? { kind: "colour", id: c.id } : { kind: "calendar" };

/** The editor's starting fields: Google's copy as it is, and how it repeats (`repeatSeed`), which a
 *  one-off has none of. Its colour is its `colorId`, any Google holds (one PM doesn't know is shown
 *  as that and sent only if changed); a label's colour isn't PM's to set, so a label leaves the
 *  colour field as `colorId` has it, read-only (the backend's `LabelReadOnly`). It has a Meet call
 *  when Google holds one, made or still being made; a request Google failed is none (C18). */
export function seedDraft(event: EventForEdit, repeat: RepeatValue = ONCE): EditorFields {
  return {
    summary: event.summary,
    location: event.location,
    description: event.description,
    time: { ...event.time },
    recurrence: repeat,
    guests: event.guests.map((g) => ({ ...g })),
    guest_perms: {
      modify: event.guests_can_modify,
      invite_others: event.guests_can_invite_others,
      see_others: event.guests_can_see_other_guests,
    },
    meet: event.call?.kind === "meet" && event.call.state !== "failed",
    reminders: copyReminders(event.reminders),
    colour: event.color_id ? { kind: "colour", id: event.color_id } : { kind: "calendar" },
    show_as: event.show_as,
    visibility: event.visibility,
    organizer_email: event.organizer_email,
  };
}

/** How the Repeat row of an existing event starts (C11): its repeating event's rule when it is one
 *  of the menu's, else that event's own words, which can only be replaced; "Repeats" for an
 *  occurrence whose repeating event PM couldn't read. A repeating event never starts as "Does not
 *  repeat", which would read as a change to it; only a one-off does. */
export function repeatSeed(series: SeriesInfo | null, repeating: boolean): RepeatValue {
  if (series?.rule) return { kind: "rule", rule: copyRule(series.rule) };
  if (series) return { kind: "kept", words: series.repeats };
  return repeating ? { kind: "kept", words: "Repeats" } : ONCE;
}

/** A field hands back `\n` whatever Google stored, so `\r\n` and `\n` are the same text: without
 *  this, opening an event whose location has Windows line endings and saving the title would send
 *  the location too. The description's own comparison (`sameDescription`) starts the same way. */
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

/** Who is on a guest list, as `address|optional` (lower-cased), sorted and once each: replies,
 *  names and order never count (R2: someone's reply is not a change). */
function guestSet(guests: readonly GuestDraft[]): string[] {
  return [...new Set(guests.map((g) => `${addressKey(g.email)}|${g.optional}`))].sort();
}

const sameGuests = (a: readonly GuestDraft[], b: readonly GuestDraft[]) => {
  const x = guestSet(a);
  const y = guestSet(b);
  return x.length === y.length && x.every((v, i) => v === y[i]);
};

const PERMS: readonly (keyof GuestPerms)[] = ["modify", "invite_others", "see_others"];

/** Whether `field` holds the same value in `a` and `b`. */
export function sameField(field: EditorField, a: EditorFields, b: EditorFields): boolean {
  switch (field) {
    case "summary":
    case "location":
      return sameText(a[field], b[field]);
    // By what it shows (C19): Google's `<html-blob>` wrapper, `<strong>` for `<b>`, or `<br>` for a
    // line are no change, so an editor that only opened a formatted description never sends it.
    case "description":
      return sameDescription(a.description, b.description);
    case "time":
      return sameTime(a.time, b.time);
    case "recurrence":
      return sameRepeat(a.recurrence, b.recurrence);
    case "guests":
      return sameGuests(a.guests, b.guests);
    case "guest_perms":
      return PERMS.every((p) => a.guest_perms[p] === b.guest_perms[p]);
    // Google may hand the list back in another order than it was sent.
    case "reminders":
      return sameReminders(a.reminders, b.reminders);
    case "colour":
      return (
        a.colour.kind === b.colour.kind &&
        (a.colour.kind !== "colour" || b.colour.kind !== "colour" || a.colour.id === b.colour.id)
      );
    case "meet":
    case "show_as":
    case "visibility":
      return a[field] === b[field];
  }
}

const stateOf = (g: GuestDraft | undefined): GuestState =>
  g === undefined ? "absent" : g.optional ? "optional" : "required";

const byAddress = (guests: readonly GuestDraft[]) =>
  new Map(guests.map((g) => [addressKey(g.email), g] as const));

/** The guest list's changes from `base` to `draft`, as Google's list should end up: one op per
 *  address added, taken off or made optional or required, in the base's spelling where it has one,
 *  sorted by lower-cased address so the same change always reads the same. Never one on the
 *  organiser's or the user's own entry, by its flags or the organiser's address (`organizer`)
 *  whether the list carries it or not: Google's copy would refuse the whole save (the backend's
 *  `check`, which looks at both). */
function guestOps(
  base: readonly GuestDraft[],
  draft: readonly GuestDraft[],
  organizer: string | null | undefined,
): GuestOp[] {
  const before = byAddress(base);
  const after = byAddress(draft);
  const theOrganiser = organizer ? addressKey(organizer) : null;
  const ops: GuestOp[] = [];
  for (const key of [...new Set([...before.keys(), ...after.keys()])].sort()) {
    const was = before.get(key);
    const now = after.get(key);
    const entry = was ?? now;
    if (!entry || key === theOrganiser) continue;
    if (was?.organizer || was?.self || now?.organizer || now?.self) continue;
    const to = stateOf(now);
    if (to !== stateOf(was)) ops.push({ email: entry.email.trim(), to });
  }
  return ops;
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
      // The rule chosen, or "Does not repeat". A rule PM can only show is the event's own, never
      // one changed to, so there's nothing of it to send.
      case "recurrence": {
        const r = draft.recurrence;
        if (r.kind === "rule") changes.recurrence = { kind: "rule", rule: copyRule(r.rule) };
        else if (r.kind === "once") changes.recurrence = { kind: "once" };
        break;
      }
      // Changes to Google's own list, never the list (R1): the backend applies them to its fresh
      // copy, so a reply that landed meanwhile is kept.
      case "guests": {
        const ops = guestOps(base.guests, draft.guests, base.organizer_email);
        if (ops.length > 0) changes.guests = ops;
        break;
      }
      // Only the boxes that changed.
      case "guest_perms": {
        const [was, now] = [base.guest_perms, draft.guest_perms];
        if (was.modify !== now.modify) changes.guests_can_modify = now.modify;
        if (was.invite_others !== now.invite_others) {
          changes.guests_can_invite_others = now.invite_others;
        }
        if (was.see_others !== now.see_others) {
          changes.guests_can_see_other_guests = now.see_others;
        }
        break;
      }
      // The whole list, or the defaults: Google replaces a list it is sent (R1).
      case "reminders":
        changes.reminders = copyReminders(draft.reminders);
        break;
      case "colour":
        changes.colour = copyColour(draft.colour);
        break;
      // Add or Remove, never the call itself: the backend asks Google for a new one (C18).
      case "meet":
        changes.meet = draft.meet ? "add" : "remove";
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

/** The save's changes, marked as the user's "Replace with plain text…" when they replaced a
 *  description and the save sends one (C19). The mark reopens only a description whose formatting PM
 *  can't edit (the backend's `replacing_formatting`), so it never goes without the description it is
 *  about: a save of the title alone after a Replace that was then undone by hand carries none. */
export function withReplace(changes: EventPatchDraft, replaced: boolean): EventPatchDraft {
  return replaced && changes.description !== undefined
    ? { ...changes, replace_formatting: true }
    : changes;
}

/** Who saving `draft` over `base` would email, judged as the backend judges it (its
 *  `notify_required`, the same table in both tests): null when nobody. A change to only busy or
 *  free, notifications or colour is the user's own and emails nobody. `omitted`: Google cut the list
 *  short, so "at least". `me`: the account saving, never a guest even where Google's copy doesn't
 *  mark it. `calendarChanged`: the same Save puts it on another calendar, which tells its guests too
 *  (C18d). The backend has the last word; this only lets the editor ask before a round trip. */
export function guestNotice(
  base: EditorFields,
  draft: EditorFields,
  omitted: boolean,
  me: string | null = null,
  calendarChanged = false,
): GuestNotice | null {
  const shared = changedFields(base, draft).some((f) => !PERSONAL.includes(f));
  return changeNotice(base.guests, draft.guests, shared, omitted, me, calendarChanged);
}

/** Whether the editor may change `field` (the permission names match the fields). How it repeats
 *  has its own right: Google locks `recurrence` exactly when it locks the start, and on someone
 *  else's event it stays the organiser's even where guests may change the time (C14 critiques
 *  C1-H2, C2-H2). The guest list and its permissions are one right. */
export function canEdit(permissions: FieldPermissions, field: EditorField): boolean {
  switch (field) {
    case "recurrence":
      return permissions.repeat;
    case "guests":
    case "guest_perms":
      return permissions.guests;
    default:
      return permissions[field];
  }
}

/** Something that stops a save before it is sent. */
export type DraftProblem =
  | { kind: "end_before_start" }
  /** The clocks skip that time in that zone. */
  | { kind: "gap"; half: "start" | "end"; zone: string; time: string }
  | { kind: "unreadable"; half: "start" | "end"; what: "zone" | "date" | "time" }
  /** A guest added whose address isn't one (the field never adds one; a paste or a hand-over
   *  could). */
  | { kind: "bad_guest"; email: string };

export interface DraftCheck {
  problems: DraftProblem[];
  /** Halves the user changed to a time the clocks pass twice that night: the save takes the first. */
  ambiguous: ("start" | "end")[];
}

/** Text a date field couldn't read, by the half of the event it sets (Start date or First day, End
 *  date or Last day). Each stands until that half's field is given a date, typed or picked, and a
 *  save is refused while either does: another date, a time or a blur never ends it, or the save
 *  would go on the very date the user was changing. */
export type BadDates = Readonly<Record<"start" | "end", string | null>>;

export const NO_BAD_DATES: BadDates = { start: null, end: null };

export const hasBadDate = (bad: BadDates) => bad.start !== null || bad.end !== null;

/** The id of the date field setting `half`, in the When fields given `when` as their id. */
export const dateFieldId = (when: string, half: "start" | "end") => `${when}-${half}-date`;

/** Where a save refused for `bad` puts focus: the date field whose refusal stands, the start's
 *  first (each half has one, whatever the event's kind). A blur there gives it nothing, so only a
 *  date typed or picked in it ends the refusal. */
export const refusedDateId = (when: string, bad: BadDates) =>
  dateFieldId(when, bad.start !== null ? "start" : "end");

/** Google's exact instants for the copy the base was seeded from (`EventForEdit` carries them). */
export interface HeldTimes {
  start_at: string | null;
  end_at: string | null;
}

const ISO_DATE = /^\d{4}-\d{2}-\d{2}$/;

/** What would stop `draft` saving, judged as the backend will judge it:
 *  - a guest added whose address isn't one (Google's own entries are never judged);
 *  - a time the user didn't change isn't sent, so it can't stop a save (a 0-minute event, or a zone
 *    this webview doesn't know, never blocks a title edit);
 *  - a half left as Google holds it keeps Google's exact instant (`held`, from the base's
 *    `EventForEdit`). Without one, either occurrence of a repeated hour is allowed, so the check only
 *    fails when no choice works;
 *  - a half the user changed takes its first occurrence, and a skipped time is refused. */
export function checkDraft(base: EditorFields, draft: EditorFields, held?: HeldTimes): DraftCheck {
  const problems: DraftProblem[] = [];
  const ambiguous: ("start" | "end")[] = [];
  const had = byAddress(base.guests);
  for (const g of draft.guests) {
    if (!had.has(addressKey(g.email)) && !isEmail(g.email)) {
      problems.push({ kind: "bad_guest", email: g.email });
    }
  }
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
  /** For the guest list, which merges: each guest whose change of the user's was dropped, because
   *  Google changed that guest too, by name or else address. */
  lost_guests: string[];
}

/** Carry `draft` (made against `oldBase`) onto `newBase`, Google's copy now. A field only the user
 *  changed keeps the user's value; a field only Google changed takes Google's; a field both changed
 *  takes Google's (it can't be saved over unseen) and is listed, unless both made the same change.
 *  The guest list and its permissions merge instead (`rebaseGuests`, `rebasePerms`). */
export function rebaseDraft(
  oldBase: EditorFields,
  newBase: EditorFields,
  draft: EditorFields,
): Rebased {
  const next: EditorFields = {
    ...newBase,
    time: { ...newBase.time },
    guests: newBase.guests.map((g) => ({ ...g })),
    guest_perms: { ...newBase.guest_perms },
    reminders: copyReminders(newBase.reminders),
    colour: copyColour(newBase.colour),
  };
  const overwritten: EditorField[] = [];
  let lost_guests: string[] = [];
  for (const f of EDITOR_FIELDS) {
    const mine = !sameField(f, oldBase, draft);
    if (!mine) continue;
    const theirs = !sameField(f, oldBase, newBase);
    if (f === "guests") {
      lost_guests = rebaseGuests(oldBase, newBase, draft, theirs, next);
      if (lost_guests.length > 0) overwritten.push(f);
    } else if (f === "guest_perms") {
      rebasePerms(oldBase, draft, next);
    } else if (!theirs) {
      setField(next, f, draft);
    } else if (!sameField(f, draft, newBase)) {
      overwritten.push(f);
    }
  }
  return { base: newBase, draft: next, overwritten, lost_guests };
}

/** The user's guest list carried onto Google's newer one, into `next`; each guest whose change of
 *  the user's was dropped, by name or else address. Only the user changed it: the user's list, each
 *  name and reply as Google has them now (someone's reply is no conflict). Both changed it: the
 *  user's changes made again on Google's list, except one to a guest Google changed too, which is
 *  dropped (Google's stands, R2); a guest the user added keeps the name the user saw. */
function rebaseGuests(
  oldBase: EditorFields,
  newBase: EditorFields,
  draft: EditorFields,
  theirs: boolean,
  next: EditorFields,
): string[] {
  const fresh = byAddress(newBase.guests);
  if (!theirs) {
    next.guests = draft.guests.map((g) => {
      const now = fresh.get(addressKey(g.email));
      return now ? { ...g, name: now.name, response: now.response } : { ...g };
    });
    return [];
  }
  const old = byAddress(oldBase.guests);
  const mine = byAddress(draft.guests);
  const lost: string[] = [];
  for (const op of guestOps(oldBase.guests, draft.guests, oldBase.organizer_email)) {
    const key = addressKey(op.email);
    const google = stateOf(fresh.get(key));
    if (google !== stateOf(old.get(key)) && google !== op.to) {
      const seen = old.get(key) ?? mine.get(key) ?? fresh.get(key);
      lost.push(seen?.name?.trim() || op.email);
      continue;
    }
    if (op.to === "absent") {
      next.guests = next.guests.filter((g) => addressKey(g.email) !== key);
    } else if (fresh.has(key)) {
      next.guests = next.guests.map((g) =>
        addressKey(g.email) === key ? { ...g, optional: op.to === "optional" } : g,
      );
    } else {
      const added = mine.get(key);
      if (added) next.guests = [...next.guests, { ...added, optional: op.to === "optional" }];
    }
  }
  return lost;
}

/** The guest permissions, each as a field of its own, into `next` (Google's): a box the user
 *  changed takes the user's state. A box has two states, so one both sides changed was changed
 *  the same way, and nothing of the user's is lost. */
function rebasePerms(oldBase: EditorFields, draft: EditorFields, next: EditorFields) {
  for (const p of PERMS) {
    if (draft.guest_perms[p] !== oldBase.guest_perms[p]) next.guest_perms[p] = draft.guest_perms[p];
  }
}

/** `date` (`YYYY-MM-DD`) moved by `days`. */
function shiftDay(date: string, days: number): string {
  const [y, m, d] = date.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + days)).toISOString().slice(0, 10);
}

/** A timed span as all day: the days it covers, an end at midnight closing the day before (so a
 *  23:00 to 00:00 event is one day), and never a last day before the first. */
export function allDayOf(t: Timed): Extract<TimeDraft, { kind: "all_day" }> {
  const last =
    t.end_time === "00:00" && t.end_date > t.start_date ? shiftDay(t.end_date, -1) : t.end_date;
  return {
    kind: "all_day",
    first_day: t.start_date,
    last_day: last < t.start_date ? t.start_date : last,
  };
}

// --- creating an event ---

/** What creating checks against: there's no time to keep, so every half counts as changed and is
 *  judged (`checkDraft` skips a time equal to its base's, and `halfInstant` keeps a base's untouched
 *  half). */
export const NEW_EVENT_BASE: EditorFields = {
  summary: "",
  location: "",
  description: "",
  time: { kind: "all_day", first_day: "", last_day: "" },
  recurrence: ONCE,
  guests: [],
  guest_perms: { ...GOOGLE_PERMS },
  meet: false,
  reminders: calendarDefaults(),
  colour: CALENDAR_COLOUR,
  show_as: "busy",
  visibility: "default",
};

/** How long a new event's draft lasts, in ms, for the creating boxes to keep while its start moves
 *  or it switches to all day and back: its own length when timed and a minute or more (a range
 *  dragged on the grid is the user's, C17), else `null`. */
export function newEventLength(d: EditorFields): number | null {
  const t = d.time;
  if (t.kind !== "timed") return null;
  const s = halfInstant(NEW_EVENT_BASE, t, "start");
  const e = halfInstant(NEW_EVENT_BASE, t, "end");
  return s !== null && e !== null && e - s >= 60_000 ? e - s : null;
}

const hhmm = (minutes: number) =>
  `${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;

/** The first time from `time` on `date`, a half hour at a time, that the clocks show in `zone`: a
 *  start the clocks skip that night (they go forward) moves on to when the hour is over. `null` when
 *  the rest of the day is skipped. */
function firstShown(date: string, time: string, zone: string): string | null {
  const [h, m] = time.split(":").map(Number);
  for (let at = h * 60 + m; at < 24 * 60; at += 30) {
    if (resolveWallTime(date, hhmm(at), zone).kind !== "gap") return hhmm(at);
  }
  return null;
}

/** A new event's starting fields, on `day` (`YYYY-MM-DD`, in `zone`): on today the next half hour
 *  strictly after `now` (14:10 → 14:30, 14:30 → 15:00, 23:40 → 00:00 the next day), on any other day
 *  09:00, stepping past a time the clocks skip; `lengthMs` long (`endAfterStart`, so a repeated hour
 *  never gives an end before the start). Busy, the calendar's default visibility, not repeating, no
 *  guests, Google's guest permissions, and the calendar's notifications and colour. With no zone at
 *  all there's no time to show, so it starts as an all-day event on `day`, free as Google's are. */
export function newEventSeed(
  day: string,
  now: Date,
  zone: string | null,
  lengthMs: number,
): EditorFields {
  const fields = {
    summary: "",
    location: "",
    description: "",
    recurrence: ONCE,
    guests: [],
    guest_perms: { ...GOOGLE_PERMS },
    // No video call: with the account's setting on, Meet starts with the first guest (C18b,
    // `meetDefault`), and a new event starts with none.
    meet: false,
    reminders: calendarDefaults(),
    colour: { kind: "calendar" } as const,
    visibility: "default" as const,
  };
  if (zone === null) {
    return { ...fields, time: { kind: "all_day", first_day: day, last_day: day }, show_as: "free" };
  }
  let date = day;
  let time = "09:00";
  let today: { date: string; time: string } | null = null;
  try {
    today = wallTimeOf(now, zone);
  } catch {
    // A zone this webview doesn't know: the live lines say so, and nothing can be saved.
  }
  if (today && today.date === day) {
    const [h, m] = today.time.split(":").map(Number);
    const next = (Math.floor((h * 60 + m) / 30) + 1) * 30;
    if (next >= 24 * 60) {
      date = shiftDay(day, 1);
      time = "00:00";
    } else {
      time = hhmm(next);
    }
  }
  let start = firstShown(date, time, zone);
  if (start === null) {
    date = shiftDay(date, 1);
    start = firstShown(date, "00:00", zone) ?? "00:00";
  }
  const timed: Timed = {
    kind: "timed",
    start_date: date,
    start_time: start,
    start_zone: zone,
    end_date: date,
    end_time: start,
    end_zone: zone,
  };
  return { ...fields, time: endAfterStart(NEW_EVENT_BASE, timed, lengthMs), show_as: "busy" };
}

/** All day ↔ timed for a new event: all day keeps the days the time covers (`allDayOf`, as editing
 *  does); timed goes back to the seed's start time on the first day, in `zone`, `lengthMs` long.
 *  Show as follows Google's default for the kind (all day free, timed busy) until the user picks
 *  it (`showAsTouched`). */
export function switchNewKind(
  draft: EditorFields,
  seed: EditorFields,
  zone: string,
  lengthMs: number,
  showAsTouched: boolean,
): EditorFields {
  const t = draft.time;
  if (t.kind === "timed") {
    return { ...draft, time: allDayOf(t), show_as: showAsTouched ? draft.show_as : "free" };
  }
  const from = seed.time.kind === "timed" ? seed.time.start_time : "09:00";
  const start = firstShown(t.first_day, from, zone) ?? from;
  const timed: Timed = {
    kind: "timed",
    start_date: t.first_day,
    start_time: start,
    start_zone: zone,
    end_date: t.first_day,
    end_time: start,
    end_zone: zone,
  };
  return {
    ...draft,
    time: endAfterStart(NEW_EVENT_BASE, timed, lengthMs),
    show_as: showAsTouched ? draft.show_as : "busy",
  };
}

/** A draft whose time just switched between timed and all-day (`draft`, with the new time), with
 *  its notifications fitted to the new kind: a list of its own goes back to the calendar's defaults,
 *  as Google web does (INFERRED, live probe E9f), since "30 minutes before" means nothing on an
 *  all-day event, which counts from the midnight before it. Back on the kind `base` has, a list that
 *  switch reset comes back as `base` has it (C13 critique C2-L5): the event never changed kind, so a
 *  save sends nothing for it. `reset` says whether the draft's defaults are the ones an earlier
 *  switch put there, untouched since; the result's `reset` says the same of the draft it returns,
 *  for the next switch. Defaults the user picked are theirs on either kind, so they stay (C13 review
 *  round 2). `note` says what the editor should say: that a list of the user's own was replaced, by
 *  the defaults or by the event's own list in Google (C13 review round 1). The defaults a switch put
 *  there going back off say nothing. Notifications the user can't change are never touched
 *  (C1-L14): the save would be refused. */
export function remindersAfterKindSwitch(
  base: EditorFields,
  draft: EditorFields,
  editable: boolean,
  reset: boolean,
): { draft: EditorFields; note: KindSwitchNote | null; reset: boolean } {
  if (!editable) return { draft, note: null, reset: false };
  if (draft.time.kind === base.time.kind) {
    if (draft.reminders.use_default && !reset) return { draft, note: null, reset: false };
    const replaced =
      !draft.reminders.use_default && !sameReminders(draft.reminders, base.reminders);
    return {
      draft: { ...draft, reminders: copyReminders(base.reminders) },
      note: !replaced ? null : base.reminders.use_default ? "defaults" : "event",
      reset: false,
    };
  }
  if (draft.reminders.use_default) return { draft, note: null, reset };
  return { draft: { ...draft, reminders: calendarDefaults() }, note: "defaults", reset: true };
}

/** `next`, a draft whose time just changed from `was`'s, with its rule following the start date
 *  (`followStartDate`): weekly on the old start's weekday becomes weekly on the new one's, and so on
 *  for a rule whose day came from the start. */
export function followStart(was: EditorFields, next: EditorFields): EditorFields {
  const r = next.recurrence;
  if (r.kind !== "rule") return next;
  const rule = followStartDate(r.rule, startDateOf(was.time), startDateOf(next.time));
  return rule === r.rule ? next : { ...next, recurrence: { kind: "rule", rule } };
}

/** What creating an event sends: every field, as the editor holds it, sharing nothing with the draft
 *  (what was sent is kept for a retry, and must not change with it). Its repeat is the rule chosen,
 *  or null for "Does not repeat"; the backend starts it on the first day the rule makes. Its guests
 *  are who and whether optional, in the order added (never an organiser's or the calendar's own
 *  entry, which only Google writes), and each guest permission goes only where it isn't Google's
 *  own. Its notifications are null on the calendar's defaults, and its colour null on the
 *  calendar's: Google then gives it the calendar's own (INFERRED, E9). Whether it asks for a Meet
 *  call (C18b): the backend mints the request. */
export function toNewEvent(draft: EditorFields): NewEventDraft {
  const sent: NewEventDraft = {
    summary: draft.summary,
    time: { ...draft.time },
    location: draft.location,
    description: draft.description,
    show_as: draft.show_as,
    visibility: draft.visibility,
    // A rule PM can only show (`kept`) belongs to an existing event, never a new one.
    recurrence: draft.recurrence.kind === "rule" ? copyRule(draft.recurrence.rule) : null,
    guests: draft.guests
      .filter((g) => !g.self && !g.organizer)
      .map((g) => ({ email: g.email, optional: g.optional })),
    reminders: draft.reminders.use_default ? null : copyReminders(draft.reminders),
    colour: draft.colour.kind === "calendar" ? null : copyColour(draft.colour),
    meet: draft.meet,
  };
  const perms = draft.guest_perms;
  if (perms.modify !== GOOGLE_PERMS.modify) sent.guests_can_modify = perms.modify;
  if (perms.invite_others !== GOOGLE_PERMS.invite_others) {
    sent.guests_can_invite_others = perms.invite_others;
  }
  if (perms.see_others !== GOOGLE_PERMS.see_others) {
    sent.guests_can_see_other_guests = perms.see_others;
  }
  return sent;
}

// --- duplicating an event (C18c) ---

/** A copy of an event, as Duplicate opens it in the full editor's create mode. */
export interface EventCopy {
  /** The new event's fields: the original's, as Google's Duplicate copies them. The type is the
   *  allow-list: nothing outside the editor's fields (PM's notes, a label, the ids) can reach the
   *  create. */
  fields: EditorFields;
  /** What wasn't copied, said under the dialog's bar. */
  notes: string[];
  /** Said under the video-call row when the original had a call, which a copy never takes. */
  callNote: string | null;
}

/** Google's own Duplicate makes no video call ("no video conferencing by default", Workspace admin
 *  help 9898950): one click adds a new one. */
export const COPY_CALL_NOTE = "The video call isn't copied. Add a new one if you need it.";
/** `COPY_CALL_NOTE` on a calendar that offers no Meet: there's nothing to add. */
export const COPY_CALL_OFF_NOTE = "The video call isn't copied.";
export const COPY_ATTACHMENTS_NOTE = "Attachments aren't copied: PM can't add attachments yet.";
export const COPY_GUESTS_NOTE =
  "The original's guests aren't copied. Add them if this is a meeting.";
export const COPY_OMITTED_NOTE =
  "Google didn't send the whole guest list, so the copy has only the guests PM could see.";

/** Said of a copy of an occurrence, which starts as a one-off: `repeats` is how the original
 *  repeats in Google Calendar's words, null when PM couldn't read it. */
export const copyRepeatNote = (repeats: string | null): string =>
  `The original repeats${repeats ? ` (${repeats})` : ""}; the copy doesn't. Choose Repeat to set it.`;

/** Said when the original has a linked milestone, which stays with it (C18 critique C2-M5). */
export const copyMilestoneNote = (label: string): string =>
  `The “${label}” milestone stays linked to the original.`;

/** The calendar roles that may add and change events: the backend's `create_block` refuses every
 *  other. */
export const WRITABLE_ROLES: readonly string[] = ["owner", "writer", "writerWithoutPrivateAccess"];

/** A new event's fields copied from `event` (Google's fresh copy of the row the user chose) and, for
 *  an occurrence, its repeating event's `series` (`repeating`: the row is one, though PM may not
 *  have read its series), as Google's Duplicate makes a copy. The title, time, place, description
 *  (unchanged, formatted or not, with no note: C18 critique C2-L12, C19 critique C2-H1), busy or
 *  free, visibility, notifications, colour and guest permissions are the original's. A copy of an
 *  event the user organises keeps its guests, all but the user's own entry, none of them the
 *  organiser, with no replies; a copy of an invitation starts with none, since "Don't send" may
 *  still put it on their calendars (C18 critique C2-M7, probe L-CP5). Google's `organizer_self`
 *  means the calendar it's read from organises it, so it's the user's own only where `role`, that
 *  calendar's access role, lets them change its events: on a colleague's calendar shared to view, it
 *  is the colleague's meeting, and its copy starts with none too. An occurrence's copy starts as a
 *  one-off at that occurrence's time (critiques C1-M4, C2-M6: a copied rule's count would start
 *  again, and a moved occurrence's copy would jump to the rule's day; probe L-CP3). Never a video
 *  call, never attachments; the note that the call isn't copied is for a call Google made or is
 *  making, never a request it failed, which made none. */
export function seedDuplicate(
  event: EventForEdit,
  series: SeriesInfo | null,
  repeating: boolean,
  role: string | null,
): EventCopy {
  const notes: string[] = [];
  const others = event.guests.filter((g) => !g.self);
  let guests: GuestDraft[] = [];
  if (!event.organizer_self || role === null || !WRITABLE_ROLES.includes(role)) {
    if (others.length > 0) notes.push(COPY_GUESTS_NOTE);
  } else {
    guests = others.map((g) => ({ ...g, organizer: false, response: null }));
    if (event.guests_omitted) notes.push(COPY_OMITTED_NOTE);
  }
  if (repeating || series !== null) notes.push(copyRepeatNote(series?.repeats ?? null));
  if (event.attachments.length > 0) notes.push(COPY_ATTACHMENTS_NOTE);
  return {
    fields: { ...seedDraft(event), guests, meet: false, organizer_email: null },
    notes,
    callNote: event.call !== null && event.call.state !== "failed" ? COPY_CALL_NOTE : null,
  };
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
    case "recurrence":
      target.recurrence = from.recurrence;
      break;
    // The guest list and its permissions merge (`rebaseGuests`, `rebasePerms`): never set whole.
    case "reminders":
      target.reminders = copyReminders(from.reminders);
      break;
    case "colour":
      target.colour = copyColour(from.colour);
      break;
    case "meet":
      target.meet = from.meet;
      break;
    case "show_as":
      target.show_as = from.show_as;
      break;
    case "visibility":
      target.visibility = from.visibility;
      break;
  }
}
