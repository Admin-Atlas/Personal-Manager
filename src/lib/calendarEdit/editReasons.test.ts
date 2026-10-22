// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The switches are exhaustive by type; these pin that every value the backend sends has words, and
// the few lines whose wording carries a promise ("nothing was overwritten").

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  GuestNotice,
  MilestoneMove,
  ReadOnlyReason,
  SplitAction,
  WriteOutcome,
  WriteWarning,
} from "../types";
import {
  CALLOUT_TONE,
  CREATE_CHANGED,
  CREATE_LOST,
  conflictFields,
  createdText,
  createReasonText,
  createText,
  deleteText,
  endsRetry,
  heldCreateText,
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
  outcomeText,
  partialText,
  problemText,
  reasonText,
  rebasedText,
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
  "became_repeating",
  "special_type",
  "locked",
  "html_description",
  "label_read_only",
  "reminders_unreadable",
];

describe("edit reasons", () => {
  it("has a distinct sentence for every reason the backend sends", () => {
    const lines = REASONS.map(reasonText);
    for (const line of lines) expect(line).toMatch(/^[A-Z].*\.$/);
    expect(new Set(lines).size).toBe(REASONS.length);
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

  it("names conflicting fields in the editor's words", () => {
    // A key the editor has no field for names nothing.
    expect(conflictFields(["start", "end", "summary", "transparency", "conferenceData"])).toEqual([
      "time",
      "summary",
      "show_as",
    ]);
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
    ];
    for (const w of all) {
      for (const kind of ["save", "delete"] as const) {
        expect(writeWarningText(w, kind)).toMatch(/^[A-Z].*\.$/);
      }
    }
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
