// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The switches are exhaustive by type; these pin that every value the backend sends has words, and
// the few lines whose wording carries a promise ("nothing was overwritten").

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ReadOnlyReason, SplitAction, WriteOutcome, WriteWarning } from "../types";
import {
  CALLOUT_TONE,
  conflictFields,
  deleteText,
  earlierChangeText,
  leftBehindText,
  listFields,
  loadText,
  outcomeText,
  problemText,
  reasonText,
  savedNotice,
  settleText,
  SPLIT_ORDER,
  splitActionLabel,
  writeWarningText,
} from "./editReasons";

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
  "has_guests",
  "special_type",
  "locked",
  "html_description",
];

describe("edit reasons", () => {
  it("has a distinct sentence for every reason the backend sends", () => {
    const lines = REASONS.map(reasonText);
    for (const line of lines) expect(line).toMatch(/^[A-Z].*\.$/);
    expect(new Set(lines).size).toBe(REASONS.length);
  });

  it("names conflicting fields in the editor's words", () => {
    expect(conflictFields(["start", "end", "summary", "transparency", "attendees"])).toEqual([
      "time",
      "summary",
      "show_as",
    ]);
    expect(listFields(["summary"])).toBe("title");
    expect(listFields(["summary", "time", "location"])).toBe("title, time and location");
  });

  it("explains every outcome, promising on a conflict only that nothing was saved over it", () => {
    const outcomes: WriteOutcome[] = [
      { outcome: "saved", warnings: [] },
      { outcome: "saved", warnings: ["mirror_refresh_pending"] },
      { outcome: "saved", warnings: ["left_on_old_series"], left_behind: ["2026-11-02"] },
      { outcome: "no_change" },
      { outcome: "needs_choice" },
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
    expect(deleteText({ outcome: "needs_choice" }, "Standup")).toEqual({
      tone: "warn",
      text: "“Standup” wasn't deleted. PM needs to ask which occurrences first. Delete it again to choose.",
    });
    expect(deleteText({ outcome: "ignored_by_google", split: false }, "Standup").text).toMatch(
      /^“Standup” wasn't deleted\./,
    );
  });

  it("ambiguous has words", () => {
    // A new event whose answer never arrived: it may be there, and Check again won't duplicate it.
    // Never "wasn't created", and never the token in the words.
    const said = outcomeText({ outcome: "ambiguous", retry_token: "ref-1" });
    expect(said).toEqual({
      tone: "warn",
      text: "PM lost touch with Google while creating this event, so it may already be in your calendar. Check again: PM looks for it first and won't make a second copy.",
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
  });
});

const saved = (warnings: WriteWarning[], left_behind?: string[]) =>
  ({ outcome: "saved", warnings, left_behind }) as const;

describe("a write's warnings", () => {
  it("words every warning, for a save and for a delete", () => {
    const all: WriteWarning[] = ["mirror_refresh_pending", "left_on_old_series", "outside_mirror"];
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
      { outcome: "needs_choice" },
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
});
