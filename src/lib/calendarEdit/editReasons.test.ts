// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The switches are exhaustive by type; these pin that every value the backend sends has words, and
// the few lines whose wording carries a promise ("nothing was overwritten").

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  Calendar,
  CalendarEvent,
  CalendarOverview,
  GuestNotice,
  MilestoneMove,
  Notify,
  ReadOnlyReason,
  SeriesScope,
  SplitAction,
  WriteOutcome,
  WriteWarning,
} from "../types";
import {
  A11_NOTE,
  askConnectedText,
  askLabel,
  askOwnerFor,
  askText,
  CALENDAR_ALONE,
  CALENDAR_WHOLE,
  calendarRowText,
  changedText,
  CALLOUT_TONE,
  CREATE_CHANGED,
  CREATE_LOST,
  conflictFields,
  copyRank,
  createdText,
  createReasonText,
  createText,
  deleteText,
  dragReason,
  duplicable,
  emailsGuests,
  endsRetry,
  eventRights,
  heldCreateText,
  SPECIAL_DATE_DRAG,
  specialKind,
  type SpecialKind,
  moveBackText,
  moveText,
  saveBackText,
  undoMoveLabel,
  undoSaveLabel,
  type EventRights,
  retryAfter,
  earlierChangeText,
  keepLabel,
  leftBehindText,
  fieldLabel,
  listFields,
  loadText,
  milestoneStayedText,
  moveBackFailedText,
  moveBackLabel,
  movedBackText,
  movedText,
  moveFailedText,
  moveLabel,
  moveOfferText,
  NOT_MOVED_UNREAD,
  notMovedText,
  notMovedUnreadText,
  notOrganizerText,
  outcomeText,
  partialText,
  problemText,
  reasonText,
  rebasedText,
  repeatTheirsText,
  replyNoticeText,
  replyText,
  savedNotice,
  settleText,
  SPLIT_ORDER,
  splitActionLabel,
  writeWarningText,
} from "./editReasons";

/** One guest Google would email with Send (C12). */
const ONE_GUEST: GuestNotice = {
  updated: 1,
  invited: 0,
  removed: 0,
  told_cancelled: 0,
  cancelled: false,
  people: 1,
  emails: 1,
  complete: true,
};

// Dates drop their year inside the current one, so every DD-MM below is read on a fixed day.
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-18T12:00:00"));
});
afterEach(() => {
  vi.useRealTimers();
});

const REASONS: ReadOnlyReason[] = [
  "not_google",
  "editing_off",
  "calendar_read_only",
  "private_event",
  "not_organizer",
  "not_invited",
  "organizer_only_guests",
  "became_repeating",
  "special_type",
  "special_date_fields",
  "date_from_contacts",
  "date_from_profile",
  "from_gmail_fields",
  "locked",
  "unsupported_formatting",
  "label_read_only",
  "reminders_unreadable",
  "other_call",
];

describe("edit reasons", () => {
  it("has a distinct sentence for every reason the backend sends", () => {
    const lines = REASONS.map(reasonText);
    for (const line of lines) expect(line).toMatch(/^[A-Z].*\.$/);
    expect(new Set(lines).size).toBe(REASONS.length);
  });

  // C20: birthdays and Gmail events have Google's own list now; the rest of Google's special kinds
  // stay read-only, in words that no longer name them. No jargon: never Google's type names.
  it("words birthdays and Gmail events as Google's lists for them (C20)", () => {
    const special = reasonText("special_type");
    expect(special).toBe(
      "PM can't change focus time, out of office, working locations or other special Google events.",
    );
    expect(special).not.toMatch(/birthday|gmail|yet/i);
    expect(reasonText("special_date_fields")).toBe(
      "On a birthday or other special date, Google lets PM change only its name, notifications and colour, and its date if it was added in Google Calendar.",
    );
    expect(reasonText("date_from_contacts")).toBe(
      "This date comes from Google Contacts, so change it there.",
    );
    expect(reasonText("date_from_profile")).toBe(
      "This birthday comes from the calendar owner's Google Account, so change its date there.",
    );
    expect(reasonText("from_gmail_fields")).toBe(
      "Gmail added this event, so Google lets PM change only its busy or free, visibility, notifications and colour, and your reply if you're invited.",
    );
    for (const line of REASONS.map(reasonText)) {
      expect(line).not.toMatch(/eventType|fromGmail|birthdayProperties|transparency|series|RSVP/);
    }
  });

  // C20 critique C2-L13: a pure copy of the backend's `kind_of`, read the same way.
  it("specialKind reads Google's types as the backend does", () => {
    const rows: [string | null | undefined, SpecialKind | null][] = [
      [null, null],
      [undefined, null],
      ["default", null],
      ["birthday", "special_date"],
      ["fromGmail", "from_gmail"],
      ["focusTime", "other"],
      ["outOfOffice", "other"],
      ["x", "other"],
      ["Birthday", "other"],
      ["", "other"],
    ];
    for (const [type, kind] of rows) expect([type, specialKind(type)]).toEqual([type, kind]);
  });

  it("words a label and unreadable notifications plainly, as the backend does (C13)", () => {
    expect(reasonText("label_read_only")).toBe(
      "This event has a Google Calendar label, which sets its colour. Change it in Google Calendar.",
    );
    expect(reasonText("reminders_unreadable")).toBe(
      "PM can't read one of this event's notifications, so it leaves them as they are.",
    );
    for (const line of REASONS.map(reasonText)) expect(line).not.toMatch(/reminder/i);
  });

  it("words formatting PM can't edit as the backend does, never promising Google's look (C19)", () => {
    const line = reasonText("unsupported_formatting");
    expect(line).toBe(
      "This description has formatting PM can't edit. PM shows what it can of it here; Google Calendar shows all of it.",
    );
    expect(line).not.toMatch(/html|rich text|markup|tag/i);
  });

  // C18: another service's call, Google's old Hangouts or a bare link, in one sentence, never
  // Google's own word for a call.
  it("words a call PM can't change plainly, as the backend does (C18)", () => {
    expect(reasonText("other_call")).toBe(
      "This event's video call is a kind PM can't change, so PM leaves it as it is. Change it in Google Calendar.",
    );
    for (const line of REASONS.map(reasonText)) expect(line).not.toMatch(/conference|add-on/i);
    expect(fieldLabel("meet")).toBe("video call");
  });

  it("names conflicting fields in the editor's words", () => {
    // A key the editor has no field for names nothing.
    expect(conflictFields(["start", "end", "summary", "transparency", "attachments"])).toEqual([
      "time",
      "summary",
      "show_as",
    ]);
    // C18: the call is the video-call row's (critique C1-L5).
    expect(conflictFields(["conferenceData", "summary"])).toEqual(["meet", "summary"]);
    expect(listFields(conflictFields(["conferenceData"]))).toBe("video call");
    // Notifications and colour (C13): a colour's check watches the label too, and both are the
    // colour, said once.
    expect(conflictFields(["eventLabelId"])).toEqual(["colour"]);
    expect(conflictFields(["colorId"])).toEqual(["colour"]);
    expect(conflictFields(["colorId", "eventLabelId", "reminders"])).toEqual([
      "colour",
      "reminders",
    ]);
    expect(listFields(["reminders", "colour"])).toBe("notifications and colour");
    // The guest list and its three permissions (C12), each said once.
    expect(
      conflictFields([
        "attendees",
        "guestsCanModify",
        "guestsCanInviteOthers",
        "guestsCanSeeOtherGuests",
      ]),
    ).toEqual(["guests", "guest_perms"]);
    expect(listFields(["guests", "guest_perms"])).toBe("guests and guest permissions");
    expect(listFields(["summary"])).toBe("title");
    expect(listFields(["summary", "time", "location"])).toBe("title, time and location");
    // How it repeats, in Google's word for it (C11).
    expect(conflictFields(["recurrence", "start"])).toEqual(["recurrence", "time"]);
    expect(fieldLabel("recurrence")).toBe("repeat");
    expect(listFields(["time", "recurrence"])).toBe("time and repeat");
  });

  it("after taking Google's copy, says whose guest change went, never that the list is Google's", () => {
    const head = "Google changed this event while you were editing.";
    expect(rebasedText(head, [], [])).toBe(
      `${head} Your changes are still here; save again to apply them.`,
    );
    expect(rebasedText(head, ["summary"], [])).toBe(
      `${head} Google's version of the title is shown now; your other changes are still here.`,
    );
    // The guest list merges: only the change to a guest Google changed too was dropped.
    const guests = rebasedText(head, ["guests"], ["Ann"]);
    expect(guests).toBe(
      `${head} Google changed Ann on the guest list too, so your change to that guest wasn't kept; your other changes are still here.`,
    );
    expect(guests).not.toMatch(/version of the guests/);
    expect(rebasedText(head, ["summary", "guests"], ["Ann", "bo@x.org"])).toBe(
      `${head} Google's version of the title is shown now. Google changed Ann and bo@x.org on the guest list too, so your changes to those guests weren't kept; your other changes are still here.`,
    );
  });

  it("explains every outcome, promising on a conflict only that nothing was saved over it", () => {
    const outcomes: WriteOutcome[] = [
      { outcome: "saved", warnings: [] },
      { outcome: "saved", warnings: ["mirror_refresh_pending"] },
      { outcome: "saved", warnings: ["left_on_old_series"], left_behind: ["2026-11-02"] },
      { outcome: "no_change" },
      { outcome: "needs_choice", scope: true },
      { outcome: "needs_choice", scope: false, guests: ONE_GUEST },
      { outcome: "ignored_by_google", split: false },
      { outcome: "ignored_by_google", split: true },
      { outcome: "partial_split", token: "t", offers: ["finish"], from: "2026-11-02" },
      { outcome: "partial_split", token: "t", offers: ["remove", "check"], from: "2026-11-02" },
      { outcome: "split_removed" },
      { outcome: "conflict", fields: ["summary"] },
      { outcome: "conflict", fields: [] },
      { outcome: "gone" },
      { outcome: "read_only", reason: "locked" },
      { outcome: "busy" },
      { outcome: "reauth" },
      { outcome: "unconfirmed" },
      { outcome: "ambiguous", retry_token: "ref-1" },
      { outcome: "failed", message: "Google didn't accept the change: Invalid start" },
      { outcome: "not_moved", message: "Google is busy right now." },
    ];
    for (const o of outcomes) {
      expect(outcomeText(o).text.length).toBeGreaterThan(0);
      expect(deleteText(o, "Dentist").text).toContain("“Dentist”");
    }
    // Whether Google emails the guests is never worded as a choice of occurrences (C12): the
    // editor says this rather than asking which occurrences, which would loop.
    expect(outcomeText({ outcome: "needs_choice", scope: false, guests: ONE_GUEST })).toEqual({
      tone: "warn",
      text: "Choose whether Google emails the guests. Nothing was saved.",
    });
    expect(outcomeText({ outcome: "needs_choice", scope: true }).text).toMatch(
      /^This repeating event changed since you chose which occurrences/,
    );
    // The named fields are the ones the editor will show Google's version of, so the line doesn't
    // promise the user's edits to them survive.
    expect(outcomeText({ outcome: "conflict", fields: ["summary", "start"] })).toEqual({
      tone: "warn",
      text: "This event changed in Google while you were editing (the title and time). Nothing was saved over it.",
    });
    expect(outcomeText({ outcome: "saved", warnings: [] }).tone).toBe("ok");
    expect(outcomeText({ outcome: "failed", message: "x" })).toEqual({ tone: "error", text: "x" });
    expect(outcomeText({ outcome: "read_only", reason: "locked" }).text).toBe(reasonText("locked"));
  });

  it("words a change PM couldn't finish by what Google allows now", () => {
    const partial = (offers: ("remove" | "finish" | "check")[]) =>
      outcomeText({ outcome: "partial_split", token: "t", offers, from: "2026-11-02" });
    // Finish alone: the earlier part already ends before the day, so it's saved, and never a hint
    // to delete anything.
    expect(partial(["finish"]).text).toMatch(/^Saved from 02-11.*Press Finish\.$/);
    expect(partial(["finish"]).text).not.toMatch(/delete|remove/i);
    // Finish or Remove: the original wasn't cut, so it may show twice; never "Saved".
    expect(partial(["finish", "remove"]).text).toMatch(/may show twice from 02-11/);
    expect(partial(["finish", "remove"]).text).not.toMatch(/^Saved/);
    expect(partial(["remove", "check"]).text).toMatch(/Remove the new part, or check again\.$/);
    // Check alone is also what a new part changed in Google leaves: never "lost touch" (C2-M5).
    expect(partial(["check"]).text).toMatch(
      /^PM couldn't finish this change from 02-11\. .*only if it shows twice/,
    );
    expect(partial(["check"]).text).not.toMatch(/lost touch/);
    for (const offers of [["finish"], ["remove"], ["check"]] as const) {
      expect(partial([...offers]).tone).toBe("warn");
    }
    expect(outcomeText({ outcome: "ignored_by_google", split: true }).text).toMatch(
      /removed the new part/,
    );
    expect(outcomeText({ outcome: "split_removed" }).tone).toBe("ok");
    // A "this and following" save that couldn't move every occurrence says so, hedged.
    expect(
      outcomeText({
        outcome: "saved",
        warnings: ["left_on_old_series"],
        left_behind: ["2026-11-02"],
      }),
    ).toEqual({
      tone: "warn",
      text: "Saved from that day on. Some occurrences you changed on their own may still be on the earlier part, or hidden by Google (02-11); check them in Google Calendar.",
    });
    expect(
      deleteText(
        { outcome: "partial_split", token: "t", offers: ["check"], from: "2026-11-02" },
        "Standup",
      ).text,
    ).toMatch(
      /^“Standup” wasn't deleted\. PM is still finishing an earlier change to it from 02-11/,
    );
  });

  it("speaks of a delete as a delete, naming the event", () => {
    // A delete that landed comes back as `saved`, and one that found it gone got what it wanted.
    expect(deleteText({ outcome: "saved", warnings: [] }, "Dentist")).toEqual({
      tone: "ok",
      text: "Deleted “Dentist”.",
    });
    expect(deleteText({ outcome: "gone" }, "Dentist").tone).toBe("ok");
    expect(deleteText({ outcome: "conflict", fields: ["start", "end"] }, "Dentist")).toEqual({
      tone: "warn",
      text: "“Dentist” wasn't deleted. It changed in Google since you opened it (the time).",
    });
    expect(deleteText({ outcome: "busy" }, " ").text).toBe(
      "“(no title)” wasn't deleted. Google is busy right now; try again in a moment.",
    );
    // An answer that never arrived is never reported as "wasn't deleted": it may have been.
    expect(deleteText({ outcome: "unconfirmed" }, "Dentist")).toEqual({
      tone: "warn",
      text: "PM lost touch with Google before it could confirm whether “Dentist” was deleted. Refresh the calendar to see.",
    });
    // A series delete that left occurrences changed on their own says they may still show.
    expect(
      deleteText(
        { outcome: "saved", warnings: ["left_on_old_series"], left_behind: ["2026-11-02"] },
        "Standup",
      ),
    ).toEqual({
      tone: "warn",
      text: "Deleted “Standup” from that day on. Some occurrences changed on their own may still show (02-11); check them in Google Calendar.",
    });
    // A delete the backend wants the occurrences chosen for, first or again, deleted nothing; a
    // held one can't reopen the dialog, so the words say to delete again.
    expect(deleteText({ outcome: "needs_choice", scope: true }, "Standup")).toEqual({
      tone: "warn",
      text: "“Standup” wasn't deleted. PM needs to ask which occurrences first. Delete it again to choose.",
    });
    // Whether Google emails the guests is no choice of occurrences (C12): guests nobody had
    // answered about, or numbers changed since the answer (said neutrally, C2-L6, and true when
    // Google dropped the guests after the last sync).
    expect(
      deleteText({ outcome: "needs_choice", scope: false, guests: ONE_GUEST }, "Standup"),
    ).toEqual({
      tone: "warn",
      text: "“Standup” wasn't deleted. PM found guests who would be told. Delete it again to choose whether Google emails them.",
    });
    expect(
      deleteText(
        { outcome: "needs_choice", scope: false, guests: ONE_GUEST, again: true },
        "Standup",
      ).text,
    ).toBe(
      "“Standup” wasn't deleted. Who Google would email has changed since you chose. Delete it again to choose whether Google emails the guests.",
    );
    expect(deleteText({ outcome: "ignored_by_google", split: false }, "Standup").text).toMatch(
      /^“Standup” wasn't deleted\./,
    );
  });

  it("ambiguous has words", () => {
    // A new event whose answer never arrived: it may be there, and Check again looks before it
    // sends. Never "wasn't created", never a promise that no second copy can be made (Google can't
    // always catch a repeated id), and never the token in the words.
    const said = outcomeText({ outcome: "ambiguous", retry_token: "ref-1" });
    expect(said).toEqual({
      tone: "warn",
      text: "PM lost touch with Google while creating this event, so it may already be in your calendar. Check again: PM looks for it first and sends it again only if Google still doesn't show it.",
    });
    expect(said.text).not.toContain("ref-1");
    expect(settleText({ outcome: "ambiguous", retry_token: "ref-1" }, "Dentist").tone).toBe("warn");
  });

  it("explains an editor that couldn't open, and a draft that can't be saved", () => {
    // A 404 can also mean the calendar stopped being visible, so "gone" doesn't claim a deletion.
    expect(loadText({ outcome: "gone" })).toBe(
      "This event is no longer in Google: it was deleted there, or this account can't see it any more.",
    );
    expect(loadText({ outcome: "failed", message: "Offline." })).toBe("Offline.");
    expect(problemText({ kind: "gap", half: "start", zone: "Europe/London", time: "01:30" })).toBe(
      "01:30 doesn't happen in Europe/London that night: the clocks skip it. Pick a time outside that hour.",
    );
    expect(problemText({ kind: "unreadable", half: "end", what: "zone" })).toBe(
      "The end time zone isn't one PM knows.",
    );
    expect(problemText({ kind: "bad_guest", email: "bob at x" })).toBe(
      "“bob at x” isn't an email address.",
    );
  });
});

const saved = (warnings: WriteWarning[], left_behind?: string[]) =>
  ({ outcome: "saved", warnings, left_behind }) as const;

describe("a write's warnings", () => {
  it("words every warning, for a save and for a delete", () => {
    const all: WriteWarning[] = [
      "mirror_refresh_pending",
      "left_on_old_series",
      "outside_mirror",
      "milestone_not_offered",
      "partly_applied",
      "kept_own_reply",
      "call_pending",
      "call_failed",
      "links_left_behind",
    ];
    for (const w of all) {
      for (const kind of ["save", "delete"] as const) {
        expect(writeWarningText(w, kind)).toMatch(/^[A-Z].*\.$/);
      }
    }
    // C18d: it names the calendar the event went to, when it's known.
    expect(writeWarningText("links_left_behind", "save", undefined, "Work")).toBe(
      "On Work it became a new event in Google, so the milestone or reminder to prepare linked to it stays with the old one.",
    );
    expect(writeWarningText("mirror_refresh_pending", "save")).toBe(
      "PM's calendar will show it after the next refresh.",
    );
    // A delete's occurrences "may still show"; a save's may be on the earlier part.
    expect(writeWarningText("left_on_old_series", "delete")).toMatch(/may still show;/);
    expect(writeWarningText("left_on_old_series", "save", ["2026-11-02"])).toMatch(
      /hidden by Google \(02-11\); check them/,
    );
  });

  it("names up to five days left behind, then how many more", () => {
    expect(leftBehindText()).toBe("");
    expect(leftBehindText([])).toBe("");
    const days = [
      "2026-11-02",
      "2026-11-09",
      "2026-11-16",
      "2026-11-23",
      "2026-11-30",
      "2026-12-07",
      "2027-01-04",
    ];
    expect(leftBehindText(days.slice(0, 5))).toBe(" (02-11, 09-11, 16-11, 23-11, 30-11)");
    expect(leftBehindText(days)).toBe(" (02-11, 09-11, 16-11, 23-11, 30-11 and 2 more)");
  });

  it("maps every tone onto a Callout's", () => {
    expect(CALLOUT_TONE).toEqual({ ok: "info", warn: "warning", error: "danger" });
  });
});

// C18d: putting an event on another calendar, in the words the editor and its notice say.
describe("another calendar (C18d)", () => {
  const moved = { to: "Work", from: "Home" };

  it("says where a saved event is now and how to put it back, from the backend's answer", () => {
    const outcome: Extract<WriteOutcome, { outcome: "saved" }> = {
      outcome: "saved",
      warnings: [],
      calendar: "gcal:me@x.com:work",
    };
    expect(savedNotice("Dentist", outcome, null, "2026-10-12", false, false, null, moved)).toEqual({
      tone: "ok",
      text: "“Dentist” is now on Work. To put it back, open it and choose Home.",
    });
    // Links left with the old event: said, naming the calendar, and worth a look.
    expect(
      savedNotice(
        "Dentist",
        { ...outcome, warnings: ["links_left_behind"] },
        null,
        undefined,
        false,
        false,
        null,
        moved,
      ),
    ).toEqual({
      tone: "warn",
      text: "“Dentist” is now on Work. On Work it became a new event in Google, so the milestone or reminder to prepare linked to it stays with the old one. To put it back, open it and choose Home.",
    });
    // No calendar change: a save's own words, as before.
    expect(savedNotice("Dentist", { outcome: "saved", warnings: [] }, null).text).toBe(
      "Saved “Dentist”.",
    );
  });

  it("says every occurrence of a repeating event is there now (C18e)", () => {
    const outcome: Extract<WriteOutcome, { outcome: "saved" }> = {
      outcome: "saved",
      warnings: ["mirror_refresh_pending"],
      calendar: "gcal:me@x.com:work",
    };
    // "All events", and "This and following" from the first occurrence, which saved every one.
    for (const [scope, asAll] of [
      ["all", false],
      ["following", true],
    ] as const) {
      expect(savedNotice("Gym", outcome, scope, "2026-10-19", false, asAll, null, moved)).toEqual({
        tone: "ok",
        text: `Every occurrence of “Gym” is now on Work. ${writeWarningText("mirror_refresh_pending", "save")} To put it back, open it and choose Home.`,
      });
    }
    // Saved for every occurrence on its own calendar: as before.
    expect(savedNotice("Gym", { outcome: "saved", warnings: [] }, "all").text).toBe(
      "Saved “Gym” for every occurrence. To change it back, edit it again.",
    );
  });

  // Review round 3 of PR 1: with Save held, no message offers it. A backend message that offers
  // another go has a row here: a test in Rust looks each one up in this file, word for word.
  it.each([
    [
      "Its guests changed in Google just now, so PM didn't change its calendar. Save again to answer for them.",
      "Its guests changed in Google just now, so PM didn't change its calendar.",
    ],
    [
      "PM lost touch with Google, and the event is still on its calendar. Try again.",
      "PM lost touch with Google, and the event is still on its calendar.",
    ],
    [
      "PM couldn't reach Google to change its calendar. Try again when you're back online.",
      "PM couldn't reach Google to change its calendar.",
    ],
    [
      "PM couldn't get this account's Google sign-in ready to change its calendar. Try again in a moment.",
      "PM couldn't get this account's Google sign-in ready to change its calendar.",
    ],
    [
      "PM couldn't ask Google about this event, so its calendar didn't change. Try again.",
      "PM couldn't ask Google about this event, so its calendar didn't change.",
    ],
    ["PM couldn't change its calendar. Try again.", "PM couldn't change its calendar."],
    // What offers nothing stays whole, and a sentence that isn't an offer isn't mistaken for one.
    ["Google is busy right now.", "Google is busy right now."],
    [
      "Google needs you to sign in to this account again.",
      "Google needs you to sign in to this account again.",
    ],
  ])("a held save's words offer no other go: %j", (message, reason) => {
    expect(notMovedUnreadText(message, "Home")).toBe(
      `Your other changes are saved, but it's still on Home. ${reason} ${NOT_MOVED_UNREAD}`,
    );
  });

  it("a held save's words leave the calendar's name and the warnings alone", () => {
    expect(notMovedUnreadText("Try again.", "St. Try again. Club", ["call_failed"])).toBe(
      `Your other changes are saved, but it's still on St. Try again. Club. Google didn't make the video call. Add it again from Edit, or in Google Calendar. ${NOT_MOVED_UNREAD}`,
    );
    expect(notMovedUnreadText("Try again.", null)).toBe(
      `Your other changes are saved, but its calendar didn't change. ${NOT_MOVED_UNREAD}`,
    );
  });

  it("says a save whose calendar change didn't land kept the rest, and why", () => {
    expect(notMovedText("Google is busy right now.", "Home")).toBe(
      "Your other changes are saved, but it's still on Home. Google is busy right now.",
    );
    expect(outcomeText({ outcome: "not_moved", message: "Try again." })).toEqual({
      tone: "warn",
      text: "Your other changes are saved, but its calendar didn't change. Try again.",
    });
    // Never any other write's answer, but each says what it carries rather than nothing.
    const notMoved: WriteOutcome = { outcome: "not_moved", message: "Try again." };
    expect(replyText(notMoved, "yes", null).text).toBe("Your reply wasn't sent.");
    expect(saveBackText(notMoved, "Gym").text).toMatch(/^Couldn't undo the save of “Gym”\./);
    expect(moveText(notMoved, "Gym", "", null).text).toMatch(/Try again\.$/);
    expect(moveBackText(notMoved, "Gym").text.length).toBeGreaterThan(0);
    expect(settleText(notMoved, "Gym").text).toMatch(/^“Gym”: /);
    expect(createText(notMoved, "Gym", false).text.length).toBeGreaterThan(0);
    // Review round 1 of PR 1: what the fields had to say beside "saved" is said too, never dropped.
    expect(
      outcomeText({
        outcome: "not_moved",
        message: "Try again.",
        warnings: ["call_failed", "partly_applied"],
      }).text,
    ).toBe(
      "Your other changes are saved, but its calendar didn't change. Try again. Google didn't keep all of your changes, so check the event. Google didn't make the video call. Add it again from Edit, or in Google Calendar.",
    );
    expect(notMovedText("Try again.", "Home", ["call_pending"])).toBe(
      "Your other changes are saved, but it's still on Home. Try again. Google is still making the video call; its link shows after a later refresh.",
    );
  });

  // Review round 1 of PR 1: the editor's Power-depth line names another calendar chosen.
  it("names the calendar among the changes", () => {
    expect(changedText([], true)).toBe("calendar");
    expect(changedText(["summary", "time"], true)).toBe("title, time and calendar");
    expect(changedText(["summary"], false)).toBe("title");
  });

  it("says under the Calendar row whose calendars are listed, and Duplicate only where it helps", () => {
    expect(calendarRowText("me@x.com", false)).toBe(
      "Only calendars you own on me@x.com are listed.",
    );
    expect(calendarRowText("me@x.com", true)).toBe(
      "Only calendars you own on me@x.com are listed. To put it on another account's calendar, duplicate it there, then delete this one.",
    );
    expect(CALENDAR_ALONE).toBe("Change the calendar on its own, then how it repeats.");
    // A repeating event goes whole (C18e), never claimed as what Google does with each occurrence.
    expect(CALENDAR_WHOLE).toBe("Changing the calendar is for the whole repeating event.");
    // A lock keeps it on its calendar, and says so; someone else's names the calendar as theirs.
    expect(reasonText("locked")).toBe(
      "Google has locked this event's title, time, place and description, so PM also keeps it on its calendar.",
    );
    expect(reasonText("not_organizer")).toMatch(/guests, video call and calendar\./);
    // Never C16's word for a drag, nor Google's API's, for a calendar change.
    for (const line of [
      calendarRowText("me@x.com", true),
      CALENDAR_ALONE,
      CALENDAR_WHOLE,
      notMovedText("x.", "Home"),
      writeWarningText("links_left_behind", "save", undefined, "Work"),
    ]) {
      expect(line).not.toMatch(/move|transfer/i);
    }
  });
});

describe("what a reply came to (C14)", () => {
  it("says the reply in Google's words, and all events as Google says it", () => {
    expect(replyText(saved([]), "no", null)).toEqual({ tone: "ok", text: "You replied No." });
    expect(replyText(saved([]), "maybe", "this").text).toBe("You replied Maybe.");
    expect(replyText(saved([]), "yes", "all")).toEqual({
      tone: "ok",
      text: "You replied Yes for all events.",
    });
    expect(replyText(saved(["mirror_refresh_pending"]), "yes", null).text).toBe(
      "You replied Yes. PM's calendar will show it after the next refresh.",
    );
    expect(replyText({ outcome: "no_change" }, "yes", null)).toEqual({
      tone: "ok",
      text: "That's already your reply.",
    });
  });

  // C14 critiques C1-M3, C2-H1: "all events" never hides an occurrence that kept its own answer.
  it("says how many occurrences answered on their own kept their answer", () => {
    expect(
      replyText(saved(["kept_own_reply"], ["2026-10-19", "2026-10-26"]), "yes", "all"),
    ).toEqual({
      tone: "warn",
      text: "You replied Yes for all events. 2 occurrences you answered on their own kept their answer.",
    });
    expect(replyText(saved(["kept_own_reply"], ["2026-10-19"]), "no", "all").text).toBe(
      "You replied No for all events. 1 occurrence you answered on its own kept its answer.",
    );
  });

  it("has words for every outcome, never a promise it went when it may not have", () => {
    const outcomes: WriteOutcome[] = [
      { outcome: "needs_choice", scope: true },
      { outcome: "ignored_by_google", split: false },
      { outcome: "partial_split", token: "t", offers: ["check"], from: "2026-11-02" },
      { outcome: "split_removed" },
      { outcome: "conflict", fields: [] },
      { outcome: "gone" },
      { outcome: "read_only", reason: "not_invited" },
      { outcome: "busy" },
      { outcome: "reauth" },
      { outcome: "unconfirmed" },
      { outcome: "ambiguous", retry_token: "t" },
      { outcome: "failed", message: "Google wouldn't take this reply from PM." },
    ];
    for (const o of outcomes) {
      const said = replyText(o, "yes", null);
      expect(said.text).toMatch(/^[A-Z“].*\.$/);
      expect(said.text).not.toMatch(/^You replied/);
      expect(said.tone).not.toBe("ok");
      expect(said.text).not.toMatch(/rsvp|attendee|series|instance|organizer/i);
    }
    // Google answered as if it took it, and doesn't hold it: never "You replied".
    expect(replyText({ outcome: "ignored_by_google", split: false }, "yes", null).text).toBe(
      "Google didn't take the reply. Try again, or reply in Google Calendar.",
    );
    expect(replyText({ outcome: "read_only", reason: "not_invited" }, "no", null).text).toBe(
      "PM can only reply where this calendar is on the guest list itself (not through a group).",
    );
    expect(replyText({ outcome: "unconfirmed" }, "no", null).text).toMatch(
      /whether it went through/,
    );
  });

  // C14 critique C2-M1: the notice that outlives the popover names the event.
  it("says it as a notice naming the event, in the same tone", () => {
    expect(replyNoticeText(saved([]), "no", null, "Standup")).toEqual({
      tone: "ok",
      text: "You replied No to “Standup”.",
    });
    expect(replyNoticeText(saved([]), "yes", "all", " ")).toEqual({
      tone: "ok",
      text: "You replied Yes to all events of “(no title)”.",
    });
    expect(
      replyNoticeText(
        saved(["mirror_refresh_pending", "kept_own_reply"], ["2026-10-19"]),
        "yes",
        "all",
        "Gym",
      ),
    ).toEqual({
      tone: "warn",
      text: "You replied Yes to all events of “Gym”. PM's calendar will show it after the next refresh. 1 occurrence you answered on its own kept its answer.",
    });
    expect(replyNoticeText({ outcome: "no_change" }, "yes", null, "Gym")).toEqual({
      tone: "ok",
      text: "That's already your reply to “Gym”.",
    });
    expect(replyNoticeText({ outcome: "busy" }, "yes", null, "Gym")).toEqual({
      tone: "warn",
      text: "Your reply to “Gym”: Google is busy right now. Reply again in a moment.",
    });
    expect(replyNoticeText({ outcome: "gone" }, "yes", null, "Gym").tone).toBe("error");
  });
});

// C14 critiques C1-L10, C2-L1, C2-L10: under someone else's event, what the user may still do.
describe("the line under someone else's event (C14b, C14c)", () => {
  // The video call is the organiser's too (C18), and so is the calendar (C18d).
  const theirs =
    "Someone else organises this event, so only they can change its title, time, place, description, visibility, guests, video call and calendar.";

  it("says what stays the organiser's and what the user may change, the reply only when this copy can", () => {
    // The reason alone never promises a reply: the calendar may not be on the guest list.
    expect(reasonText("not_organizer")).toBe(
      `${theirs} Your notifications, colour and busy or free are yours to change.`,
    );
    expect(notOrganizerText(true, true)).toBe(
      `${theirs} Your reply, notifications, colour and busy or free are yours to change.`,
    );
    expect(notOrganizerText(false, true)).toBe(reasonText("not_organizer"));
    expect(notOrganizerText(true, false)).toBe(
      "Someone else organises this event. Turn on editing for this account to reply and change your own settings from PM.",
    );
    expect(notOrganizerText(false, false)).toBe(
      "Someone else organises this event. Turn on editing for this account to change your own settings from PM.",
    );
  });

  it("says the guest list is the organiser's, and why a group invitation can't be answered", () => {
    expect(reasonText("organizer_only_guests")).toBe(
      "In PM, only the organiser can change who's invited and the video call.",
    );
    expect(reasonText("not_invited")).toBe(
      "PM can only reply where this calendar is on the guest list itself (not through a group).",
    );
  });

  it("words the A11 note and the Repeat row's line without jargon", () => {
    expect(A11_NOTE).toBe(
      "The organiser lets guests change this event. Your changes show on your copy, and may be lost if the organiser changes the event.",
    );
    expect(repeatTheirsText(true)).toBe(
      "Someone else organises this repeating event, so only they can change how it repeats.",
    );
    expect(repeatTheirsText(false)).toBe(
      "Someone else organises this event, so only they can make it repeat.",
    );
    for (const line of [A11_NOTE, repeatTheirsText(true), repeatTheirsText(false)]) {
      expect(line).not.toMatch(/series|recurr|attendee|organizer|guestsCanModify|instance/i);
    }
  });
});

describe("a repeating event's save, said once the editor has closed", () => {
  it("says how to change it back exactly when there's a scope and nothing was left behind", () => {
    for (const scope of [null, "this", "following", "all"] as const) {
      for (const outcome of [
        saved([]),
        saved(["mirror_refresh_pending"]),
        saved(["left_on_old_series"], ["2026-11-02"]),
      ]) {
        const said = savedNotice("Gym", outcome, scope, "2026-10-21");
        const left = outcome.warnings.includes("left_on_old_series");
        expect(said.text.includes("To change it back"), `${scope} ${outcome.warnings}`).toBe(
          (scope === "this" || scope === "all") && !left,
        );
        expect(said.tone).toBe(left ? "warn" : "ok");
        expect(said.text).toContain("“Gym”");
        expect(said.text).not.toMatch(/series|split|instance|exception|master/i);
      }
    }
  });

  it("words each scope, a one-off's as before", () => {
    expect(savedNotice("Gym", saved([]), null).text).toBe("Saved “Gym”.");
    expect(savedNotice("Gym", saved(["mirror_refresh_pending"]), null).text).toBe(
      "Saved “Gym” to Google. PM's calendar will show it after the next refresh.",
    );
    expect(savedNotice("Gym", saved([]), "this").text).toBe(
      "Saved “Gym” for this occurrence. To change it back, edit it again.",
    );
    expect(savedNotice("Gym", saved([]), "all").text).toBe(
      "Saved “Gym” for every occurrence. To change it back, edit it again.",
    );
    expect(savedNotice("Gym", saved(["mirror_refresh_pending"]), "all").text).toBe(
      "Saved “Gym” for every occurrence. PM's calendar will show it after the next refresh. To change it back, edit it again.",
    );
  });

  // C16d: with an Undo on offer the button says how to change it back, and after Send the words say
  // an Undo emails the guests again. Only one event or "This event" ever has one.
  it("words a save with an Undo, each scope, and after Send", () => {
    const said = (scope: SeriesScope | null, sent: Notify | null) =>
      savedNotice("Gym", saved([]), scope, "2026-10-21", false, false, { sent }).text;
    expect(said(null, null)).toBe("Saved “Gym”.");
    expect(said("this", null)).toBe("Saved “Gym” for this occurrence.");
    expect(said(null, "all")).toBe("Saved “Gym”. Guests will get a second email if you undo.");
    expect(said("this", "external_only")).toBe(
      "Saved “Gym” for this occurrence. Guests will get a second email if you undo.",
    );
    expect(said(null, "none")).toBe("Saved “Gym”.");
    // Several occurrences never have one: their words are as ever, whatever is passed.
    expect(said("all", "all")).toBe(savedNotice("Gym", saved([]), "all").text);
    expect(said("following", "all")).toBe(
      savedNotice("Gym", saved([]), "following", "2026-10-21").text,
    );
    expect(undoSaveLabel("Gym", null)).toBe("Undo saving “Gym”");
    expect(undoSaveLabel("Gym", "all")).toBe("Undo saving “Gym” (guests get a second email)");
  });

  it("says what undoing a save came to, in a save's words", () => {
    expect(saveBackText(saved([]), "Gym")).toEqual({
      tone: "ok",
      text: "Put “Gym” back as it was.",
    });
    expect(saveBackText({ outcome: "conflict", fields: ["summary"] }, "Gym")).toEqual({
      tone: "warn",
      text: "“Gym” changed in Google since, so PM didn't undo the save.",
    });
    expect(saveBackText({ outcome: "failed", message: "Google said no." }, "Gym")).toEqual({
      tone: "error",
      text: "Couldn't undo the save of “Gym”. Google said no.",
    });
    expect(saveBackText(saved(["mirror_refresh_pending"]), "Gym").text).toBe(
      "Put “Gym” back in Google. PM's calendar will show it after the next refresh.",
    );
    const all: WriteOutcome[] = [
      { outcome: "no_change" },
      { outcome: "needs_choice", scope: true },
      { outcome: "ignored_by_google", split: false },
      { outcome: "partial_split", token: "s", offers: ["check"], from: "2026-10-20" },
      { outcome: "split_removed" },
      { outcome: "gone" },
      { outcome: "read_only", reason: "locked" },
      { outcome: "busy" },
      { outcome: "reauth" },
      { outcome: "unconfirmed" },
      { outcome: "ambiguous", retry_token: "r" },
    ];
    for (const o of all) {
      const text = saveBackText(o, "Gym").text;
      expect(text).toMatch(/\.$/);
      expect(text).not.toMatch(/series|instance|exception|master|drag/i);
    }
  });

  // C13 review round 1: a save Google applied without keeping all of it (a colour it wouldn't clear
  // beside a new title) is saved, never "nothing changed", and never a plain "Saved": it says to look.
  it("says when Google didn't keep all of a save", () => {
    expect(savedNotice("Gym", saved(["partly_applied"]), null)).toEqual({
      tone: "warn",
      text: "Saved “Gym”. Google didn't keep all of your changes, so check the event.",
    });
    expect(savedNotice("Gym", saved(["mirror_refresh_pending", "partly_applied"]), null)).toEqual({
      tone: "warn",
      text: "Saved “Gym” to Google. Google didn't keep all of your changes, so check the event. PM's calendar will show it after the next refresh.",
    });
    expect(outcomeText(saved(["partly_applied"]))).toEqual({
      tone: "warn",
      text: "Saved to Google. Google didn't keep all of your changes, so check the event.",
    });
    expect(outcomeText(saved(["partly_applied", "mirror_refresh_pending"])).text).toBe(
      "Saved to Google. Google didn't keep all of your changes, so check the event. PM's calendar will show it after the next refresh.",
    );
  });

  // C18 §13.3: a save that asked for a Meet call says when Google is still making it (the link can
  // wait for a later sync, critique C1-L11), and when Google made none, which is worth a look.
  it("says when Google is still making the video call, or made none", () => {
    expect(savedNotice("Gym", saved(["call_pending"]), null)).toEqual({
      tone: "ok",
      text: "Saved “Gym”. Google is still making the video call; its link shows after a later refresh.",
    });
    expect(savedNotice("Gym", saved(["call_failed"]), "this")).toEqual({
      tone: "warn",
      text: "Saved “Gym” for this occurrence. Google didn't make the video call. Add it again from Edit, or in Google Calendar. To change it back, edit it again.",
    });
    expect(savedNotice("Gym", saved(["call_pending", "partly_applied"]), null)).toEqual({
      tone: "warn",
      text: "Saved “Gym”. Google didn't keep all of your changes, so check the event. Google is still making the video call; its link shows after a later refresh.",
    });
  });

  // C11 §10: a one-off made to repeat whose occurrences all fall outside the months PM syncs is in
  // Google but won't show here, which the notice says, and keeps.
  it("says when a one-off made to repeat won't show here", () => {
    expect(savedNotice("Gym", saved(["outside_mirror"]), null)).toEqual({
      tone: "warn",
      text: "Saved “Gym” to Google. It starts outside the months PM syncs, so PM's calendar won't show it.",
    });
  });

  // C2-H2: a "this and following" save makes a repeating event of its own; editing it again never
  // joins it back, so the notice doesn't promise that.
  it("never promises a 'this and following' save can be changed back", () => {
    expect(savedNotice("Gym", saved([]), "following", "2026-10-21").text).toBe(
      "Saved “Gym” from 21-10 on, as a repeating event of its own. There's no Undo: editing it again changes that part, but won't join it back to the earlier one.",
    );
    expect(
      savedNotice("Gym", saved(["left_on_old_series"], ["2026-11-02"]), "following", "2026-10-21"),
    ).toEqual({
      tone: "warn",
      text: "Saved “Gym” from 21-10 on, as a repeating event of its own. Some occurrences you changed on their own may still be on the earlier part, or hidden by Google (02-11); check them in Google Calendar.",
    });
  });

  // C11c review and C11d review 1: "Does not repeat" leaves one event, never a repeating one, nor
  // every occurrence. From one occurrence on it's a separate event, as the choices said; for every
  // occurrence, the one kept is the first still there, so no day is named.
  it("says a stop left one event that doesn't repeat", () => {
    expect(savedNotice("Gym", saved([]), "following", "2026-10-19", true)).toEqual({
      tone: "ok",
      text: "Saved “Gym” from 19-10 on, as a separate event that doesn't repeat. There's no Undo: editing it again changes that event, but won't join it back to the earlier one.",
    });
    expect(savedNotice("Gym", saved([]), "all", "2026-10-19", true)).toEqual({
      tone: "ok",
      text: "Saved “Gym” as one event that doesn't repeat. There's no Undo: to make it repeat again, edit it and pick a repeat.",
    });
  });

  // C11c review and C11d review 1: a save that leaves its one event outside the months PM syncs
  // says so and warns. PM's calendar won't show it, so it can't be edited again here: the notice
  // points to Google Calendar, after a stop as after any other save.
  it("never says to edit again what PM won't show", () => {
    expect(savedNotice("Gym", saved(["outside_mirror"]), "all", "2026-10-19", true)).toEqual({
      tone: "warn",
      text: "Saved “Gym” as one event that doesn't repeat. It starts outside the months PM syncs, so PM's calendar won't show it. To change it back, use Google Calendar.",
    });
    expect(savedNotice("Standup", saved(["outside_mirror"]), "all")).toEqual({
      tone: "warn",
      text: "Saved “Standup” for every occurrence. It starts outside the months PM syncs, so PM's calendar won't show it. To change it back, use Google Calendar.",
    });
    expect(savedNotice("Gym", saved(["outside_mirror"]), "following", "2026-10-19", true)).toEqual({
      tone: "warn",
      text: "Saved “Gym” from 19-10 on, as a separate event that doesn't repeat. It starts outside the months PM syncs, so PM's calendar won't show it. There's no Undo: to change it, use Google Calendar.",
    });
    for (const scope of ["this", "following", "all"] as const) {
      for (const stops of [false, true]) {
        for (const asAll of [false, true]) {
          const said = savedNotice(
            "Gym",
            saved(["outside_mirror"]),
            scope,
            "2026-10-19",
            stops,
            asAll,
          );
          expect(said.text, `${scope} ${stops} ${asAll}`).not.toMatch(/edit(ing)? it again/);
        }
      }
    }
  });

  // C11c review round 2: the old occurrences still show until the next refresh, so the calendar
  // catching up is still said. The pair "Does not repeat" for every occurrence answers when Google's
  // list of what's left stops short, and from one occurrence on when PM's calendar couldn't be
  // written.
  it("says the calendar catches up when the old occurrences still show beside an event outside", () => {
    const both = saved(["outside_mirror", "mirror_refresh_pending"]);
    expect(savedNotice("Standup", both, "all", "2026-10-19", true)).toEqual({
      tone: "warn",
      text: "Saved “Standup” as one event that doesn't repeat. It starts outside the months PM syncs, so PM's calendar won't show it. PM's calendar will catch up after the next refresh. To change it back, use Google Calendar.",
    });
    expect(savedNotice("Standup", both, "following", "2026-12-06", true)).toEqual({
      tone: "warn",
      text: "Saved “Standup” from 06-12 on, as a separate event that doesn't repeat. It starts outside the months PM syncs, so PM's calendar won't show it. PM's calendar will catch up after the next refresh. There's no Undo: to change it, use Google Calendar.",
    });
  });

  // C11c review round 2: "This and following" on the first occurrence saves every occurrence, so its
  // notice is All's: no part made, no earlier one, and a stop names the one event left.
  it("words a 'this and following' save from the first occurrence as every occurrence", () => {
    expect(savedNotice("Gym", saved([]), "following", "2026-10-05", false, true)).toEqual({
      tone: "ok",
      text: "Saved “Gym” for every occurrence. To change it back, edit it again.",
    });
    expect(savedNotice("Gym", saved([]), "following", "2026-10-06", true, true)).toEqual({
      tone: "ok",
      text: "Saved “Gym” as one event that doesn't repeat, on 06-10. There's no Undo: to make it repeat again, edit it and pick a repeat.",
    });
    // Outside the months PM syncs, it points to Google Calendar as All does.
    expect(
      savedNotice("Gym", saved(["outside_mirror"]), "following", "2026-10-06", true, true).text,
    ).toBe(
      "Saved “Gym” as one event that doesn't repeat, on 06-10. It starts outside the months PM syncs, so PM's calendar won't show it. To change it back, use Google Calendar.",
    );
    // Only that row: All's own stop names no day, and any other row picked keeps its words.
    expect(savedNotice("Gym", saved([]), "all", "2026-10-19", true, false).text).toBe(
      "Saved “Gym” as one event that doesn't repeat. There's no Undo: to make it repeat again, edit it and pick a repeat.",
    );
    expect(savedNotice("Gym", saved([]), "this", "2026-10-19", false, true).text).toBe(
      "Saved “Gym” for this occurrence. To change it back, edit it again.",
    );
  });
});

describe("settling a change PM couldn't finish", () => {
  it("labels its buttons, always in one order", () => {
    expect(SPLIT_ORDER.map(splitActionLabel)).toEqual([
      "Finish",
      "Remove the new part",
      "Check again",
    ]);
    const offers: SplitAction[] = ["check", "remove"];
    expect(SPLIT_ORDER.filter((a) => offers.includes(a))).toEqual(["remove", "check"]);
  });

  it("says what each answer came to, naming the event", () => {
    const outcomes: WriteOutcome[] = [
      { outcome: "saved", warnings: [] },
      { outcome: "saved", warnings: ["left_on_old_series"], left_behind: ["2026-11-02"] },
      { outcome: "no_change" },
      { outcome: "needs_choice", scope: true },
      { outcome: "ignored_by_google", split: true },
      { outcome: "partial_split", token: "t", offers: ["finish"], from: "2026-11-02" },
      { outcome: "split_removed" },
      { outcome: "conflict", fields: [] },
      { outcome: "gone" },
      { outcome: "read_only", reason: "editing_off" },
      { outcome: "busy" },
      { outcome: "reauth" },
      { outcome: "unconfirmed" },
      { outcome: "failed", message: "x" },
    ];
    for (const o of outcomes) expect(settleText(o, "Gym").text).toContain("“Gym”");
    expect(settleText({ outcome: "saved", warnings: [] }, "Gym")).toEqual({
      tone: "ok",
      text: "Finished the change to “Gym”.",
    });
    expect(
      settleText(
        { outcome: "saved", warnings: ["left_on_old_series"], left_behind: ["2026-11-02"] },
        "Gym",
      ).tone,
    ).toBe("warn");
    // A milestone that stayed has its own notice naming it (C9b-2): not said twice.
    expect(settleText({ outcome: "saved", warnings: ["milestone_not_offered"] }, "Gym")).toEqual({
      tone: "ok",
      text: "Finished the change to “Gym”.",
    });
    expect(settleText({ outcome: "split_removed" }, "Gym")).toEqual({
      tone: "ok",
      text: "Removed the new part of “Gym”. The repeating event is as it was.",
    });
    // Still held: the backend's words when it gave some, else what Google allows now.
    expect(
      settleText(
        {
          outcome: "partial_split",
          token: "t",
          offers: ["check"],
          from: "2026-11-02",
          note: "PM couldn't read every occurrence it has to check, so it didn't finish. Try again in a moment.",
        },
        "Gym",
      ),
    ).toEqual({
      tone: "warn",
      text: "“Gym”: PM couldn't read every occurrence it has to check, so it didn't finish. Try again in a moment.",
    });
    expect(
      settleText(
        { outcome: "partial_split", token: "t", offers: ["finish"], from: "2026-11-02" },
        "Gym",
      ).text,
    ).toMatch(/^“Gym”: Saved from 02-11/);
    // PM's own claim comes back held, with a note; a plain busy is Google's, after Finish was taken
    // back (C9b-1a review round 2).
    expect(settleText({ outcome: "busy" }, "Gym").text).toBe(
      "“Gym”: Google is busy right now, so PM put the repeating event back as it was. Make the change again in a moment.",
    );
    expect(settleText({ outcome: "failed", message: "x" }, "Gym")).toEqual({
      tone: "error",
      text: "“Gym”: x",
    });
  });

  // C11c review round 3: a Finish words what landed as a save does. Outside the months PM syncs,
  // PM's calendar won't show it, so it catches up rather than shows it, and that's a warning.
  it("words a Finish that lands outside the months PM syncs as a save says it", () => {
    const both = saved(["outside_mirror", "mirror_refresh_pending"]);
    expect(settleText(both, "Standup")).toEqual({
      tone: "warn",
      text: "Finished the change to “Standup”. It starts outside the months PM syncs, so PM's calendar won't show it. PM's calendar will catch up after the next refresh.",
    });
    expect(settleText(saved(["outside_mirror"]), "Standup").tone).toBe("warn");
    expect(settleText(saved(["mirror_refresh_pending"]), "Standup")).toEqual({
      tone: "ok",
      text: "Finished the change to “Standup”. PM's calendar will show it after the next refresh.",
    });
  });

  it("says an earlier change blocks this one, hinting at deleting only when Remove is offered", () => {
    const cases: [SplitAction[], RegExp][] = [
      [["finish", "remove"], /Finish it or remove the new part first\.$/],
      [["finish"], /Finish it first\.$/],
      [["remove", "check"], /Remove the new part, or check again\.$/],
      [["check"], /Check again; if it still can't, look at that day in Google Calendar\.$/],
    ];
    for (const [offers, words] of cases) {
      const text = earlierChangeText(offers, "2026-11-02");
      expect(text).toMatch(/^PM is still finishing an earlier change to it from 02-11\. /);
      expect(text).toMatch(words);
      if (!offers.includes("remove")) expect(text).not.toMatch(/delete|remove/i);
    }
    // A delete blocked by one: no "Settle that first", the buttons are on its notice.
    expect(
      deleteText(
        { outcome: "partial_split", token: "t", offers: ["finish"], from: "2026-11-02" },
        "Gym",
      ).text,
    ).toBe(
      "“Gym” wasn't deleted. PM is still finishing an earlier change to it from 02-11. Finish it first.",
    );
  });

  // C12 critiques C1-M4, C2-M2: a change that went with Send says, wherever it's held, which
  // settles would email guests, before a button is pressed; only offers that would, and only after
  // a Send (the backend's `emails`).
  it("says which settles would email guests, after a Send", () => {
    const held: WriteOutcome = {
      outcome: "partial_split",
      token: "t",
      offers: ["finish", "remove"],
      from: "2026-11-02",
      emails: ["finish", "remove"],
    };
    const both =
      "If you remove the new part, guests will get a cancellation email. If you finish it, guests will get the rest of the update emails.";
    expect(outcomeText(held).text).toMatch(
      new RegExp(`Finish the change, or remove the new part\\. ${both}$`),
    );
    expect(settleText(held, "Gym").text).toMatch(new RegExp(` ${both}$`));
    expect(settleText({ ...held, note: "Try again in a moment." }, "Gym").text).toBe(
      `“Gym”: Try again in a moment. ${both}`,
    );
    expect(deleteText(held, "Gym").text).toMatch(
      new RegExp(`remove the new part first\\. ${both}$`),
    );
    expect(earlierChangeText(["remove", "check"], "2026-11-02", ["remove"])).toBe(
      "PM is still finishing an earlier change to it from 02-11. Remove the new part, or check again. If you remove the new part, guests will get a cancellation email.",
    );
    // Don't send, or nobody to tell: nothing said.
    expect(outcomeText({ ...held, emails: undefined }).text).not.toMatch(/guests/);
    // Only what's offered now.
    expect(partialText(["remove", "check"], "2026-11-02", ["finish"])).not.toMatch(/guests/);
  });
});

// --- creating an event (#884, C10) ---

const made = (warnings: WriteWarning[] = []): Extract<WriteOutcome, { outcome: "saved" }> => ({
  outcome: "saved",
  warnings,
  left_behind: [],
});
const lost: WriteOutcome = { outcome: "failed", message: CREATE_LOST };
const changed: WriteOutcome = { outcome: "failed", message: CREATE_CHANGED };

describe("creating an event", () => {
  // The webview ends a retry token only on these: anything else may leave the first copy in Google.
  it("drops a retry token only on saved, gone, CREATE_LOST or CREATE_CHANGED", () => {
    const ends: WriteOutcome[] = [made(), { outcome: "gone" }, lost, changed];
    const keeps: WriteOutcome[] = [
      { outcome: "ambiguous", retry_token: "ref-1" },
      { outcome: "reauth" },
      { outcome: "busy" },
      { outcome: "read_only", reason: "editing_off" },
      { outcome: "failed", message: "PM couldn't reach Google." },
      { outcome: "unconfirmed" },
    ];
    for (const o of ends) {
      expect(endsRetry(o)).toBe(true);
      expect(retryAfter("ref-1", o)).toBeNull();
    }
    for (const o of keeps) {
      expect(endsRetry(o)).toBe(false);
      expect(retryAfter("ref-1", o)).toBe("ref-1");
    }
    // A first create holds a token only when its answer never arrived.
    expect(retryAfter(null, { outcome: "ambiguous", retry_token: "ref-2" })).toBe("ref-2");
    expect(retryAfter(null, { outcome: "busy" })).toBeNull();
    expect(retryAfter(null, { outcome: "failed", message: "Offline." })).toBeNull();
  });

  it("says Created, and where PM's calendar won't show it", () => {
    const here = { name: "Work", hidden: false };
    expect(createdText("Dentist", made(), here)).toEqual({
      tone: "ok",
      text: "Created “Dentist”.",
    });
    expect(createdText("  ", made(), here).text).toBe("Created “(no title)”.");
    expect(createdText("Dentist", made(), { name: "Work", hidden: true })).toEqual({
      tone: "warn",
      text: "Created “Dentist” in Work, which is hidden here.",
    });
    expect(createdText("Dentist", made(["outside_mirror"]), here)).toEqual({
      tone: "warn",
      text: "Created “Dentist” in Google. It starts outside the months PM syncs, so it won't show here.",
    });
    expect(createdText("Dentist", made(["mirror_refresh_pending"]), here)).toEqual({
      tone: "ok",
      text: "Created “Dentist” in Google. PM's calendar will show it after the next refresh.",
    });
  });

  // C18b: a call the new event asked for that Google is still making is said, and one it didn't
  // make is a warning that stays.
  it("says what came of a new event's video call", () => {
    const here = { name: "Work", hidden: false };
    expect(createdText("Dentist", made(["call_pending"]), here)).toEqual({
      tone: "ok",
      text: "Created “Dentist”. Google is still making the video call; its link shows after a later refresh.",
    });
    expect(createdText("Dentist", made(["call_failed"]), here)).toEqual({
      tone: "warn",
      text: "Created “Dentist”. Google didn't make the video call. Add it again from Edit, or in Google Calendar.",
    });
    expect(createdText("Dentist", made(["mirror_refresh_pending", "call_pending"]), here)).toEqual({
      tone: "ok",
      text: "Created “Dentist” in Google. PM's calendar will show it after the next refresh. Google is still making the video call; its link shows after a later refresh.",
    });
    expect(createdText("Dentist", made(["call_pending"]), { name: "Work", hidden: true })).toEqual({
      tone: "warn",
      text: "Created “Dentist” in Work, which is hidden here. Google is still making the video call; its link shows after a later refresh.",
    });
  });

  it("never says a create PM may have made wasn't made", () => {
    const ambiguous = createText({ outcome: "ambiguous", retry_token: "ref-1" }, "Dentist", true);
    expect(ambiguous.tone).toBe("warn");
    expect(ambiguous.text).toBe(
      "PM lost touch with Google while creating “Dentist”, so it may already be in your calendar. Check again: PM looks for it first and sends it again only if Google still doesn't show it.",
    );
    expect(ambiguous.text).not.toContain("ref-1");
    // Held, the calendar can't be changed, so the refusal never asks for another one, and it says
    // the first send may have landed.
    const refused = createText(
      { outcome: "read_only", reason: "calendar_read_only" },
      "Dentist",
      true,
    ).text;
    expect(refused).not.toMatch(/Choose another/);
    expect(refused).toMatch(/^“Dentist” wasn't sent again: .* PM may already have made it: /);
    for (const o of [
      { outcome: "reauth" },
      { outcome: "busy" },
      { outcome: "read_only", reason: "editing_off" },
      { outcome: "failed", message: "Not now." },
    ] as WriteOutcome[]) {
      const said = createText(o, "Dentist", true);
      expect(said.tone).toBe("warn");
      expect(said.text).not.toMatch(/wasn't created/);
    }
    expect(heldCreateText("Dentist")).toEqual({
      tone: "warn",
      text: "PM may already have made “Dentist”. Check again: PM looks for it in Google first and sends it again only if it isn't there.",
    });
  });

  it("names what wasn't created, and why", () => {
    expect(
      createText({ outcome: "failed", message: "PM couldn't reach Google." }, "Dentist", false),
    ).toEqual({ tone: "error", text: "“Dentist” wasn't created: PM couldn't reach Google." });
    expect(
      createText({ outcome: "read_only", reason: "calendar_read_only" }, "Dentist", false).text,
    ).toBe("“Dentist” wasn't created: This calendar doesn't let you add events. Choose another.");
    expect(createReasonText("private_event")).toBe(
      "This calendar's sharing doesn't let you make private events on it.",
    );
    expect(createReasonText("editing_off")).toBe(reasonText("editing_off"));
    expect(createText({ outcome: "gone" }, "Dentist", false)).toEqual({
      tone: "warn",
      text: "PM made “Dentist”, but it has since been deleted in Google.",
    });
    // The token PM no longer holds: never "create it again" without looking first.
    expect(createText(lost, "Dentist", false)).toEqual({
      tone: "warn",
      text: "PM lost touch with Google and can't tell whether “Dentist” was made. Look for it in Google Calendar before creating it again, so it isn't made twice.",
    });
    expect(createText(changed, "Dentist", false).text).toMatch(
      /^“Dentist” wasn't sent again: .* so it isn't made twice\.$/,
    );
  });

  // C12d: whether Google sends the invitations, said only where it can't be asked again; nothing
  // was sent, and the words never say "guests" emailed or name a command.
  it("says a create waiting on the invitations question wasn't created", () => {
    const one = {
      updated: 0,
      invited: 1,
      removed: 0,
      told_cancelled: 0,
      cancelled: false,
      people: 1,
      emails: 1,
      complete: true,
    };
    expect(
      createText({ outcome: "needs_choice", scope: false, guests: one }, "Dentist", false),
    ).toEqual({
      tone: "warn",
      text: "“Dentist” wasn't created: Choose whether Google sends the invitations, then save it again.",
    });
    expect(
      createText(
        { outcome: "needs_choice", scope: false, guests: one, again: true },
        "Dentist",
        false,
      ),
    ).toEqual({
      tone: "warn",
      text: "“Dentist” wasn't created: PM's count of who Google would invite differs from the one you answered. Save it again to choose whether Google sends the invitations.",
    });
  });
});

// C9b-2: moving a milestone after a "this and following" change.
describe("move words", () => {
  const launch: MilestoneMove = {
    milestoneId: 7,
    label: "Launch",
    project: "Atlas",
    newTitle: "Retro",
    earlierTitle: "Gym",
    toIcal: "tail@google.com",
    toDate: "2026-10-19",
    backIcal: "abc@google.com",
    backDate: "2026-10-19",
    earlierLastDay: "2026-10-12",
    today: "2026-10-20",
    pending: false,
  };

  it("names the new part by its title and first day, and what saying no costs", () => {
    expect(moveOfferText(launch)).toBe(
      "Move the “Launch” milestone in Atlas to the new part of “Retro”, which starts on 19-10? If you don't, it stays on the earlier part, which ended on 12-10, and shows as overdue.",
    );
    // Days PM can't say are left out; a year outside this one is said; a blank title has a name.
    expect(
      moveOfferText({ ...launch, newTitle: "  ", toDate: null, earlierLastDay: "2025-12-29" }),
    ).toBe(
      "Move the “Launch” milestone in Atlas to the new part of “(no title)”? If you don't, it stays on the earlier part, which ended on 29-12-2025, and shows as overdue.",
    );
    expect(moveOfferText({ ...launch, earlierLastDay: null })).toMatch(
      /stays on the earlier part, which has ended, and shows as overdue\.$/,
    );
  });

  it("names each part by its own title once the change retitled the new one", () => {
    expect(moveLabel(launch)).toBe("Move “Launch” to the new part of “Retro”");
    expect(keepLabel(launch)).toBe("Keep it there: “Launch” stays on the earlier part of “Gym”");
    expect(movedText(launch)).toBe("Moved “Launch” to the new part of “Retro”.");
    expect(moveBackLabel(launch)).toBe("Move back “Launch” to the earlier part of “Gym”");
    expect(movedBackText(launch)).toBe("“Launch” is back on the earlier part of “Gym”.");
  });

  it("says when PM's calendar will show a move to a part it hasn't shown yet", () => {
    expect(movedText({ ...launch, pending: true })).toBe(
      "Moved “Launch” to the new part of “Retro”. PM's calendar will show its new date after the next refresh.",
    );
  });

  it("says why a move failed, and the way out when a move back did", () => {
    expect(moveFailedText(launch, "That milestone no longer exists.")).toBe(
      "Couldn't move “Launch”. That milestone no longer exists.",
    );
    expect(moveBackFailedText(launch, "That milestone changed since; nothing was moved.")).toBe(
      "Couldn't move “Launch” back. That milestone changed since; nothing was moved. Unlink it in Atlas if it shouldn't follow the new part.",
    );
  });

  it("says which milestone stayed when PM couldn't find the new part", () => {
    expect(milestoneStayedText(launch)).toBe(
      "PM couldn't find the new part of “Retro” in Google, so “Launch” in Atlas stays on the earlier part, which ended on 12-10, and shows as overdue. Unlink it in Atlas to set its date yourself.",
    );
  });

  // C9b-2 review round 3, R3-F3: the earlier part's last day still to come, as when an occurrence
  // before the change was moved past it.
  it("says an earlier part whose last day is today or later hasn't ended", () => {
    expect(moveOfferText({ ...launch, earlierLastDay: "2026-10-21" })).toBe(
      "Move the “Launch” milestone in Atlas to the new part of “Retro”, which starts on 19-10? If you don't, it stays on the earlier part, whose last day is 21-10, and shows as overdue after that.",
    );
    expect(milestoneStayedText({ ...launch, earlierLastDay: "2026-10-21" })).toBe(
      "PM couldn't find the new part of “Retro” in Google, so “Launch” in Atlas stays on the earlier part, whose last day is 21-10, and shows as overdue after that. Unlink it in Atlas to set its date yourself.",
    );
    // Today is its last day; yesterday's has ended.
    expect(moveOfferText({ ...launch, earlierLastDay: "2026-10-20" })).toMatch(
      /stays on the earlier part, whose last day is 20-10, and shows as overdue after that\.$/,
    );
    expect(milestoneStayedText({ ...launch, earlierLastDay: "2026-10-19" })).toMatch(
      /stays on the earlier part, which ended on 19-10, and shows as overdue\. Unlink/,
    );
  });
});

// One predicate for the popover's Edit and Delete, the dedup and a drag (C16a, §6.4).
describe("an event's rights in the calendar", () => {
  const ACCOUNT = "gcal:me@x.org";
  const CAL = "gcal:me@x.org:me@x.org";
  // Only what `eventRights` reads of the overview: each calendar's provider and account, and the
  // account's editing status.
  const overview = (editing: "on" | "off") =>
    ({
      calendars: [
        { id: CAL, provider: "google", source_id: ACCOUNT },
        { id: "ical:feed", provider: "ical", source_id: "ical:feed" },
      ],
      editing: { [ACCOUNT]: editing },
    }) as unknown as CalendarOverview;
  const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
    id: `${CAL}:e1`,
    calendar_id: CAL,
    summary: "Retro",
    description: null,
    location: null,
    start: "2026-10-20T09:00:00Z",
    end: "2026-10-20T10:00:00Z",
    all_day: false,
    html_link: null,
    uid: "e1@google.com",
    organizer_self: true,
    edit_block: null,
    can_delete: true,
    can_respond: false,
    can_retime: true,
    ...over,
  });
  const theirs = { organizer_self: false, can_delete: false, can_respond: true };

  // C18c: any Google event of the ordinary kind whose details the account can read, editing on or
  // not and whoever organises it.
  it("duplicable: a Google event PM can read, of the ordinary kind", () => {
    const calendar = (provider: string, access_role: string | null) =>
      ({ id: CAL, provider, access_role }) as unknown as Calendar;
    const rows: [string, CalendarEvent, Calendar | undefined, boolean][] = [
      ["owner", row({}), calendar("google", "owner"), true],
      ["writer", row({}), calendar("google", "writer"), true],
      ["no private access", row({}), calendar("google", "writerWithoutPrivateAccess"), true],
      ["reader", row({}), calendar("google", "reader"), true],
      ["invitation", row(theirs), calendar("google", "owner"), true],
      ["free/busy reader", row({}), calendar("google", "freeBusyReader"), false],
      ["no role", row({}), calendar("google", null), false],
      ["birthday", row({ event_type: "birthday" }), calendar("google", "owner"), false],
      ["Gmail", row({ event_type: "fromGmail" }), calendar("google", "owner"), false],
      ["kind unset", row({ event_type: null }), calendar("google", "owner"), true],
      ["iCal", row({}), calendar("ical", "reader"), false],
      ["Outlook", row({}), calendar("microsoft", "owner"), false],
      ["no calendar", row({}), undefined, false],
    ];
    for (const [name, ev, cal, want] of rows) {
      expect([name, duplicable(ev, cal)]).toEqual([name, want]);
    }
  });

  it("offers a drag exactly where the list says the time may change", () => {
    const on = overview("on");
    const rows: [string, CalendarEvent, CalendarOverview, EventRights][] = [
      ["one-off", row({}), on, { canEdit: true, canDelete: true, canRetime: true, reason: null }],
      [
        "locked",
        row({ locked: true, can_delete: false, can_retime: false }),
        on,
        { canEdit: true, canDelete: false, canRetime: false, reason: reasonText("locked") },
      ],
      [
        "invitation",
        row({ ...theirs, can_retime: false }),
        on,
        {
          canEdit: true,
          canDelete: false,
          canRetime: false,
          reason: notOrganizerText(true, true),
        },
      ],
      [
        "invitation with modify",
        row({ ...theirs, guests_can_modify: true, can_retime: true }),
        on,
        { canEdit: true, canDelete: false, canRetime: true, reason: A11_NOTE },
      ],
      [
        "editing off",
        row({}),
        overview("off"),
        { canEdit: false, canDelete: false, canRetime: false, reason: reasonText("editing_off") },
      ],
      // A Gmail event's organiser is Google's placeholder: editing off says the editing-off line,
      // never "Someone else organises this event" (C20a deviation 5, live row G1).
      [
        "a Gmail event, editing off",
        row({ ...theirs, event_type: "fromGmail", can_delete: true, can_retime: false }),
        overview("off"),
        { canEdit: false, canDelete: false, canRetime: false, reason: reasonText("editing_off") },
      ],
      [
        "focus time",
        row({
          event_type: "focusTime",
          edit_block: "special_type",
          can_delete: false,
          can_retime: false,
        }),
        on,
        { canEdit: false, canDelete: false, canRetime: false, reason: reasonText("special_type") },
      ],
      // C20: a birthday offers Edit and Delete, no drag, and says what can change; a Gmail event
      // says what Google allows on it, whoever organises it; a lock doesn't change either line.
      [
        "a birthday",
        row({ event_type: "birthday", can_retime: false }),
        on,
        {
          canEdit: true,
          canDelete: true,
          canRetime: false,
          reason: reasonText("special_date_fields"),
        },
      ],
      [
        "a locked birthday",
        row({ event_type: "birthday", locked: true, can_retime: false }),
        on,
        {
          canEdit: true,
          canDelete: true,
          canRetime: false,
          reason: reasonText("special_date_fields"),
        },
      ],
      [
        "a Gmail event",
        row({ ...theirs, event_type: "fromGmail", can_delete: true, can_retime: false }),
        on,
        {
          canEdit: true,
          canDelete: true,
          canRetime: false,
          reason: reasonText("from_gmail_fields"),
        },
      ],
      [
        "a locked Gmail event",
        row({ event_type: "fromGmail", locked: true, can_retime: false }),
        on,
        {
          canEdit: true,
          canDelete: true,
          canRetime: false,
          reason: reasonText("from_gmail_fields"),
        },
      ],
      [
        "a subscription",
        row({ calendar_id: "ical:feed" }),
        on,
        { canEdit: false, canDelete: false, canRetime: false, reason: null },
      ],
      [
        "a milestone",
        row({ calendar_id: "pm:milestones" }),
        on,
        { canEdit: false, canDelete: false, canRetime: false, reason: null },
      ],
      [
        "a row the list didn't mark",
        row({ can_delete: undefined, can_retime: undefined }),
        on,
        { canEdit: true, canDelete: false, canRetime: false, reason: null },
      ],
    ];
    for (const [name, ev, view, want] of rows) {
      expect([name, eventRights(ev, view)]).toEqual([name, want]);
    }
  });

  // C20 critiques C2-M4, C1-L12 (LD14): a refused drag of a birthday says its own line by the
  // pointer; every other refused drag says the row's line, and one that's allowed says nothing.
  it("a birthday's refused drag says it can't be dragged, by the pointer", () => {
    const on = overview("on");
    const drag = (ev: CalendarEvent, view = on) => dragReason(ev, eventRights(ev, view));
    expect(SPECIAL_DATE_DRAG).toBe(
      "Birthdays can't be dragged in PM. Open Edit to change what Google allows.",
    );
    const dated = row({ event_type: "birthday", can_retime: false });
    expect(drag(dated)).toBe(SPECIAL_DATE_DRAG);
    // Its details keep the line that says what changes (C2-M4).
    expect(eventRights(dated, on).reason).toBe(reasonText("special_date_fields"));
    // A Gmail event's drag says the Gmail line; a locked event's, the lock's.
    const gmail = row({ event_type: "fromGmail", can_retime: false });
    expect(drag(gmail)).toBe(reasonText("from_gmail_fields"));
    expect(drag(row({ locked: true, can_retime: false }))).toBe(reasonText("locked"));
    // Editing off says so, birthday or not. Were the list ever to allow a birthday's drag (it never
    // knows who owns the date), nothing would refuse it: the row's line stands, unshown.
    expect(drag(dated, overview("off"))).toBe(reasonText("editing_off"));
    expect(drag(row({ event_type: "birthday", can_retime: true }))).toBe(
      reasonText("special_date_fields"),
    );
    expect(drag(row({}))).toBeNull();
  });

  it("ranks the copy you organise first, then any you can change (C14 §9.5)", () => {
    const on = overview("on");
    const rank = (ev: CalendarEvent, view = on) => copyRank(eventRights(ev, view), ev);
    expect(rank(row({}))).toBe(2);
    expect(rank(row(theirs))).toBe(1);
    expect(rank(row({}), overview("off"))).toBe(0);
  });
});

// Asking a view-only calendar's owner for edit access (C20b §10, §14.4 and the Adopted words).
describe("asking for edit access", () => {
  const SHARED = "gcal:me@x.org:team@group.calendar.google.com";
  const BOSS = { owner: "boss@x.org", connected: false };
  const overview = { ask_access: { [SHARED]: BOSS } } as unknown as CalendarOverview;
  const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
    id: `${SHARED}:e1`,
    calendar_id: SHARED,
    summary: "Retro",
    description: null,
    location: null,
    start: "2026-10-20T09:00:00Z",
    end: "2026-10-20T10:00:00Z",
    all_day: false,
    html_link: null,
    uid: null,
    edit_block: "calendar_read_only",
    ...over,
  });

  it("a view-only row asks the owner the overview names", () => {
    expect(askOwnerFor(row({}), overview)).toBe(BOSS);
  });

  it("no ask on a row the list didn't mark view-only, or without an owner", () => {
    for (const block of [null, "not_google", "editing_off", "private_event", "locked"] as const) {
      expect(askOwnerFor(row({ edit_block: block }), overview), String(block)).toBeNull();
    }
    expect(askOwnerFor(row({ calendar_id: "gcal:me@x.org:other" }), overview)).toBeNull();
    expect(askOwnerFor(row({}), null)).toBeNull();
  });

  it("says what came of it, naming the owner, the permission and the calendar", () => {
    const permission = "“Make changes and see event details”";
    expect(askLabel("boss@x.org")).toBe("Ask boss@x.org for edit access");
    expect(askText({ outcome: "opened", owner: "boss@x.org" }, "Team")).toBe(
      `PM asked your system to open an email to boss@x.org in your mail app. If nothing opened, write to boss@x.org and ask for ${permission}.`,
    );
    expect(askText({ outcome: "no_mail_app", owner: "boss@x.org" }, "Team")).toBe(
      `PM couldn't open your mail app. Write to boss@x.org and ask for ${permission} on “Team”.`,
    );
    expect(askText({ outcome: "no_owner" }, "Team")).toBe(
      "PM can't tell who to ask for this calendar now. If its sharing just changed, refresh the calendar.",
    );
    expect(askText({ outcome: "failed", message: "Not now." }, "Team")).toBe(
      "PM couldn't open the email: Not now.",
    );
    expect(askConnectedText("boss@x.org", "me@x.org")).toBe(
      `boss@x.org is connected to PM too: open Google Calendar as boss@x.org and share this calendar with me@x.org, choosing ${permission}.`,
    );
    expect(askConnectedText("boss@x.org", null)).toContain(
      "share this calendar with this account, choosing",
    );
  });

  it("names Google's permission as the email does", () => {
    // The email (calendar_ask.rs) asks for the permission by the name these lines tell the user to
    // ask for, so the two can't drift apart.
    const email = Object.values(
      import.meta.glob<string>("../../../src-tauri/src/calendar_ask.rs", {
        query: "?raw",
        import: "default",
        eager: true,
      }),
    );
    expect(email).toHaveLength(1);
    const named = email[0].match(/const PERMISSION: &str = "([^"]+)";/)?.[1];
    expect(named).toBe("Make changes and see event details");
    expect(askText({ outcome: "opened", owner: "o@x.org" }, "T")).toContain(`“${named}”`);
    expect(askConnectedText("o@x.org", null)).toContain(`“${named}”`);
  });
});

// What a move came to, and its Undo (C16a, §13.1).
describe("move words", () => {
  const saved = (warnings: WriteWarning[] = [], undo = true): WriteOutcome => ({
    outcome: "saved",
    warnings,
    ...(undo ? { undo: { undo_token: "u1", undo_seconds: 8 } } : {}),
  });
  const when = "20-10 · 10:00–11:00";

  it("says where it is now, and that guests get a second email if a move that emailed them is undone", () => {
    expect(moveText(saved(), "Retro", when, null)).toEqual({
      tone: "ok",
      text: "“Retro” is now 20-10 · 10:00–11:00.",
    });
    for (const sent of ["all", "external_only"] as const) {
      expect(moveText(saved(), "Retro", when, sent).text).toBe(
        "“Retro” is now 20-10 · 10:00–11:00. Guests will get a second email if you undo.",
      );
      expect(undoMoveLabel("Retro", sent)).toBe("Undo moving “Retro” (guests get a second email)");
    }
    expect(moveText(saved(), "Retro", when, "none").text).not.toContain("second email");
    expect(undoMoveLabel("Retro", "none")).toBe("Undo moving “Retro”");
    // No Undo, no words about one.
    expect(moveText(saved([], false), "Retro", when, "all").text).not.toContain("second email");
    // "all day" mid-sentence (C2-L18).
    expect(moveText(saved(), "", "All day · 20-10", null).text).toBe(
      "“(no title)” is now all day · 20-10.",
    );
    expect(moveText(saved(["mirror_refresh_pending"], false), "Retro", when, null)).toEqual({
      tone: "ok",
      text: "Moved “Retro” in Google. PM's calendar will show it after the next refresh.",
    });
  });

  it("says why a move didn't happen, and promises only what's true", () => {
    const words = (o: WriteOutcome) => moveText(o, "Retro", when, null).text;
    expect(words({ outcome: "conflict", fields: ["start"] })).toBe(
      "“Retro” changed in Google since PM last showed it, so it wasn't moved. PM now shows Google's version.",
    );
    expect(words({ outcome: "read_only", reason: "locked" })).toBe(
      `“Retro” wasn't moved. ${reasonText("locked")}`,
    );
    expect(words({ outcome: "failed", message: "Google didn't accept the change." })).toBe(
      "“Retro” wasn't moved. Google didn't accept the change.",
    );
    expect(words({ outcome: "unconfirmed" })).toBe(
      "PM lost touch with Google and can't tell whether “Retro” moved. It will show Google's version after the next refresh.",
    );
    expect(words({ outcome: "gone" })).toBe("“Retro” isn't in Google any more.");
    // Every outcome has words, none of them jargon.
    const all: WriteOutcome[] = [
      saved(),
      { outcome: "no_change" },
      { outcome: "needs_choice", scope: true },
      { outcome: "ignored_by_google", split: false },
      { outcome: "partial_split", token: "s", offers: ["check"], from: "2026-10-20" },
      { outcome: "split_removed" },
      { outcome: "conflict", fields: [] },
      { outcome: "gone" },
      { outcome: "read_only", reason: "not_organizer" },
      { outcome: "busy" },
      { outcome: "reauth" },
      { outcome: "unconfirmed" },
      { outcome: "ambiguous", retry_token: "r" },
      { outcome: "failed", message: "Google had a problem with this." },
    ];
    for (const o of all) {
      for (const text of [words(o), moveBackText(o, "Retro").text]) {
        expect(text).toMatch(/\.$/);
        expect(text).not.toMatch(/series|instance|exception|master|pointer|drag handle/i);
      }
    }
  });

  it("says what putting a move back came to", () => {
    expect(moveBackText(saved([], false), "Retro")).toEqual({
      tone: "ok",
      text: "Moved “Retro” back.",
    });
    expect(moveBackText({ outcome: "conflict", fields: ["start"] }, "Retro")).toEqual({
      tone: "warn",
      text: "“Retro” changed in Google since, so it wasn't moved back.",
    });
    expect(
      moveBackText({ outcome: "failed", message: "That can't be undone any more." }, "Retro").text,
    ).toBe("Couldn't move “Retro” back. That can't be undone any more.");
    expect(emailsGuests("all") && emailsGuests("external_only")).toBe(true);
    expect(emailsGuests("none") || emailsGuests(null)).toBe(false);
  });
});
