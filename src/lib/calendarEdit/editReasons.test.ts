// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The switches are exhaustive by type; these pin that every value the backend sends has words, and
// the few lines whose wording carries a promise ("nothing was overwritten").

import { describe, expect, it } from "vitest";

import type { ReadOnlyReason, WriteOutcome } from "../types";
import {
  conflictFields,
  deleteText,
  listFields,
  loadText,
  outcomeText,
  problemText,
  reasonText,
} from "./editReasons";

const REASONS: ReadOnlyReason[] = [
  "not_google",
  "editing_off",
  "calendar_read_only",
  "private_event",
  "not_organizer",
  "recurring",
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
    expect(partial(["check"]).text).toMatch(/^PM lost touch .*only if it shows twice/);
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
      text: "Saved from that day on. Some occurrences you changed on their own may still be on the earlier part, or hidden by Google; check them in Google Calendar.",
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
      text: "Deleted “Standup” from that day on. Some occurrences changed on their own may still show; check them in Google Calendar.",
    });
    // A plan that changed since the choice, or a change Google ignored, deleted nothing.
    expect(deleteText({ outcome: "needs_choice" }, "Standup").text).toMatch(
      /wasn't deleted\. .*Choose again\.$/,
    );
    expect(deleteText({ outcome: "ignored_by_google", split: false }, "Standup").text).toMatch(
      /^“Standup” wasn't deleted\./,
    );
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
