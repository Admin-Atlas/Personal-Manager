// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Replies to invitations (#884, C14): the user's own reply on a row, the three choices in Google's
// order, and a note counted as the backend counts it. Pure; names no wrapper and no command. What a
// reply came to, in words, is `replyText` in editReasons.ts, beside the other outcomes.
//
// The user's reply is their own entry on the guest list, the one Google marks `self`: the calendar
// the copy is on, chosen as the backend chooses it (`calendar_write::rsvp::self_entry`).
//
// The calendar shows one row per occurrence, and the same invitation can be mirrored on two of the
// user's calendars (both their accounts connected): the popover offers a reply from each copy that
// can send one (`replyCopies`), and the grid strikes a row through only when every copy that has
// the user's own entry says they declined (`markDeclined`), the nearest PM can come to Google
// drawing each calendar's copy apart.

import { occurrenceKey } from "../calendar-layout";
import type { Attendee, Calendar, CalendarEvent, EventForEdit, GuestForEdit, Rsvp } from "../types";
import { addressKey, replyOf, trimmed, type Reply } from "./guests";

/** The three replies, in Google's order. */
export const RSVP_CHOICES: readonly { value: Rsvp; label: string }[] = [
  { value: "yes", label: "Yes" },
  { value: "no", label: "No" },
  { value: "maybe", label: "Maybe" },
];

/** A reply's word on its button, Google's own. */
export function rsvpLabel(reply: Rsvp): string {
  return RSVP_CHOICES.find((c) => c.value === reply)?.label ?? reply;
}

/** The user's own reply on a mirror row (the guest-list entry marked `self`, with an address), or
 *  null without one. With two marked, the one whose address is `ownAddress` (the calendar's
 *  `remote_id`), ignoring case, else the first, as the backend picks. */
export function myReply(
  ev: Pick<CalendarEvent, "attendees">,
  ownAddress: string | null = null,
): Reply | null {
  const own = ownEntry(ev, ownAddress);
  return own ? replyOf(own.response) : null;
}

/** The user's own entry on a row's guest list (see `myReply`), or null without one. */
function ownEntry<A extends Attendee>(
  ev: { attendees?: readonly A[] | null },
  ownAddress: string | null,
): A | null {
  const marked = (ev.attendees ?? []).filter(
    (a) => a.self && a.email !== null && trimmed(a.email) !== "",
  );
  return (
    (ownAddress !== null
      ? marked.find((a) => a.email !== null && addressKey(a.email) === addressKey(ownAddress))
      : undefined) ??
    marked[0] ??
    null
  );
}

/** The user declined it (Google strikes such an event through). */
export const declinedByMe = (
  ev: Pick<CalendarEvent, "attendees">,
  ownAddress: string | null = null,
): boolean => myReply(ev, ownAddress) === "no";

/** A `Reply` (with "awaiting") as one of the three, or null: with no reply yet, none is chosen. */
export function asRsvp(r: Reply | null): Rsvp | null {
  return r === "yes" || r === "no" || r === "maybe" ? r : null;
}

/** Longest note PM sends, in characters, counted as the backend counts them: after tidying, by
 *  code point (`[...text]`), never by UTF-16 unit. */
export const NOTE_MAX = 1000;

/** A note as Google keeps it, as the backend tidies it before sending (`tidy_note`): `\r\n` and
 *  `\r` as `\n`, other control characters except `\n` and `\t` dropped, the ends trimmed. */
export function tidyNote(text: string): string {
  const lines = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  // Rust's `char::is_control`: the C0 and C1 controls and DEL.
  // eslint-disable-next-line no-control-regex
  const kept = lines.replace(/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g, "");
  return trimmed(kept);
}

/** How long a note is, as the backend counts it. */
export const noteLength = (text: string): number => [...tidyNote(text)].length;

/** A reply as Google words it on the guest list (the backend's `Rsvp::as_google`). */
const GOOGLE_RESPONSE: Record<Rsvp, string> = {
  yes: "accepted",
  no: "declined",
  maybe: "tentative",
};

/** The editor's copy once Google holds a reply sent from it (C14c): the reply and its note (`note`
 *  null left the one Google held), and the user's own entry on `guests` (the one the backend
 *  answers for: `ownAddress`'s, else the first marked) saying it. Nothing else changes: a reply is
 *  none of the editor's fields, so it never makes a draft to save. */
export function withReply(
  event: EventForEdit,
  reply: Rsvp,
  note: string | null,
  ownAddress: string | null,
): EventForEdit {
  const response = GOOGLE_RESPONSE[reply];
  return {
    ...event,
    reply: { response, note: note === null ? (event.reply?.note ?? "") : tidyNote(note) },
    guests: guestsWithReply(event.guests, reply, ownAddress),
  };
}

/** `guests` with the user's own entry (as `withReply` picks it) saying `reply`; the same list when
 *  there's none. A new list, every other entry as it was. */
export function guestsWithReply<T extends GuestForEdit>(
  guests: readonly T[],
  reply: Rsvp,
  ownAddress: string | null,
): T[] {
  const own = ownEntry({ attendees: guests }, ownAddress);
  return guests.map((g) => (g === own ? { ...g, response: GOOGLE_RESPONSE[reply] } : g));
}

// --- declined events, and the copies a reply can come from (C14b) ---

/** A row as a view shows it: `declined` when the user declined it, set by `markDeclined` on the
 *  Calendar tab and by the backend on the Focus tab's agenda (whose rows carry no guest list). */
export type ShownEvent = CalendarEvent & { declined?: boolean };

/** Whether a shown row is struck through: the user declined it, and Google Calendar draws a declined
 *  event's title struck through. */
export const isDeclined = (ev: CalendarEvent): boolean => (ev as ShownEvent).declined === true;

/** An event's accessible name, ending ", declined" when it is struck through (`isDeclined`). */
export const declinedName = (name: string, declined: boolean): string =>
  declined ? `${name}, declined` : name;

/** `kept`, the rows a view shows (one per occurrence, deduped from `visible`, every copy it could
 *  show), each marked `declined` when at least one copy of its occurrence has the user's own guest
 *  entry and every such copy says they declined (C14 critiques C1-M4, C2-H3). Google draws each
 *  calendar's copy apart; PM shows one for them all, so a copy still going keeps the row plain.
 *  `ownAddress` gives a calendar's own address (its `remote_id`), which picks the entry as the
 *  backend does. A row left plain is the same object; a declined one is a copy. */
export function markDeclined<T extends CalendarEvent>(
  kept: readonly T[],
  visible: readonly CalendarEvent[],
  ownAddress: (calendarId: string) => string | null,
): T[] {
  const copies = new Map<string, CalendarEvent[]>();
  for (const e of visible) {
    const key = occurrenceKey(e);
    if (key === null) continue;
    copies.set(key, [...(copies.get(key) ?? []), e]);
  }
  return kept.map((ev) => {
    const key = occurrenceKey(ev);
    const all = (key !== null ? copies.get(key) : undefined) ?? [ev];
    const answers = all.flatMap((copy) => {
      const own = ownEntry(copy, ownAddress(copy.calendar_id));
      return own ? [replyOf(own.response)] : [];
    });
    const declined = answers.length > 0 && answers.every((r) => r === "no");
    return declined ? { ...ev, declined: true } : ev;
  });
}

/** One copy of an occurrence the user may reply from, and whose reply it is. */
export interface ReplyCopy {
  event: CalendarEvent;
  calendar: Calendar;
  /** The calendar's name when the popover must say whose reply it is, else null. */
  forWhom: string | null;
}

/** The copies of `ev`'s occurrence the user can reply from: every row of `events` (the rows before
 *  the view's dedup, so the invitation's copy shows even when the organiser's was kept) with the
 *  same occurrence, or `ev` alone when it has none, that the list marked answerable, on a Google
 *  calendar `canWrite` allows (shown, and its account's editing on). `ev`'s own copy first, then
 *  by calendar name. Each is named when there's more than one, or when its calendar isn't its
 *  account's main one: replying there answers for that calendar (C14 spec §0.5 row 6). */
export function replyCopies(
  events: readonly CalendarEvent[],
  ev: CalendarEvent,
  calendars: readonly Calendar[],
  canWrite: (calendar: Calendar) => boolean,
): ReplyCopy[] {
  const key = occurrenceKey(ev);
  const rows = key === null ? [ev] : events.filter((e) => occurrenceKey(e) === key);
  const copies = rows.flatMap((event) => {
    const calendar = calendars.find((c) => c.id === event.calendar_id);
    return event.can_respond === true &&
      calendar !== undefined &&
      calendar.provider === "google" &&
      canWrite(calendar)
      ? [{ event, calendar }]
      : [];
  });
  copies.sort(
    (a, b) =>
      Number(b.event.id === ev.id) - Number(a.event.id === ev.id) ||
      a.calendar.name.localeCompare(b.calendar.name),
  );
  return copies.map((c) => ({
    ...c,
    forWhom: copies.length > 1 || !c.calendar.is_primary ? c.calendar.name : null,
  }));
}
