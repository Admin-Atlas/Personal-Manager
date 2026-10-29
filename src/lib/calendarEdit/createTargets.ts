// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a new event can go (#884, C10), as pure functions over the calendar overview. A calendar
// takes one when it's a Google calendar PM syncs, its account has editing on, and its role lets you
// add events: the same rule the backend's gate applies before anything is sent, and the one the
// calendar uses for Edit and Delete. Hidden calendars stay on offer (hiding is only what the
// calendar shows); the editor says so before and after the save.

import type {
  Calendar,
  CalendarAccount,
  CalendarOverview,
  EditingStatus,
  TimeDraft,
} from "../types";
import { formatDateLocal } from "../format";
import { wallInstant } from "../wallTime";
import { DEFAULT_LENGTH_MS, type GuestDraft } from "./eventDraft";
import type { CreatedWhere } from "./editReasons";
import { changeNotice, sameAddress } from "./guests";

/** A calendar a new event may go to, with its account. */
export interface CreateTarget {
  calendar: Calendar;
  account: CalendarAccount;
  /** writerWithoutPrivateAccess: it takes every event but a private one. */
  noPrivate: boolean;
}

export interface CreateTargets {
  /** By account, in the overview's account order; each account's calendars primary first, then by
   *  name. Accounts with none to offer are left out. */
  groups: { account: CalendarAccount; targets: CreateTarget[] }[];
  /** Google accounts whose calendars aren't offered because editing isn't on, with their status. */
  off: { account: CalendarAccount; status: EditingStatus }[];
}

/** The roles that may add events: the backend's `create_block` refuses every other. */
const WRITABLE_ROLES: readonly string[] = ["owner", "writer", "writerWithoutPrivateAccess"];

/** Every calendar a new event may go to, and the Google accounts that have editing off. */
export function createTargets(overview: CalendarOverview): CreateTargets {
  const groups: CreateTargets["groups"] = [];
  const off: CreateTargets["off"] = [];
  for (const account of overview.accounts) {
    if (account.provider !== "google") continue;
    const status = overview.editing[account.id] ?? "off";
    if (status !== "on") {
      off.push({ account, status });
      continue;
    }
    const targets = overview.calendars
      .filter(
        (c) =>
          c.source_id === account.id &&
          c.provider === "google" &&
          c.selected &&
          c.access_role !== null &&
          WRITABLE_ROLES.includes(c.access_role),
      )
      .sort((a, b) => Number(b.is_primary) - Number(a.is_primary) || a.name.localeCompare(b.name))
      .map((calendar) => ({
        calendar,
        account,
        noPrivate: calendar.access_role === "writerWithoutPrivateAccess",
      }));
    if (targets.length > 0) groups.push({ account, targets });
  }
  return { groups, off };
}

/** Why no new event can be made, for "+ New event" (which shows only once a Google account is
 *  connected), or null when a calendar can take one. With editing on somewhere, none of its
 *  calendars can; with it on nowhere, the way out is the switch in Settings. */
export function createReasonFor(overview: CalendarOverview, t: CreateTargets): string | null {
  if (t.groups.length > 0) return null;
  const editingOn = overview.accounts.some(
    (a) => a.provider === "google" && overview.editing[a.id] === "on",
  );
  return editingOn
    ? "None of your calendars can take new events."
    : "Turn on editing for a Google account in Settings › Connectors to create events.";
}

/** Where a new event starts: the calendar last created in (`lastId`), while it can still take one;
 *  else the first primary calendar, in account order; else the first calendar on offer; else none,
 *  and nothing can be created. */
export function defaultTarget(t: CreateTargets, lastId: string | null): CreateTarget | null {
  const all = t.groups.flatMap((g) => g.targets);
  return (
    all.find((x) => x.calendar.id === lastId) ??
    all.find((x) => x.calendar.is_primary) ??
    all[0] ??
    null
  );
}

/** Where a copy starts (C18c): the original's calendar when a new event can go there, else where a
 *  new event would (`defaultTarget`). Any account's calendar can be picked after, Duplicate being
 *  the way to another account (Q13). */
export function duplicateTarget(
  t: CreateTargets,
  calendarId: string,
  lastId: string | null,
): CreateTarget | null {
  return targetById(t, calendarId) ?? defaultTarget(t, lastId);
}

/** A copy's guests on `target` (C18c): never the calendar's own address, which organises the event
 *  there and can't be its guest (critique C1-L9). The picker drops it on a later pick too
 *  (`NewEventEditor`). */
export function guestsOnto<G extends { email: string }>(
  guests: readonly G[],
  target: CreateTarget,
): G[] {
  const own = target.calendar.remote_id;
  return own ? guests.filter((g) => !sameAddress(g.email, own)) : [...guests];
}

/** The calendar `id` on offer, if it is. */
export function targetById(t: CreateTargets, id: string): CreateTarget | null {
  for (const g of t.groups) {
    const found = g.targets.find((x) => x.calendar.id === id);
    if (found) return found;
  }
  return null;
}

/** How long a new event on `target` lasts, in ms: its account's Google default length, else an
 *  hour (none read yet, or none on offer). */
export function lengthFor(overview: CalendarOverview, target: CreateTarget | null): number {
  return accountLength(overview, target?.account.id ?? null);
}

/** The Google default length of the account `accountId` (`gcal:` + address), in ms, else an hour:
 *  how long an all-day event dragged into the hours becomes (C16b), as a new event there would. */
export function accountLength(overview: CalendarOverview, accountId: string | null): number {
  const minutes = accountId !== null ? overview.default_length[accountId] : undefined;
  return typeof minutes === "number" && Number.isInteger(minutes) && minutes >= 1 && minutes <= 1440
    ? minutes * 60_000
    : DEFAULT_LENGTH_MS;
}

/** The calendar offers Google Meet (its `allowedConferenceSolutionTypes`), so a new event there can
 *  ask for a call (C18b). */
export const meetOffered = (t: CreateTarget | null): boolean =>
  t !== null && t.calendar.conference_types.includes("hangoutsMeet");

/** Whether a new event on `t` inviting `guests` has a Meet call before the user touches the row
 *  (C18b): its account's Google setting says to add one (`overview.auto_meet`), the calendar offers
 *  Meet, and it invites someone other than the user, as Google adds Meet "when you add invitees"
 *  (Help 72143) and "when users add the first guest" (admin help 9898950), critique C2-H2. An
 *  account whose setting PM hasn't read is off: no call is made on a guess. Probe L-M6 records
 *  whether a personal account adds one to a solo event too; if so, the guest clause goes. */
export function meetDefault(
  overview: CalendarOverview,
  t: CreateTarget | null,
  guests: readonly GuestDraft[],
): boolean {
  if (t === null || !meetOffered(t) || overview.auto_meet[t.account.id] !== true) return false;
  // Who Google would invite, counted as the email question counts them: never the user, a room or
  // the calendar's own address.
  return changeNotice([], guests, true, false, t.account.email) !== null;
}

/** A new event's video call once the calendar picked is `t` (C18b): untouched (`touched` false),
 *  what `meetDefault` says there; touched, as the user left it, unless `t` offers no Meet. `off`: a
 *  call the draft had went off because `t` offers no Meet, which the editor says (`meetOffText`). */
export function meetOnCalendar(
  overview: CalendarOverview,
  t: CreateTarget | null,
  draft: { meet: boolean; guests: readonly GuestDraft[] },
  touched: boolean,
): { meet: boolean; off: boolean } {
  const offered = meetOffered(t);
  return {
    meet: touched ? draft.meet && offered : meetDefault(overview, t, draft.guests),
    off: draft.meet && !offered,
  };
}

/** Said when another calendar turned a new event's video call off (C18b). */
export const meetOffText = (calendar: string): string =>
  `${calendar} doesn't offer Google Meet, so the video call is off.`;

/** `date` (`YYYY-MM-DD`) at UTC midnight, moved by `days`, as ms. */
const utcDay = (date: string, days = 0) => Date.parse(`${date}T00:00:00Z`) + days * 86_400_000;

/** Which side of the months PM mirrors a new event's time falls wholly on, so it would be saved in
 *  Google but not shown here, or null when it overlaps them: the backend's own overlap test (start
 *  before the window's end, end after its start; all-day days as UTC midnights, the last one
 *  exclusive). The backend decides; this is the warning before Save. A time that can't be read says
 *  nothing: the problem lines do. */
export function outsideMirror(
  time: TimeDraft,
  overview: CalendarOverview,
): "before" | "after" | null {
  const from = Date.parse(overview.mirror_start);
  const until = Date.parse(overview.mirror_end);
  if (Number.isNaN(from) || Number.isNaN(until)) return null;
  let start: number;
  let end: number;
  if (time.kind === "all_day") {
    start = utcDay(time.first_day);
    end = utcDay(time.last_day, 1);
  } else {
    const s = wallInstant(time.start_date, time.start_time, time.start_zone);
    if (s === null) return null;
    start = s.getTime();
    end = wallInstant(time.end_date, time.end_time, time.end_zone)?.getTime() ?? start;
  }
  if (Number.isNaN(start) || Number.isNaN(end)) return null;
  if (start >= until) return "after";
  return end <= from ? "before" : null;
}

/** Where a new event on `calendarId` shows (C10 §15): its name, and whether PM's calendar keeps it
 *  out of sight. It does when the calendar is hidden in the list and no copy of the same Google
 *  calendar shows through another account (that copy would show it after the next sync), or when
 *  PM no longer syncs it at all (unticked in Settings while the event was being made). */
export function createdWhere(
  overview: CalendarOverview,
  hidden: ReadonlySet<string>,
  calendarId: string,
): CreatedWhere {
  const cal = overview.calendars.find((c) => c.id === calendarId);
  if (!cal) return { name: "that calendar", hidden: true };
  const twinShows = overview.calendars.some(
    (c) =>
      c.id !== cal.id &&
      c.provider === cal.provider &&
      cal.remote_id !== null &&
      c.remote_id === cal.remote_id &&
      c.selected &&
      !hidden.has(c.id),
  );
  return { name: cal.name, hidden: !cal.selected || (hidden.has(cal.id) && !twinShows) };
}

/** What a new event's surfaces (the full editor and quick create) say before Save (C10 §15): a start
 *  outside the months PM syncs, and a calendar hidden here with no twin showing it. One that
 *  `repeats` from before those months still shows here wherever it falls inside them, as the backend
 *  lists and lands those (C11); one that starts after them has none there. */
export function createNotes(
  time: TimeDraft,
  repeats: boolean,
  overview: CalendarOverview,
  hidden: ReadonlySet<string>,
  calendarId: string,
): string[] {
  const notes: string[] = [];
  const side = outsideMirror(time, overview);
  if (side !== null) {
    const range = `${formatDateLocal(new Date(overview.mirror_start))} – ${formatDateLocal(
      new Date(overview.mirror_end),
    )}`;
    notes.push(
      side === "before" && repeats
        ? `This starts before the months PM syncs (${range}), so only its occurrences within them will show here.`
        : `This starts outside the months PM syncs (${range}), so it'll be saved in Google but won't show here.`,
    );
  }
  const where = createdWhere(overview, hidden, calendarId);
  if (hidden.has(calendarId) && where.hidden) {
    notes.push(
      `${where.name} is hidden in your calendar list, so the new event won't show until you show that calendar again.`,
    );
  }
  return notes;
}
