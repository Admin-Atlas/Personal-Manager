// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Replies to invitations (#884, C14): the user's own reply on a row, the three choices in Google's
// order, and a note counted as the backend counts it. Pure; names no wrapper and no command. What a
// reply came to, in words, is `replyText` in editReasons.ts, beside the other outcomes.
//
// The user's reply is their own entry on the guest list, the one Google marks `self`: the calendar
// the copy is on, chosen as the backend chooses it (`calendar_write::rsvp::self_entry`).

import type { CalendarEvent, Rsvp } from "../types";
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
  const marked = (ev.attendees ?? []).filter(
    (a) => a.self && a.email !== null && trimmed(a.email) !== "",
  );
  const own =
    (ownAddress !== null
      ? marked.find((a) => a.email !== null && addressKey(a.email) === addressKey(ownAddress))
      : undefined) ?? marked[0];
  return own ? replyOf(own.response) : null;
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
