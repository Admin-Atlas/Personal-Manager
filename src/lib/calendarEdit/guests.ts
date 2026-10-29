// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Guests in the editor (#884, C12): the address check, replies as glyphs, suggestions from addresses
// already in PM's calendars (no contacts, no new permission), and every line the email question
// shows. Pure; names no wrapper and no command.
//
// Who a change would email is counted as the backend counts it (`calendar_write::attendees`, the
// same tables in both tests): never the user, never a room, and a change to only the user's own
// fields (busy or free) emails nobody. The backend has the last word: when the two disagree, the
// save asks with the backend's numbers, so neither emails anyone unasked.

import type { Calendar, CalendarEvent, GuestNotice, Notify, SeriesScope } from "../types";

// --- addresses ---

/** White space as the backend reads it (Rust's `char::is_whitespace`, Unicode's White_Space), so
 *  the two trim and refuse the same characters: JavaScript's `\s` and `trim()` differ from it in
 *  two (U+0085, U+FEFF). Every one is a single UTF-16 unit. */
const SPACE = new Set<number>([
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x2000, 0x2001, 0x2002, 0x2003, 0x2004,
  0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200a, 0x2028, 0x2029, 0x202f, 0x205f, 0x3000,
]);
const isSpace = (ch: string) => SPACE.has(ch.charCodeAt(0));

/** What an address may never hold besides space (Google refuses them; a comma or angle bracket is
 *  two addresses or a name, typed together). */
const BANNED = new Set(['"', ",", ";", ":", "<", ">"]);

/** `text` with the backend's white space taken off both ends (Rust's `str::trim`). */
export function trimmed(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && isSpace(text[start])) start++;
  while (end > start && isSpace(text[end - 1])) end--;
  return text.slice(start, end);
}

/** An address as PM compares it: trimmed, whatever its case. */
export const addressKey = (text: string) => trimmed(text).toLowerCase();

/** The domain Google gives rooms and other bookable resources. */
const ROOM_DOMAIN = "@resource.calendar.google.com";

/** Whether `text` is an email address as PM accepts one, as the backend's `is_address` (one table
 *  for both): trimmed, at most 254 characters, exactly one `@` with 1-64 characters before it, a
 *  domain of dot-separated labels (at least two, none empty, none starting or ending with `-`), and
 *  no space, double quote, comma, semicolon, colon or angle bracket. */
export function isEmail(text: string): boolean {
  const chars = [...trimmed(text)];
  if (chars.length > 254 || chars.some((ch) => isSpace(ch) || BANNED.has(ch))) return false;
  const at = chars.indexOf("@");
  if (at < 0) return false;
  const domain = chars.slice(at + 1).join("");
  const labels = domain.split(".");
  return (
    at >= 1 &&
    at <= 64 &&
    !domain.includes("@") &&
    labels.length >= 2 &&
    labels.every((l) => l !== "" && !l.startsWith("-") && !l.endsWith("-"))
  );
}

/** The same address, whatever its case or the space round it. */
export const sameAddress = (a: string, b: string): boolean => addressKey(a) === addressKey(b);

/** A room's or another bookable resource's address (Google's own domain), for lists that don't say
 *  (the mirror, a guest typed in). */
export const isRoomAddress = (email: string): boolean => addressKey(email).endsWith(ROOM_DOMAIN);

/** `text` cut at commas, semicolons and new lines, outside double quotes, so a quoted name can hold
 *  a comma; each piece with whether a comma ended it. */
function chunks(text: string): { text: string; comma: boolean }[] {
  const out: { text: string; comma: boolean }[] = [];
  let quoted = false;
  let start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '"') quoted = !quoted;
    else if (!quoted && (c === "," || c === ";" || c === "\n" || c === "\r")) {
      out.push({ text: text.slice(start, i), comma: c === "," });
      start = i + 1;
    }
  }
  out.push({ text: text.slice(start), comma: false });
  return out;
}

const words = (text: string) => text.split(/\s+/).filter((w) => w !== "");

const NAMED = /<([^<>]*)>/;

/** Addresses pasted or typed together: split on commas, semicolons, spaces and new lines. A
 *  "Name <a@b.c>" form gives its address and drops the name, which read as words of its own would be
 *  left in the box and stop the save (C12 critique C1), and so does Outlook's unquoted
 *  "Last, First <a@b.c>", whose comma is the name's. Each address once; empty pieces dropped;
 *  anything else kept, for the caller to leave in the box. */
export function splitAddresses(text: string): string[] {
  const out: string[] = [];
  const pieces = chunks(text);
  for (let i = 0; i < pieces.length; i++) {
    const chunk = pieces[i].text;
    const named = NAMED.exec(chunk);
    if (!named) {
      // The start of a name: pieces with no address, each ended by a comma, up to a "<a@b.c>".
      let j = i;
      while (j < pieces.length && pieces[j].comma && !/[@<>]/.test(pieces[j].text)) j++;
      if (j > i && j < pieces.length && NAMED.test(pieces[j].text)) {
        i = j - 1;
        continue;
      }
      out.push(...words(chunk));
      continue;
    }
    // An address typed before the name is kept; the name's own words go.
    out.push(...words(chunk.slice(0, named.index)).filter(isEmail));
    out.push(...words(named[1]));
    out.push(...words(chunk.slice(named.index + named[0].length)));
  }
  // "a@b.c <a@b.c>", as some mail apps copy an address with no name, is one guest.
  const once = new Set<string>();
  return out.filter((p) => {
    if (!isEmail(p)) return true;
    const key = addressKey(p);
    if (once.has(key)) return false;
    once.add(key);
    return true;
  });
}

// --- replies ---

/** A guest's reply, in the glyphs' words. */
export type Reply = "yes" | "no" | "maybe" | "awaiting";

/** Google's and Microsoft's words for a reply, or null for an organiser's own or an unknown one. */
export function replyOf(v: string | null): Reply | null {
  switch (v) {
    case "accepted":
      return "yes";
    case "declined":
      return "no";
    case "tentative":
    case "tentativelyAccepted":
      return "maybe";
    case "needsAction":
    case "notResponded":
    case "none":
      return "awaiting";
    default:
      return null;
  }
}

/** A reply as the event popover and the guest list name it (Google's and Microsoft's words). */
export function responseLabel(v: string | null): string | null {
  switch (v) {
    case "accepted":
      return "accepted";
    case "declined":
      return "declined";
    case "tentative":
    case "tentativelyAccepted":
      return "maybe";
    case "needsAction":
    case "notResponded":
    case "none":
      return "no reply";
    case "organizer":
      return "organiser";
    default:
      return null;
  }
}

export const REPLY_GLYPH: Record<Reply, string> = { yes: "✓", no: "✗", maybe: "?", awaiting: "" };

const REPLY_ORDER: readonly Reply[] = ["yes", "no", "maybe", "awaiting"];

/** `k` guests, in words. */
const guestCount = (k: number) => (k === 1 ? "1 guest" : `${k} guests`);

/** "3 guests: 1 yes, 1 no, 1 awaiting", as Google sums up a guest list (INFERRED): the organiser
 *  counts as a guest, rooms don't; "1 guest" when no reply is known. Empty for no guests. */
export function guestSummary(
  guests: readonly { email: string; response: string | null; room: boolean }[],
): string {
  const people = guests.filter((g) => !g.room && !isRoomAddress(g.email));
  if (people.length === 0) return "";
  const parts = REPLY_ORDER.map((reply) => {
    const n = people.filter((g) => replyOf(g.response) === reply).length;
    return n > 0 ? `${n} ${reply}` : "";
  }).filter((p) => p !== "");
  const head = guestCount(people.length);
  return parts.length > 0 ? `${head}: ${parts.join(", ")}` : head;
}

// --- suggestions ---

/** Someone PM has seen on an event, to offer as a guest. */
export interface GuestSuggestion {
  email: string;
  name: string | null;
  /** How many events they are on. */
  count: number;
}

/** Calendars PM reads through an account the user signed in to (Google, Outlook): never a
 *  subscription, whose feed can carry strangers' addresses, whatever it calls itself (C12 critique
 *  C1-H3: an Outlook-published feed's provider is "outlook" too). */
const fromAnAccount = (c: Calendar) =>
  c.source_id.startsWith("gcal:") || c.source_id.startsWith("outlook:");

/** The account a calendar is read with: the address after `gcal:` or `outlook:`. */
const accountOf = (c: Calendar) => c.source_id.slice(c.source_id.indexOf(":") + 1);

/** The domain Google gives a calendar that isn't a person's (one the user made, "Work" say). */
const GROUP_DOMAIN = "@group.calendar.google.com";

/** Every address in PM's mirrored Google and Outlook calendars, once each, its name the most common
 *  non-empty one, most-seen first then by name. Never the user, a room, `me`, or anything that
 *  isn't an address. The user is every account's own address (Outlook never marks one as the
 *  user's), and the entry Google marks as the calendar's own on the user's primary calendar and on
 *  a calendar that isn't a person's: there it's the calendar itself, never a guest to invite. On a
 *  person's calendar shared into the account, it's the colleague whose calendar it is, who stays
 *  suggested. */
export function guestSuggestions(
  events: readonly CalendarEvent[],
  calendars: readonly Calendar[],
  me: string | null,
): GuestSuggestion[] {
  const accounts = calendars.filter(fromAnAccount);
  const fromAccounts = new Set(accounts.map((c) => c.id));
  // The calendars whose own entry is the user's, not a colleague's.
  const ownEntryIsMine = new Set(
    accounts
      .filter((c) => c.is_primary || addressKey(c.remote_id ?? "").endsWith(GROUP_DOMAIN))
      .map((c) => c.id),
  );
  const seen = new Map<string, { email: string; names: Map<string, number>; count: number }>();
  const mine = new Set<string>(me ? [addressKey(me)] : []);
  for (const c of accounts) mine.add(addressKey(accountOf(c)));
  for (const event of events) {
    if (!fromAccounts.has(event.calendar_id)) continue;
    for (const a of event.attendees ?? []) {
      if (!a.email) continue;
      const key = addressKey(a.email);
      if (a.self && ownEntryIsMine.has(event.calendar_id)) {
        mine.add(key);
        continue;
      }
      if (!isEmail(a.email) || isRoomAddress(a.email)) continue;
      const entry = seen.get(key) ?? { email: trimmed(a.email), names: new Map(), count: 0 };
      entry.count += 1;
      const name = a.name?.trim();
      if (name) entry.names.set(name, (entry.names.get(name) ?? 0) + 1);
      seen.set(key, entry);
    }
  }
  const out: GuestSuggestion[] = [];
  for (const [key, entry] of seen) {
    if (mine.has(key)) continue;
    let name: string | null = null;
    let most = 0;
    for (const [n, k] of entry.names) {
      if (k > most || (k === most && name !== null && n.localeCompare(name) < 0)) {
        name = n;
        most = k;
      }
    }
    out.push({ email: entry.email, name, count: entry.count });
  }
  const label = (s: GuestSuggestion) => (s.name ?? s.email).toLowerCase();
  return out.sort(
    (a, b) =>
      b.count - a.count ||
      label(a).localeCompare(label(b)) ||
      a.email.toLowerCase().localeCompare(b.email.toLowerCase()),
  );
}

/** Up to `limit` suggestions whose address or name holds `query` (any case; at least one character
 *  besides space), leaving out addresses already on the list. */
export function matchSuggestions(
  all: readonly GuestSuggestion[],
  query: string,
  present: readonly string[],
  limit = 6,
): GuestSuggestion[] {
  const q = trimmed(query).toLowerCase();
  if (q === "") return [];
  const on = new Set(present.map(addressKey));
  return all
    .filter(
      (s) =>
        !on.has(addressKey(s.email)) &&
        (s.email.toLowerCase().includes(q) || (s.name ?? "").toLowerCase().includes(q)),
    )
    .slice(0, limit);
}

// --- who a change would email ---

/** One guest as the counts read it. `room`: Google's flag, where the list has one. */
interface Person {
  email: string | null;
  self: boolean;
  room?: boolean;
}

/** The calendar's own addresses, as the backend's `Own`: every entry marked as the calendar's own,
 *  and `me` (the account saving), never a guest Google emails. */
function ownOf(lists: readonly (readonly Person[])[], me: string | null): Set<string> {
  const own = new Set<string>(me && trimmed(me) ? [addressKey(me)] : []);
  for (const list of lists) {
    for (const g of list) if (g.self && g.email) own.add(addressKey(g.email));
  }
  return own;
}

/** Who on `list` Google would email: every address that is neither the user's (`own`, which holds
 *  every entry marked as the calendar's) nor a room's. */
function recipients(list: readonly Person[], own: Set<string>): Set<string> {
  const out = new Set<string>();
  for (const g of list) {
    if (!g.email || g.room || isRoomAddress(g.email)) continue;
    const key = addressKey(g.email);
    if (!own.has(key)) out.add(key);
  }
  return out;
}

/** Who a change from `before` to `after` would email, counted as the backend's `Reach` counts a
 *  PATCH: guests added are invited and guests taken off are told; when the change is `shared` (it
 *  touches anything guests see, not only the user's own fields) everyone still on it hears too.
 *  `cutShort`: Google didn't send the whole list, so a shared change reaches "at least" these. Null
 *  when nobody would be emailed and nothing was hidden. `moves`: the same Save puts the event on
 *  another calendar (C18d), which tells everyone on it then once more, as the backend's
 *  `Reach::of_transfer` counts it: each of them already invited hears of a change, and the emails
 *  grow by one each. */
export function changeNotice(
  before: readonly Person[],
  after: readonly Person[],
  shared: boolean,
  cutShort: boolean,
  me: string | null = null,
  moves = false,
): GuestNotice | null {
  const own = ownOf([before, after], me);
  const was = recipients(before, own);
  const now = recipients(after, own);
  const told = shared || moves;
  const removed = [...was].filter((a) => !now.has(a));
  const invited = [...now].filter((a) => !was.has(a));
  const updated = told ? [...now].filter((a) => was.has(a)) : [];
  const people = updated.length + invited.length + removed.length;
  // The PATCH's own emails (one each, the user's own fields reaching nobody), and the calendar
  // change's: one to everyone on it then.
  const patched = shared ? people : invited.length + removed.length;
  const emails = patched + (moves ? now.size : 0);
  // A change to only the user's own fields emails nobody, however much of the list Google sent.
  const complete = !cutShort || !told;
  if (people === 0 && complete) return null;
  return {
    updated: updated.length,
    invited: invited.length,
    removed: removed.length,
    told_cancelled: 0,
    cancelled: false,
    people,
    emails,
    complete,
  };
}

/** Who deleting a mirror row would tell: its guests but the user and rooms (by address, since the
 *  mirror has no room flag), when the user organises it; null when nobody. */
export function deleteNotice(row: CalendarEvent, me: string | null = null): GuestNotice | null {
  if (!row.organizer_self) return null;
  const list = (row.attendees ?? []).map((a) => ({
    email: a.email,
    // The organiser of an event the calendar organises is the calendar itself.
    self: a.self || a.organizer,
  }));
  const told = recipients(list, ownOf([list], me)).size;
  if (told === 0) return null;
  return {
    updated: told,
    invited: 0,
    removed: 0,
    told_cancelled: 0,
    cancelled: true,
    people: told,
    emails: told,
    complete: true,
  };
}

/** Who moving a mirror row would email (C16a), judged as the backend's `notify_required` judges a
 *  change of times, which every guest shares: everyone on it but the user and rooms, told of a
 *  change. On an invitation the organiser lets guests change (C14c), the organiser is one of them;
 *  one they don't is no move of the user's, so nobody. Null when nobody. The backend counts again on
 *  Google's copy, and has the last word. */
export function moveNotice(row: CalendarEvent, me: string | null = null): GuestNotice | null {
  const mine = row.organizer_self === true;
  if (!mine && !(row.organizer_self === false && row.guests_can_modify === true)) return null;
  const list = (row.attendees ?? []).map((a) => ({
    email: a.email,
    // The organiser of an event the calendar organises is the calendar itself; someone else's is
    // a guest Google emails.
    self: a.self || (mine && a.organizer),
  }));
  const told = recipients(list, ownOf([list], me)).size;
  if (told === 0) return null;
  return {
    updated: told,
    invited: 0,
    removed: 0,
    told_cancelled: 0,
    cancelled: false,
    people: told,
    emails: told,
    complete: true,
  };
}

// --- the email question ---

/** What the answer is for: a save, a delete, or a new event. */
export type NotifyKind = "save" | "delete" | "create";

/** What an answer sends. Don't send is "none" (plan C12's copy). If probe L8-6 shows Google's own
 *  Don't send still emails guests outside Google Calendar, this becomes "external_only" and
 *  `dontSendDetail` loses its warning: one place to change. */
export const notifyOf = (n: Notify): Notify => n;

/** A delete speaks as one whatever its writes (a "this and following" delete cuts the earlier part
 *  with a change, not a delete). */
const cancels = (n: GuestNotice, kind: NotifyKind) => n.cancelled || kind === "delete";

/** The question, as Google asks it. */
export function notifyQuestion(n: GuestNotice, kind: NotifyKind): string {
  if (cancels(n, kind)) return "Send cancellation emails to guests?";
  if (n.invited > 0 && n.updated === 0 && n.removed === 0 && n.told_cancelled === 0) {
    return "Send invitation emails to guests?";
  }
  return "Send update emails to guests?";
}

/** Who would get what with Send, a line each, only those that apply. `scope`: the occurrences
 *  chosen, for a repeating event. */
export function guestLines(n: GuestNotice, scope: SeriesScope | null, kind: NotifyKind): string[] {
  const lines: string[] = [];
  const cancelled = cancels(n, kind);
  if (cancelled) {
    // Everyone a delete reaches hears the same: those only an occurrence's cancel reaches too.
    const told = n.updated + n.told_cancelled;
    if (told > 0) lines.push(`${guestCount(told)} will be told it's cancelled.`);
  } else {
    if (n.updated > 0) lines.push(`${guestCount(n.updated)} already invited will get an update.`);
    // On an occurrence the change cancels and nothing else it writes: no update reaches them.
    if (n.told_cancelled > 0) {
      lines.push(`${guestCount(n.told_cancelled)} will be told an occurrence is cancelled.`);
    }
  }
  if (n.invited > 0) lines.push(`${guestCount(n.invited)} new to it will get an invitation.`);
  if (n.removed > 0) {
    lines.push(`${guestCount(n.removed)} you took off will be told they're no longer invited.`);
  }
  // A delete makes no new part: its counts already cover every occurrence cancelled.
  if (scope === "following" && !cancelled && n.emails > n.people) {
    lines.push(
      `In all, about ${n.emails} emails: the new part is a new event, so its guests get an invitation to it, and an update about the earlier part and each occurrence moved or cancelled.`,
    );
  }
  // Not knowing covers a list Google cut short, an occurrence with guests of its own, and
  // occurrences PM couldn't list: none of them is "PM couldn't see every guest" alone (C12
  // critique C2-L13).
  if (!n.complete) {
    lines.push(
      n.people > 0
        ? "There may be more: PM can't tell for sure who else Google emails."
        : "PM can't tell for sure whether Google emails anyone.",
    );
  }
  return lines;
}

/** Send's line: who Google sends the emails as. */
export function sendDetail(from: string | null): string {
  return from ? `Google sends the emails from ${from}.` : "Google sends the emails.";
}

/** Don't send's line, with Google's own warning about it ("can stop events from syncing to external
 *  calendars or cause events to be lost") in plain words. */
export function dontSendDetail(kind: NotifyKind): string {
  const what =
    kind === "delete" ? "the cancellation" : kind === "create" ? "the invitation" : "the change";
  return `Nobody is emailed. Guests who don't use Google Calendar may never get ${what}: Google warns their calendars can stop updating, or lose the event.`;
}
