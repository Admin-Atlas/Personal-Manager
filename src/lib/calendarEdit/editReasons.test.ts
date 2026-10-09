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
      { outcome: "no_change" },
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
