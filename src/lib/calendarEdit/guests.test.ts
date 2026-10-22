// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Guests in the editor (#884, C12). Two tables here are the backend's too, row for row
// (`calendar_write::attendees`): the address check (`an_address_is_checked_as_the_editor_checks_it`)
// and who a save would email (`notify_required_table`, N1-N15; `a_delete_tells_every_guest_but_you_
// and_rooms`, D1-D5). A row added to one is added to the other, so the editor asks first exactly
// when the backend would refuse to send unasked.

import { describe, expect, it } from "vitest";

import type { Attendee, Calendar, CalendarEvent, EventForEdit, GuestNotice } from "../types";
import { guestNotice, seedDraft, type EditorFields, type GuestDraft } from "./eventDraft";
import {
  changeNotice,
  deleteNotice,
  dontSendDetail,
  guestLines,
  guestSuggestions,
  guestSummary,
  isEmail,
  isRoomAddress,
  matchSuggestions,
  notifyOf,
  notifyQuestion,
  REPLY_GLYPH,
  replyOf,
  responseLabel,
  sameAddress,
  sendDetail,
  splitAddresses,
} from "./guests";

/** The backend's table, verbatim: (text, an address?). */
const ADDRESS_ROWS: [string, boolean][] = [
  ["a@b.co", true],
  ["A.B+tag@sub.x.org", true],
  ["a@b", false],
  ["a@@b.co", false],
  ["@b.co", false],
  ["a@b..co", false],
  ["a@-b.co", false],
  ["a b@c.co", false],
  ['"a"@b.co', false],
  ["a@b.co,", false],
  [`${"x".repeat(65)}@b.co`, false],
  [`a@${"b".repeat(250)}.co`, false],
  [" a@b.co ", true],
  ["o'brien@x.org", true],
];

describe("addresses", () => {
  it("are checked as the backend checks them", () => {
    expect(ADDRESS_ROWS[11][0]).toHaveLength(255);
    for (const [text, want] of ADDRESS_ROWS) expect([text, isEmail(text)]).toEqual([text, want]);
  });

  it("trim and refuse the backend's white space, not JavaScript's", () => {
    const nel = String.fromCharCode(0x85);
    const nbsp = String.fromCharCode(0xa0);
    // Rust's white space has U+0085 and JavaScript's doesn't: inside, it's refused by both.
    expect(isEmail(`a${nel}b@c.co`)).toBe(false);
    expect(isEmail(`${nel}a@b.co${nbsp}`)).toBe(true);
    expect(sameAddress(`${nel}A@b.co`, "a@B.CO ")).toBe(true);
  });

  it("know a room by Google's domain, whatever the case", () => {
    expect(isRoomAddress("Room1@Resource.Calendar.Google.com")).toBe(true);
    expect(isRoomAddress("room1@x.org")).toBe(false);
    expect(isRoomAddress("resource.calendar.google.com@x.org")).toBe(false);
  });

  it("split as typed or pasted together, a name's address read first", () => {
    expect(splitAddresses("a@b.co, C <c@d.org>; e@f.io")).toEqual(["a@b.co", "c@d.org", "e@f.io"]);
    // A name with spaces gives only its address: its words would stay in the box and stop the save.
    expect(splitAddresses("Jane Doe <j@x.org>")).toEqual(["j@x.org"]);
    expect(splitAddresses('"Doe, Jane" <j@x.org>, k@x.org')).toEqual(["j@x.org", "k@x.org"]);
    expect(splitAddresses("a@b.co\nb@c.co  c@d.co")).toEqual(["a@b.co", "b@c.co", "c@d.co"]);
    // An address typed before a name is kept; what isn't an address is left for the box.
    expect(splitAddresses("a@b.co Jane <j@x.org>")).toEqual(["a@b.co", "j@x.org"]);
    expect(splitAddresses("bob at x")).toEqual(["bob", "at", "x"]);
    expect(splitAddresses(" , ;")).toEqual([]);
  });

  it("split Outlook's unquoted “Last, First <a@b>” lists, and give each address once", () => {
    // Outlook copies recipients so: the comma is the name's, and no surname is left for the box.
    expect(splitAddresses("Smith, Jo <jo@x.org>; Doe, Al <al@x.org>")).toEqual([
      "jo@x.org",
      "al@x.org",
    ]);
    expect(splitAddresses("Smith, Jr., Jo <jo@x.org>")).toEqual(["jo@x.org"]);
    expect(splitAddresses("'Smith, Jo' <jo@x.org>")).toEqual(["jo@x.org"]);
    // An address before the comma is no name: kept.
    expect(splitAddresses("a@b.co, Jo <jo@x.org>")).toEqual(["a@b.co", "jo@x.org"]);
    // Names with no address after them stay, for the box.
    expect(splitAddresses("Smith, Jones")).toEqual(["Smith", "Jones"]);
    // An address with no name, as some mail apps copy one, and the same address twice.
    expect(splitAddresses("jo@x.org <jo@x.org>")).toEqual(["jo@x.org"]);
    expect(splitAddresses("jo@x.org, JO@x.org")).toEqual(["jo@x.org"]);
  });
});

describe("replies", () => {
  it("read Google's and Microsoft's words", () => {
    const rows: [string | null, string | null][] = [
      ["accepted", "yes"],
      ["declined", "no"],
      ["tentative", "maybe"],
      ["tentativelyAccepted", "maybe"],
      ["needsAction", "awaiting"],
      ["notResponded", "awaiting"],
      ["none", "awaiting"],
      ["organizer", null],
      ["something new", null],
      [null, null],
    ];
    for (const [word, want] of rows) expect([word, replyOf(word)]).toEqual([word, want]);
    expect(REPLY_GLYPH).toEqual({ yes: "✓", no: "✗", maybe: "?", awaiting: "" });
  });

  it("are labelled as the event popover always labelled them", () => {
    expect(
      ["accepted", "declined", "tentativelyAccepted", "needsAction", "organizer", null].map(
        responseLabel,
      ),
    ).toEqual(["accepted", "declined", "maybe", "no reply", "organiser", null]);
  });

  it("sum up as Google's line does, the organiser a guest and a room not", () => {
    const g = (email: string, response: string | null, room = false) => ({
      email,
      response,
      room,
    });
    expect(
      guestSummary([
        g("me@x.org", "accepted"),
        g("a@x.org", "declined"),
        g("b@x.org", "needsAction"),
        g("room1@x.org", "accepted", true),
        // A room the list doesn't flag is a room by its address.
        g("room2@resource.calendar.google.com", "accepted"),
      ]),
    ).toBe("3 guests: 1 yes, 1 no, 1 awaiting");
    expect(guestSummary([g("a@x.org", null)])).toBe("1 guest");
    expect(guestSummary([g("a@x.org", "tentative")])).toBe("1 guest: 1 maybe");
    expect(guestSummary([g("room1@x.org", null, true)])).toBe("");
  });
});

describe("suggestions", () => {
  const calendar = (
    id: string,
    source_id: string,
    provider: string,
    is_primary = false,
  ): Calendar => ({
    id,
    source_id,
    provider,
    remote_id: null,
    name: id,
    color: null,
    selected: true,
    is_primary,
    quiet: false,
    kind: null,
    access_role: null,
    time_zone: null,
    default_reminders: [],
    conference_types: [],
    data_owner: null,
  });
  const who = (email: string, name: string | null = null, self = false): Attendee => ({
    email,
    name,
    response: null,
    optional: false,
    organizer: false,
    self,
  });
  const event = (calendar_id: string, attendees: Attendee[]): CalendarEvent => ({
    id: `${calendar_id}:${attendees.length}:${attendees[0]?.email}`,
    calendar_id,
    summary: "",
    description: null,
    location: null,
    start: "2026-10-20T09:00:00Z",
    end: null,
    all_day: false,
    html_link: null,
    uid: null,
    attendees,
  });
  const calendars = [
    calendar("gcal:me@x.org:me@x.org", "gcal:me@x.org", "google", true),
    calendar("outlook:me@work.org:AAA", "outlook:me@work.org", "microsoft", true),
    // An Outlook-published feed calls itself "outlook": still a subscription (C12 critique C1-H3).
    calendar("ics:feed1", "ics:feed1", "outlook"),
    calendar("ics:feed2", "ics:feed2", "ical"),
  ];
  const events = [
    event("gcal:me@x.org:me@x.org", [
      who("me@x.org", "Me", true),
      who("Ann@x.org", "Ann"),
      who("bob@x.org"),
      who("room1@resource.calendar.google.com", "Room 1"),
    ]),
    event("gcal:me@x.org:me@x.org", [who("ann@X.org", "Ann Smith"), who("cat@x.org", "Cat")]),
    // Outlook never marks the user's own entry (`graph_attendees`): it's known by the account.
    event("outlook:me@work.org:AAA", [
      who("ANN@x.org", "Ann"),
      who("Me@Work.org", "Me"),
      who("dan@work.org", "Dan"),
      who("/O=EXCHANGELABS/OU=EXCHANGE ADMINISTRATIVE GROUP", "Legacy"),
    ]),
    event("ics:feed1", [who("stranger@x.org", "Stranger"), who("ann@x.org", "Ann")]),
    event("ics:feed2", [who("other@x.org", "Other")]),
  ];

  it("offer everyone in the user's own accounts' calendars, once, most seen first", () => {
    const all = guestSuggestions(events, calendars, "bob@x.org");
    expect(all).toEqual([
      // Seen three times in any case, under its commonest name; the feed's sighting not counted.
      { email: "Ann@x.org", name: "Ann", count: 3 },
      // Outlook accounts are kept; then by name.
      { email: "cat@x.org", name: "Cat", count: 1 },
      { email: "dan@work.org", name: "Dan", count: 1 },
    ]);
    // Never a subscription's people, the user (any calendar's own entry), a room, `me`, or
    // something that isn't an address.
    const emails = all.map((s) => s.email.toLowerCase());
    for (const never of ["stranger@x.org", "other@x.org", "me@x.org", "me@work.org", "bob@x.org"]) {
      expect(emails).not.toContain(never);
    }
    expect(guestSuggestions(events, calendars, null).map((s) => s.email)).toContain("bob@x.org");
  });

  it("know the user by every account's address, and a shared calendar's owner as a colleague", () => {
    // A colleague's calendar shared into the Google account: their own entry on it is marked as the
    // calendar's (Google's `self`), and the user's is not.
    const shared = calendar("gcal:me@x.org:boss@x.org", "gcal:me@x.org", "google");
    const theirs = [
      event(shared.id, [who("boss@x.org", "Boss", true), who("me@x.org", "Me"), who("eve@x.org")]),
      event("gcal:me@x.org:me@x.org", [who("me@x.org", "Me", true), who("boss@x.org", "Boss")]),
    ];
    // `me` is another account's here, so only the account addresses keep the user out.
    const all = guestSuggestions(theirs, [...calendars, shared], "other@y.org");
    expect(all.map((s) => s.email)).toEqual(["boss@x.org", "eve@x.org"]);
    expect(all[0]).toEqual({ email: "boss@x.org", name: "Boss", count: 2 });
  });

  it("never offer the user's own other calendars, while a colleague's shared one stays offered", () => {
    // "Work", a calendar the user made: on events they organise there, its own entry is marked as
    // the calendar's, under its group address.
    const WORK = "abc123@group.calendar.google.com";
    const work = {
      ...calendar(`gcal:me@x.org:${WORK}`, "gcal:me@x.org", "google"),
      remote_id: WORK,
    };
    const shared = {
      ...calendar("gcal:me@x.org:boss@x.org", "gcal:me@x.org", "google"),
      remote_id: "boss@x.org",
    };
    const theirs = [
      event(work.id, [{ ...who(WORK, "Work", true), organizer: true }, who("eve@x.org", "Eve")]),
      event(work.id, [{ ...who(WORK, "Work", true), organizer: true }, who("fay@x.org", "Fay")]),
      event(shared.id, [who("boss@x.org", "Boss", true), who("eve@x.org", "Eve")]),
    ];
    const all = guestSuggestions(theirs, [...calendars, work, shared], null);
    expect(all.map((s) => s.email)).toEqual(["eve@x.org", "boss@x.org", "fay@x.org"]);
  });

  it("match by name or address, leaving out who is on the list, up to the limit", () => {
    const all = guestSuggestions(events, calendars, null);
    expect(matchSuggestions(all, "an", []).map((s) => s.email)).toEqual([
      "Ann@x.org",
      "dan@work.org",
    ]);
    expect(matchSuggestions(all, "WORK", []).map((s) => s.email)).toEqual(["dan@work.org"]);
    expect(matchSuggestions(all, "an", ["ANN@x.org "]).map((s) => s.email)).toEqual([
      "dan@work.org",
    ]);
    expect(matchSuggestions(all, "x.org", [], 2)).toHaveLength(2);
    expect(matchSuggestions(all, "  ", [])).toEqual([]);
  });
});

// --- who a save would email: the backend's N1-N15, as the editor's fields ---

const guest = (email: string, over: Partial<GuestDraft> = {}): GuestDraft => ({
  email,
  name: null,
  response: "accepted",
  optional: false,
  organizer: false,
  self: false,
  room: false,
  ...over,
});
const ME = guest("me@x.org", { organizer: true, self: true });
const A = guest("A@x.org");
const B = guest("B@x.org", { response: "needsAction" });
const R = guest("room1@resource.calendar.google.com", { room: true, response: null });
const NEW_B = guest("b@x.org", { response: null });

const opened: EventForEdit = {
  summary: "Retro",
  location: "",
  description: "",
  description_html: false,
  time: {
    kind: "timed",
    start_date: "2026-10-20",
    start_time: "09:00",
    start_zone: "Europe/London",
    end_date: "2026-10-20",
    end_time: "10:00",
    end_zone: "Europe/London",
  },
  start_at: null,
  end_at: null,
  show_as: "busy",
  visibility: "default",
  attachments: [],
  html_link: null,
  guests: [],
  guests_can_modify: false,
  guests_can_invite_others: true,
  guests_can_see_other_guests: true,
  guests_omitted: false,
  organizer_email: "me@x.org",
};
const fields = (guests: GuestDraft[]): EditorFields => seedDraft({ ...opened, guests });
const retitled = (f: EditorFields): EditorFields => ({ ...f, summary: "Retro (moved)" });

const told = (updated: number, invited = 0, removed = 0): GuestNotice => ({
  updated,
  invited,
  removed,
  told_cancelled: 0,
  cancelled: false,
  people: updated + invited + removed,
  emails: updated + invited + removed,
  complete: true,
});

describe("who a save would email (the backend's N1-N15)", () => {
  it("asks exactly when the backend would", () => {
    const optionalA = { ...A, optional: true };
    const invitation = [
      guest("o@x.org", { organizer: true }),
      guest("me@x.org", { self: true }),
      A,
    ];
    const rows: [string, EditorFields, EditorFields, boolean, GuestNotice | null][] = [
      ["N1", fields([]), retitled(fields([])), false, null],
      ["N2", fields([ME]), retitled(fields([ME])), false, null],
      ["N3", fields([ME, R]), retitled(fields([ME, R])), false, null],
      ["N4", fields([ME, A]), retitled(fields([ME, A])), false, told(1)],
      ["N5", fields([ME, A]), { ...fields([ME, A]), show_as: "free" }, false, null],
      ["N6", fields([ME, A]), fields([ME, A, NEW_B]), false, told(1, 1)],
      ["N7", fields([ME]), fields([ME, NEW_B]), false, told(0, 1)],
      ["N8", fields([ME, A, B]), fields([ME, A]), false, told(1, 0, 1)],
      ["N9", fields([ME, A]), fields([ME, optionalA]), false, told(1)],
      // The editor opened on A awaiting; A's reply since changes nothing.
      [
        "N10",
        fields([ME, { ...A, response: "needsAction" }]),
        retitled(fields([ME, A])),
        false,
        told(1),
      ],
      ["N11", fields(invitation), retitled(fields(invitation)), false, told(2)],
      ["N12", fields([ME, A]), retitled(fields([ME, A])), true, { ...told(1), complete: false }],
      ["N13", fields([ME, A]), { ...fields([ME, A]), show_as: "free" }, true, null],
      [
        "N14",
        fields([ME, A]),
        {
          ...fields([ME, A]),
          guest_perms: { modify: true, invite_others: true, see_others: true },
        },
        false,
        told(1),
      ],
      ["N15", fields([ME, A]), fields([ME, A, R]), false, told(1)],
    ];
    for (const [name, base, draft, omitted, want] of rows) {
      expect([name, guestNotice(base, draft, omitted, "cal@x.org")]).toEqual([name, want]);
    }
  });

  it("never counts the user, marked or not", () => {
    // The account's own address, typed in where Google hasn't marked it, isn't a guest.
    const typed = guest("Cal@x.org", { response: null });
    expect(guestNotice(fields([ME]), fields([ME, typed]), false, "cal@x.org")).toBeNull();
    // The user's entry is the user under any spelling Google's copy marks.
    expect(changeNotice([ME], [ME, guest("ME@x.org")], true, false)).toBeNull();
    // Without the account, the same address is a new guest.
    expect(guestNotice(fields([ME]), fields([ME, typed]), false)).toEqual(told(0, 1));
  });

  it("never counts a room, by Google's flag or by its address", () => {
    const hall = guest("hall@x.org", { room: true, response: null });
    expect(changeNotice([ME], [ME, hall], true, false)).toBeNull();
    const unflagged = guest("room2@resource.calendar.google.com", { response: null });
    expect(changeNotice([ME], [ME, unflagged], true, false)).toBeNull();
  });

  it("with nobody seen but a list cut short, still asks, with no counts", () => {
    expect(guestNotice(fields([ME]), retitled(fields([ME])), true)).toEqual({
      ...told(0),
      complete: false,
    });
  });
});

describe("who a delete would tell (the backend's D1-D5, from the calendar's row)", () => {
  const who = (email: string, self = false): Attendee => ({
    email,
    name: null,
    response: null,
    optional: false,
    organizer: self,
    self,
  });
  const row = (attendees: Attendee[], organizer_self = true): CalendarEvent => ({
    id: "e1",
    calendar_id: "gcal:me@x.org:me@x.org",
    summary: "Retro",
    description: null,
    location: null,
    start: "2026-10-20T09:00:00Z",
    end: null,
    all_day: false,
    html_link: null,
    uid: null,
    attendees,
    organizer_self,
  });
  const cancelled = (n: number): GuestNotice => ({ ...told(n), cancelled: true });

  // D4 (a list Google cut short) has no mirror form: the row has no such flag, and the backend
  // asks with its own "at least" count when Google's copy says so.
  it("tells every guest but the user and rooms", () => {
    expect(deleteNotice(row([who("me@x.org", true)]))).toBeNull(); // D1
    expect(deleteNotice(row([who("me@x.org", true), who("A@x.org")]))).toEqual(cancelled(1)); // D2
    // D3: a room, by its address (the mirror has no room flag).
    expect(
      deleteNotice(row([who("me@x.org", true), who("room1@Resource.Calendar.Google.com")])),
    ).toBeNull();
    // The account's own address, unmarked in the mirror, is the user.
    expect(deleteNotice(row([who("me@x.org")]), "me@x.org")).toBeNull();
    // An event the user doesn't organise is no delete of theirs to tell anyone about (C14).
    expect(deleteNotice(row([who("me@x.org", true), who("A@x.org")], false))).toBeNull();
    expect(deleteNotice(row([]))).toBeNull();
  });

  // Not one of the backend's rows. The backend knows the organiser of an event the calendar
  // organises by the calendar's id, which `Own` holds; the mirror row doesn't carry that id, so the
  // editor knows it by the organiser's entry instead. Google marks that entry as the calendar's own
  // too, so the two agree on every copy it sends; this pins the editor's side should a copy ever
  // come without the mark.
  it("counts the organiser of an event the calendar organises as the user, marked or not", () => {
    const team = { ...who("team@group.x.org"), organizer: true };
    expect(deleteNotice(row([team, who("A@x.org")]))).toEqual(cancelled(1));
  });
});

// --- the words of the email question ---

describe("the email question", () => {
  it("asks as Google does, a delete always about cancellation", () => {
    expect(notifyQuestion(told(2), "save")).toBe("Send update emails to guests?");
    expect(notifyQuestion(told(0, 2), "save")).toBe("Send invitation emails to guests?");
    expect(notifyQuestion(told(0, 2), "create")).toBe("Send invitation emails to guests?");
    expect(notifyQuestion(told(0, 1, 1), "save")).toBe("Send update emails to guests?");
    expect(notifyQuestion(told(1, 1), "save")).toBe("Send update emails to guests?");
    expect(notifyQuestion({ ...told(2), cancelled: true }, "save")).toBe(
      "Send cancellation emails to guests?",
    );
    // A "this and following" delete's cut is a change, not a delete: the kind says it's one.
    expect(notifyQuestion(told(2), "delete")).toBe("Send cancellation emails to guests?");
  });

  it("says who gets what, each line only when it applies", () => {
    expect(guestLines(told(1), null, "save")).toEqual([
      "1 guest already invited will get an update.",
    ]);
    expect(guestLines(told(2, 1, 1), null, "save")).toEqual([
      "2 guests already invited will get an update.",
      "1 guest new to it will get an invitation.",
      "1 guest you took off will be told they're no longer invited.",
    ]);
    expect(guestLines(told(0, 3), null, "create")).toEqual([
      "3 guests new to it will get an invitation.",
    ]);
    expect(guestLines({ ...told(2), cancelled: true }, null, "delete")).toEqual([
      "2 guests will be told it's cancelled.",
    ]);
    // "One update covers every occurrence" waits for probe L8-11 (C12 critique C2-M6).
    expect(guestLines(told(2), "all", "save")).toEqual([
      "2 guests already invited will get an update.",
    ]);
  });

  // C12c review round 3: "Does not repeat" from an occurrence cuts the series and cancels a later
  // occurrence with a guest of its own, whom nothing else reaches: Google emails them a
  // cancellation, never an update.
  it("tells guests only an occurrence's cancel reaches that it's cancelled", () => {
    const cancelledOnly = (k: number, n: GuestNotice = told(0)): GuestNotice => ({
      ...n,
      told_cancelled: k,
      people: n.people + k,
      emails: n.emails + k,
    });
    expect(guestLines(cancelledOnly(1), "following", "save")).toEqual([
      "1 guest will be told an occurrence is cancelled.",
    ]);
    expect(guestLines(cancelledOnly(2, told(1)), "following", "save")).toEqual([
      "1 guest already invited will get an update.",
      "2 guests will be told an occurrence is cancelled.",
    ]);
    // Not an invitation-only question, with someone told of a cancel.
    expect(notifyQuestion(cancelledOnly(1, told(0, 1)), "save")).toBe(
      "Send update emails to guests?",
    );
    expect(guestLines(cancelledOnly(1, told(0, 1)), "following", "save")).toEqual([
      "1 guest will be told an occurrence is cancelled.",
      "1 guest new to it will get an invitation.",
    ]);
    // A delete keeps its own words: everyone it reaches is told it's cancelled.
    expect(guestLines(cancelledOnly(1, told(1)), "following", "delete")).toEqual([
      "2 guests will be told it's cancelled.",
    ]);
  });

  it("counts a 'this and following' change's emails, never for a delete", () => {
    const split = { ...told(2, 1), emails: 10 };
    expect(guestLines(split, "following", "save")).toEqual([
      "2 guests already invited will get an update.",
      "1 guest new to it will get an invitation.",
      "In all, about 10 emails: the new part is a new event, so its guests get an invitation to it, and an update about the earlier part and each occurrence moved or cancelled.",
    ]);
    // One email each: nothing more to say.
    expect(guestLines(told(2, 1), "following", "save")).toHaveLength(2);
    // A delete makes no new part (C12 critiques C1-M1, C2-M1), cancelled flag or not.
    for (const n of [
      { ...told(3), emails: 7 },
      { ...told(3), emails: 7, cancelled: true },
    ]) {
      expect(guestLines(n, "following", "delete")).toEqual([
        "3 guests will be told it's cancelled.",
      ]);
    }
  });

  it("says when PM can't tell for sure, with or without anyone counted", () => {
    expect(guestLines({ ...told(1), complete: false }, null, "save")).toEqual([
      "1 guest already invited will get an update.",
      "There may be more: PM can't tell for sure who else Google emails.",
    ]);
    // A short listing asks even with nobody seen (C12 critique C1), and says why.
    expect(guestLines({ ...told(0), complete: false }, "all", "save")).toEqual([
      "PM can't tell for sure whether Google emails anyone.",
    ]);
  });

  it("explains Send and Don't send", () => {
    expect(sendDetail("me@x.org")).toBe("Google sends the emails from me@x.org.");
    expect(sendDetail(null)).toBe("Google sends the emails.");
    expect(dontSendDetail("save")).toBe(
      "Nobody is emailed. Guests who don't use Google Calendar may never get the change: Google warns their calendars can stop updating, or lose the event.",
    );
    expect(dontSendDetail("delete")).toBe(
      "Nobody is emailed. Guests who don't use Google Calendar may never get the cancellation: Google warns their calendars can stop updating, or lose the event.",
    );
    expect(dontSendDetail("create")).toBe(
      "Nobody is emailed. Guests who don't use Google Calendar may never get the invitation: Google warns their calendars can stop updating, or lose the event.",
    );
    // Don't send is "none" until probe L8-6 says otherwise.
    expect(notifyOf("none")).toBe("none");
    expect(notifyOf("all")).toBe("all");
  });
});

describe("the module", () => {
  const SOURCE = Object.values(
    import.meta.glob("./guests.ts", { query: "?raw", import: "default", eager: true }),
  )[0] as string;

  // Suggestions and counts are pure frontend code over rows the view already holds: nothing here
  // can reach a calendar write (C12 spec: "no model reaches the write path").
  it("names no write wrapper or command, and imports nothing that runs", () => {
    expect(SOURCE.length).toBeGreaterThan(1000);
    for (const name of [
      "getCalendarEventForEdit",
      "updateCalendarEvent",
      "createCalendarEvent",
      "deleteCalendarEvent",
      "cancelCalendarDelete",
      "listHeldDeletes",
      "seriesChoices",
      "getSeriesSummary",
      "settleSeriesSplit",
      "update_calendar_event",
      "create_calendar_event",
      "delete_calendar_event",
      "invoke",
    ]) {
      expect([name, SOURCE.includes(name)]).toEqual([name, false]);
    }
    const imports = SOURCE.split("\n").filter((l) => /^import\b/.test(l));
    expect(imports.every((l) => /^import type\b/.test(l))).toBe(true);
  });
});
