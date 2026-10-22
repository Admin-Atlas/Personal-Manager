// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Replies to invitations (C14): the user's own reply read as the backend reads it, and a note
// counted as the backend counts it (the same rows as `calendar_write::rsvp`'s tests); which rows are
// struck through as declined, and which copies of an occurrence a reply can come from (C14b).

import { describe, expect, it } from "vitest";

import type { Attendee, Calendar, CalendarEvent } from "../types";
import {
  asRsvp,
  declinedByMe,
  declinedName,
  isDeclined,
  markDeclined,
  myReply,
  NOTE_MAX,
  noteLength,
  replyCopies,
  RSVP_CHOICES,
  rsvpLabel,
  tidyNote,
} from "./rsvp";

const entry = (email: string | null, response: string | null, self = false): Attendee => ({
  name: null,
  email,
  response,
  optional: false,
  organizer: false,
  self,
});

describe("the user's own reply", () => {
  it("reads the entry marked as the calendar, never another guest's", () => {
    const ev = {
      attendees: [entry("a@x.org", "declined"), entry("me@x.org", "accepted", true)],
    };
    expect(myReply(ev)).toBe("yes");
    expect(declinedByMe(ev)).toBe(false);
    expect(myReply({ attendees: [entry("a@x.org", "declined")] })).toBeNull();
    expect(myReply({ attendees: [] })).toBeNull();
    expect(myReply({})).toBeNull();
    // Google's "tentative" is Maybe; no reply yet is awaiting.
    expect(myReply({ attendees: [entry("me@x.org", "tentative", true)] })).toBe("maybe");
    expect(myReply({ attendees: [entry("me@x.org", "needsAction", true)] })).toBe("awaiting");
  });

  it("with two marked, takes the calendar's own address in any case, else the first", () => {
    const ev = {
      attendees: [entry("Me+pm@X.org", "declined", true), entry("me@x.org", "accepted", true)],
    };
    expect(myReply(ev, "ME@x.org")).toBe("yes");
    expect(myReply(ev, "other@x.org")).toBe("no");
    expect(myReply(ev)).toBe("no");
    expect(declinedByMe(ev, "me@x.org")).toBe(false);
    expect(declinedByMe(ev, "me+pm@x.org")).toBe(true);
  });

  it("skips an entry marked as the calendar with no address, as the backend does", () => {
    const ev = { attendees: [entry(null, "declined", true), entry(" ", "declined", true)] };
    expect(myReply(ev)).toBeNull();
    expect(declinedByMe(ev)).toBe(false);
  });

  it("offers the three replies in Google's order, with Google's words", () => {
    expect(RSVP_CHOICES.map((c) => c.label)).toEqual(["Yes", "No", "Maybe"]);
    expect(RSVP_CHOICES.map((c) => c.value)).toEqual(["yes", "no", "maybe"]);
    expect(rsvpLabel("maybe")).toBe("Maybe");
    expect(asRsvp("yes")).toBe("yes");
    expect(asRsvp("maybe")).toBe("maybe");
    // No reply yet: none of the three is chosen.
    expect(asRsvp("awaiting")).toBeNull();
    expect(asRsvp(null)).toBeNull();
  });
});

describe("a note", () => {
  it("is tidied as the backend tidies it", () => {
    expect(tidyNote("a\r\nb\rc")).toBe("a\nb\nc");
    // The bell dropped, the tab trimmed at the end; tabs and new lines inside kept.
    expect(tidyNote("\u0007x\t")).toBe("x");
    expect(tidyNote(" a\tb\n\nc ")).toBe("a\tb\n\nc");
    // Rust's trim and controls, not JavaScript's: a C1 control goes, a zero-width no-break space
    // at the end stays (Rust doesn't count it as space).
    expect(tidyNote("x\u0085")).toBe("x");
    expect(tidyNote("x﻿")).toBe("x﻿");
  });

  it("is counted in characters after tidying, never in UTF-16 units", () => {
    expect(NOTE_MAX).toBe(1000);
    expect(noteLength("é".repeat(NOTE_MAX))).toBe(NOTE_MAX);
    expect(noteLength(`${"é".repeat(NOTE_MAX)}é`)).toBe(NOTE_MAX + 1);
    // Two UTF-16 units each, one character each.
    const faces = "😀".repeat(NOTE_MAX);
    expect(faces.length).toBe(2 * NOTE_MAX);
    expect(noteLength(faces)).toBe(NOTE_MAX);
    expect(noteLength(`  ${"a".repeat(NOTE_MAX)}\r\n`)).toBe(NOTE_MAX);
  });
});

/** A mirror row: by default one occurrence of "u1" on Bobby's main calendar, answerable. */
const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "me",
  summary: "Standup",
  description: null,
  location: null,
  start: "2026-10-12T09:00:00Z",
  end: "2026-10-12T09:30:00Z",
  all_day: false,
  html_link: null,
  uid: "u1",
  can_respond: true,
  ...over,
});

const calendar = (id: string, over: Partial<Calendar> = {}): Calendar => ({
  id,
  source_id: `gcal:${id}`,
  provider: "google",
  remote_id: `${id}@x.org`,
  name: id,
  color: null,
  selected: true,
  is_primary: true,
  quiet: false,
  kind: null,
  access_role: "owner",
  time_zone: null,
  default_reminders: [],
  conference_types: [],
  data_owner: null,
  ...over,
});

describe("declined events (C14b)", () => {
  const own = (id: string) => `${id}@x.org`;
  const mark = (kept: CalendarEvent[], visible: CalendarEvent[] = kept) =>
    markDeclined(kept, visible, own).map((e) => [e.id, isDeclined(e)]);

  it("strikes through a row the user declined, never one another guest declined", () => {
    const declined = row({ attendees: [entry("me@x.org", "declined", true)] });
    const going = row({
      id: "e2",
      uid: "u2",
      attendees: [entry("me@x.org", "accepted", true), entry("a@x.org", "declined")],
    });
    const nobody = row({ id: "e3", uid: null, attendees: [] });
    expect(mark([declined, going, nobody])).toEqual([
      ["e1", true],
      ["e2", false],
      ["e3", false],
    ]);
    // A row left plain is the same object; the marked one is a copy, the row itself untouched.
    const [first, second] = markDeclined([declined, going], [declined, going], own);
    expect(second).toBe(going);
    expect(first).not.toBe(declined);
    expect(isDeclined(declined)).toBe(false);
  });

  it("strikes a merged row through only when every copy with the user's entry declined", () => {
    const kept = row({ calendar_id: "work", attendees: [entry("work@x.org", "accepted", true)] });
    const mine = row({ id: "e2", attendees: [entry("me@x.org", "declined", true)] });
    // The kept copy (the organiser's, say) still going keeps it plain.
    expect(mark([kept], [kept, mine])).toEqual([["e1", false]]);
    // Declined on both: struck through.
    const both = { ...kept, attendees: [entry("work@x.org", "declined", true)] };
    expect(mark([both], [both, mine])).toEqual([["e1", true]]);
    // A copy without the user's own entry doesn't count either way.
    const theirs = { ...kept, attendees: [entry("me@x.org", "accepted")] };
    expect(mark([theirs], [theirs, mine])).toEqual([["e1", true]]);
    // Another occurrence of the same event (another start) is another row.
    const later = row({ id: "e3", start: "2026-10-19T09:00:00Z", attendees: mine.attendees });
    expect(mark([kept], [kept, later])).toEqual([["e1", false]]);
  });

  it("reads the calendar's own entry when two are marked, as the backend does", () => {
    const alias = [entry("Me+pm@x.org", "declined", true), entry("ME@x.org", "accepted", true)];
    expect(mark([row({ attendees: alias })])).toEqual([["e1", false]]);
    expect(mark([row({ calendar_id: "work", attendees: alias })])).toEqual([["e1", true]]);
  });

  it("says a declined event's name with ', declined' at the end", () => {
    expect(declinedName("09:00, Standup", true)).toBe("09:00, Standup, declined");
    expect(declinedName("09:00, Standup", false)).toBe("09:00, Standup");
    // The Focus agenda's rows say it themselves.
    expect(isDeclined({ ...row({}), declined: true } as CalendarEvent)).toBe(true);
  });
});

describe("the copies a reply can come from (C14b)", () => {
  const me = calendar("me");
  const work = calendar("work", { is_primary: false, name: "Work" });
  const team = calendar("team", { name: "Team" });
  const calendars = [me, work, team];
  const all = () => true;
  const ids = (copies: ReturnType<typeof replyCopies>) =>
    copies.map((c) => [c.event.id, c.forWhom]);

  it("offers one copy on a main calendar without a name", () => {
    const ev = row({});
    expect(ids(replyCopies([ev], ev, calendars, all))).toEqual([["e1", null]]);
  });

  it("names a lone copy on a calendar that isn't the account's main one", () => {
    const ev = row({ calendar_id: "work" });
    expect(ids(replyCopies([ev], ev, calendars, all))).toEqual([["e1", "Work"]]);
  });

  it("names every copy when there are two, the one shown first, the rest by calendar name", () => {
    const shown = row({ id: "e3", calendar_id: "work" });
    const rows = [row({ id: "e1", calendar_id: "team" }), row({ id: "e2" }), shown];
    expect(ids(replyCopies(rows, shown, calendars, all))).toEqual([
      ["e3", "Work"],
      ["e2", "me"],
      ["e1", "Team"],
    ]);
  });

  it("leaves out a copy the list can't answer, a hidden one, one with editing off, and Outlook's", () => {
    const shown = row({});
    const rows = [
      shown,
      row({ id: "e2", calendar_id: "work", can_respond: false }),
      row({ id: "e3", calendar_id: "team" }),
      row({ id: "e4", calendar_id: "outlook" }),
      row({ id: "e5", calendar_id: "gone" }),
    ];
    const outlook = calendar("outlook", { provider: "microsoft" });
    const offered = replyCopies(rows, shown, [...calendars, outlook], (c) => c.id !== "team");
    expect(ids(offered)).toEqual([["e1", null]]);
    // Nothing answerable, nothing offered.
    expect(replyCopies(rows, shown, calendars, () => false)).toEqual([]);
  });

  it("takes only the occurrence shown, and the row alone when it has no iCal UID", () => {
    const shown = row({ uid: null });
    const rows = [shown, row({ id: "e2", calendar_id: "team", uid: null })];
    expect(ids(replyCopies(rows, shown, calendars, all))).toEqual([["e1", null]]);
    const other = row({ id: "e3", start: "2026-10-19T09:00:00Z" });
    const occurrence = row({});
    expect(ids(replyCopies([occurrence, other], occurrence, calendars, all))).toEqual([
      ["e1", null],
    ]);
  });
});
