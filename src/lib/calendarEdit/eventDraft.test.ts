// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every zone is named, so no case depends on the zone the tests run in.

import { describe, expect, it } from "vitest";

import type { EventForEdit, FieldPermissions, TimeDraft } from "../types";
import {
  canEdit,
  changedFields,
  checkDraft,
  rebaseDraft,
  seedDraft,
  toChanges,
  type EditorFields,
} from "./eventDraft";

const timed = (
  start_date: string,
  start_time: string,
  end_date: string,
  end_time: string,
  zone = "Europe/London",
): TimeDraft => ({
  kind: "timed",
  start_date,
  start_time,
  start_zone: zone,
  end_date,
  end_time,
  end_zone: zone,
});

const event: EventForEdit = {
  summary: "Dentist",
  location: "High St",
  description: "Bring the form\r\nand the card",
  description_html: false,
  time: timed("2026-10-12", "09:00", "2026-10-12", "10:00"),
  start_at: "2026-10-12T08:00:00Z",
  end_at: "2026-10-12T09:00:00Z",
  show_as: "busy",
  visibility: "default",
  attachments: [],
  html_link: null,
};

const base = (): EditorFields => seedDraft(event);

describe("the event draft", () => {
  it("starts as Google's copy, with nothing to send", () => {
    const b = base();
    expect(b.summary).toBe("Dentist");
    expect(changedFields(b, seedDraft(event))).toEqual([]);
    expect(toChanges(b, seedDraft(event))).toEqual({});
  });

  it("sends only what changed", () => {
    const draft = { ...base(), summary: "Dentist (moved)", show_as: "free" as const };
    expect(changedFields(base(), draft)).toEqual(["summary", "show_as"]);
    expect(toChanges(base(), draft)).toEqual({ summary: "Dentist (moved)", show_as: "free" });
    // Cleared text is a change, sent as "".
    expect(toChanges(base(), { ...base(), location: "" })).toEqual({ location: "" });
  });

  it("doesn't count a textarea's line endings as an edit", () => {
    // The textarea hands back \n for Google's \r\n.
    const draft = { ...base(), description: "Bring the form\nand the card" };
    expect(changedFields(base(), draft)).toEqual([]);
  });

  it("sends a time change whole, and a switch to all day", () => {
    const later = { ...base(), time: timed("2026-10-12", "09:30", "2026-10-12", "10:30") };
    expect(toChanges(base(), later)).toEqual({ time: later.time });
    const allDay = {
      ...base(),
      time: { kind: "all_day" as const, first_day: "2026-10-12", last_day: "2026-10-12" },
    };
    expect(changedFields(base(), allDay)).toEqual(["time"]);
  });

  it("maps fields to their permissions", () => {
    const perms: FieldPermissions = {
      summary: false,
      time: false,
      location: false,
      description: false,
      show_as: true,
      visibility: true,
      delete: false,
      reasons: ["locked"],
    };
    expect(canEdit(perms, "summary")).toBe(false);
    expect(canEdit(perms, "show_as")).toBe(true);
  });
});

describe("checking a draft before it is sent", () => {
  it("passes an ordinary event", () => {
    expect(checkDraft(base(), base())).toEqual({ problems: [], ambiguous: [] });
  });

  it("refuses an end before the start", () => {
    const draft = { ...base(), time: timed("2026-10-12", "10:00", "2026-10-12", "09:00") };
    expect(checkDraft(base(), draft).problems).toEqual([{ kind: "end_before_start" }]);
    const days = {
      ...base(),
      time: { kind: "all_day" as const, first_day: "2026-10-13", last_day: "2026-10-12" },
    };
    expect(checkDraft(base(), days).problems).toEqual([{ kind: "end_before_start" }]);
  });

  it("refuses a time the clocks skip", () => {
    const draft = { ...base(), time: timed("2026-03-29", "01:30", "2026-03-29", "03:00") };
    expect(checkDraft(base(), draft).problems).toEqual([
      { kind: "gap", half: "start", zone: "Europe/London", time: "01:30" },
    ]);
  });

  it("never judges a time the save won't send", () => {
    // A 0-minute event (Google allows them): renaming it must not be refused.
    const zero: EditorFields = {
      ...base(),
      time: timed("2026-10-12", "09:00", "2026-10-12", "09:00"),
    };
    expect(checkDraft(zero, { ...zero, summary: "Call" })).toEqual({
      problems: [],
      ambiguous: [],
    });
    // Nor a zone this webview doesn't know, when the time is left alone.
    const odd = { ...base(), time: timed("2026-10-12", "09:00", "2026-10-12", "10:00", "Nowhere") };
    expect(checkDraft(odd, { ...odd, summary: "Call" }).problems).toEqual([]);
  });

  // 25 Oct 2026, Europe/London: 01:00–01:59 happens twice (BST, then GMT).
  const fold: EditorFields = {
    ...base(),
    time: timed("2026-10-25", "01:45", "2026-10-25", "01:15"), // 01:45 BST to 01:15 GMT
  };
  const foldHeld = { start_at: "2026-10-25T00:45:00Z", end_at: "2026-10-25T01:15:00Z" };

  it("judges a half the user left alone by Google's exact instant", () => {
    // Only the start changes, to 01:40 (its first occurrence, 00:40Z): the held end, 01:15 GMT, is
    // still after it, and only the changed half is noted as repeated.
    const startMoved = {
      ...fold,
      time: timed("2026-10-25", "01:40", "2026-10-25", "01:15"),
    };
    expect(checkDraft(fold, startMoved, foldHeld)).toEqual({ problems: [], ambiguous: ["start"] });
    // Google holds a start at the SECOND 01:30 (01:30Z); moving the end to 01:45 means its first
    // occurrence, 00:45Z, which is before the start. The backend refuses it, so the check does too.
    const late = { ...base(), time: timed("2026-10-25", "01:30", "2026-10-25", "02:30") };
    const lateHeld = { start_at: "2026-10-25T01:30:00Z", end_at: "2026-10-25T02:30:00Z" };
    const endMoved = { ...late, time: timed("2026-10-25", "01:30", "2026-10-25", "01:45") };
    expect(checkDraft(late, endMoved, lateHeld).problems).toEqual([{ kind: "end_before_start" }]);
    // Without Google's instants, either occurrence of the untouched start is allowed.
    expect(checkDraft(late, endMoved).problems).toEqual([]);
  });

  it("takes the first occurrence of halves the user typed", () => {
    // Both typed in: both are BST, and the end comes first.
    expect(checkDraft(base(), fold)).toEqual({
      problems: [{ kind: "end_before_start" }],
      ambiguous: ["start", "end"],
    });
  });

  it("says what it couldn't read", () => {
    const zone = {
      ...base(),
      time: timed("2026-10-12", "09:00", "2026-10-12", "10:00", "Nowhere"),
    };
    expect(checkDraft(base(), zone).problems).toContainEqual({
      kind: "unreadable",
      half: "start",
      what: "zone",
    });
  });
});

describe("carrying a draft onto Google's newer copy", () => {
  it("keeps the user's edits and takes Google's other changes", () => {
    const draft = { ...base(), summary: "Dentist (moved)" };
    const google = { ...base(), location: "Low St" };
    const r = rebaseDraft(base(), google, draft);
    expect(r.draft.summary).toBe("Dentist (moved)");
    expect(r.draft.location).toBe("Low St");
    expect(r.overwritten).toEqual([]);
    expect(r.base).toBe(google);
  });

  it("lets Google win a field both changed, and says so", () => {
    const draft = { ...base(), summary: "Dentist (PM)", location: "Room 2" };
    const google = { ...base(), summary: "Dentist (Google)" };
    const r = rebaseDraft(base(), google, draft);
    expect(r.draft.summary).toBe("Dentist (Google)");
    expect(r.draft.location).toBe("Room 2");
    expect(r.overwritten).toEqual(["summary"]);
  });

  it("doesn't count the same change made on both sides as lost", () => {
    const later = timed("2026-10-12", "11:00", "2026-10-12", "12:00");
    const r = rebaseDraft(base(), { ...base(), time: later }, { ...base(), time: { ...later } });
    expect(r.overwritten).toEqual([]);
    expect(toChanges(r.base, r.draft)).toEqual({});
  });
});
