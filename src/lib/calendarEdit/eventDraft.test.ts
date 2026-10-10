// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every zone is named, so no case depends on the zone the tests run in.

import { describe, expect, it } from "vitest";

import type { EventForEdit, FieldPermissions, TimeDraft } from "../types";
import {
  canEdit,
  changedFields,
  checkDraft,
  endAfterStart,
  endDateMin,
  endTimeChoices,
  endsAfterStart,
  firstEndOnItsDate,
  halfInstant,
  rebaseDraft,
  seedDraft,
  toChanges,
  type EditorFields,
} from "./eventDraft";
import { QUARTER_HOURS } from "./timeSlots";

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

type Timed = Extract<TimeDraft, { kind: "timed" }>;
const asTimed = (t: TimeDraft): Timed => {
  if (t.kind !== "timed") throw new Error("timed");
  return t;
};

describe("the End time list", () => {
  it("offers only times after the start, with the event's length", () => {
    const t = asTimed(timed("2026-10-12", "09:00", "2026-10-12", "10:00"));
    const choices = endTimeChoices(base(), t, event, "10:00");
    expect(choices[0]).toEqual({ value: "09:15", label: "09:15 (15 mins)", after: true });
    expect(choices.find((c) => c.value === "10:00")?.label).toBe("10:00 (1 hr)");
    expect(choices.find((c) => c.value === "10:30")?.label).toBe("10:30 (1.5 hrs)");
    expect(choices.some((c) => c.value <= "09:00")).toBe(false);
  });

  it("offers the whole day, unlabelled, when the end is on a later day", () => {
    const t = asTimed(timed("2026-10-12", "23:00", "2026-10-13", "01:00"));
    const choices = endTimeChoices(base(), t, event);
    expect(choices).toHaveLength(96);
    expect(choices[0]).toEqual({ value: "00:00", label: "00:00", after: true });
  });

  it("reads the start in the end's zone", () => {
    // 09:00 London is 04:00 New York: an end at 04:00 there is no later than the start.
    const t: Timed = {
      ...asTimed(timed("2026-10-12", "09:00", "2026-10-12", "05:00")),
      end_zone: "America/New_York",
    };
    const choices = endTimeChoices(base(), t, event);
    expect(choices.some((c) => c.value === "04:00")).toBe(false);
    expect(choices[0]).toEqual({ value: "04:15", label: "04:15 (15 mins)", after: true });
  });

  it("keeps the current end on offer, marked, when it isn't after the start", () => {
    const t = asTimed(timed("2026-10-12", "09:00", "2026-10-12", "08:00"));
    const choices = endTimeChoices(base(), t, event);
    expect(choices[0]).toEqual({ value: "08:00", label: "08:00", after: false });
    expect(choices.filter((c) => !c.after)).toHaveLength(1);
  });

  it("uses Google's exact instants across the night the clocks go back", () => {
    // 01:45 BST to 01:15 GMT is a 30-minute event, valid only by Google's held instants.
    const fold = { ...base(), time: timed("2026-10-25", "01:45", "2026-10-25", "01:15") };
    const held = { start_at: "2026-10-25T00:45:00Z", end_at: "2026-10-25T01:15:00Z" };
    const choices = endTimeChoices(fold, asTimed(fold.time), held, "01:15");
    expect(choices.find((c) => c.value === "01:15")).toEqual({
      value: "01:15",
      label: "01:15 (30 mins)",
      after: true,
    });
  });

  it("leaves the list alone when the start is a time the clocks skip", () => {
    const t = asTimed(timed("2026-03-29", "01:30", "2026-03-29", "03:00"));
    const choices = endTimeChoices(base(), t, event);
    expect(choices).toHaveLength(96);
    expect(choices.every((c) => c.label === c.value)).toBe(true);
  });

  // Every end the list calls valid saves, and every end that saves is in the list, so the list never
  // offers what Save refuses or hides what it allows.
  it("agrees with the save check for every quarter hour", () => {
    for (const start of ["00:00", "09:00", "23:45"]) {
      for (const endDate of ["2026-10-11", "2026-10-12", "2026-10-13"]) {
        const t = asTimed(timed("2026-10-12", start, endDate, "12:00"));
        const offered = endTimeChoices(base(), t, event);
        for (const hm of QUARTER_HOURS) {
          const draft = { ...base(), time: { ...t, end_time: hm } };
          const saves = !checkDraft(base(), draft, event).problems.some(
            (p) => p.kind === "end_before_start",
          );
          const choice = offered.find((c) => c.value === hm);
          expect(choice?.after ?? false, `${start} → ${endDate} ${hm}`).toBe(saves);
        }
      }
    }
  });
});

describe("keeping the end after the start", () => {
  it("starts the End date on the start's day, read in the end's zone", () => {
    const t = asTimed(timed("2026-10-12", "09:00", "2026-10-12", "10:00"));
    expect(endDateMin(base(), t, event)).toBe("2026-10-12");
    // 00:30 London on the 12th is still the 11th in New York.
    const early: Timed = {
      ...asTimed(timed("2026-10-12", "00:30", "2026-10-12", "02:00")),
      end_zone: "America/New_York",
    };
    expect(endDateMin(base(), early, event)).toBe("2026-10-11");
  });

  it("puts the end a length after the start, in the end's zone", () => {
    const t = asTimed(timed("2026-10-12", "23:30", "2026-10-12", "09:00"));
    expect(endAfterStart(base(), t, 60 * 60_000, event)).toMatchObject({
      end_date: "2026-10-13",
      end_time: "00:30",
    });
    // A zone this webview doesn't know leaves the draft as it is rather than throwing.
    expect(endAfterStart(base(), { ...t, end_zone: "Nowhere" }, 60_000, event)).toEqual({
      ...t,
      end_zone: "Nowhere",
    });
  });

  // 25-10-2026 in London: 01:00-01:59 happens twice. A wall time names only the first pass, so an
  // end that falls in the second would read back an hour early, maybe before the start.
  it("never writes an end the night the clocks go back reads as earlier", () => {
    for (const [start, minutes] of [
      ["01:45", 30],
      ["01:00", 60],
      ["01:30", 45],
      ["00:45", 90],
    ] as const) {
      const t = asTimed(timed("2026-10-25", start, "2026-10-25", "09:00"));
      const moved = endAfterStart(base(), t, minutes * 60_000, event);
      expect(endsAfterStart(base(), moved, event), `${start} + ${minutes}m`).toBe(true);
      // Never shorter than asked, and at most the repeated hour longer.
      const length =
        halfInstant(base(), moved, "end", event)! - halfInstant(base(), moved, "start", event)!;
      expect(length).toBeGreaterThanOrEqual(minutes * 60_000);
      expect(length).toBeLessThanOrEqual((minutes + 60) * 60_000);
    }
    // 01:45 BST (00:45Z) + 30 min is 01:15Z, the GMT pass: the end moves on to 02:00 GMT.
    const t = asTimed(timed("2026-10-25", "01:45", "2026-10-25", "09:00"));
    expect(endAfterStart(base(), t, 30 * 60_000, event)).toMatchObject({ end_time: "02:00" });
  });

  it("treats a length under a minute as a minute", () => {
    const t = asTimed(timed("2026-10-12", "09:00", "2026-10-12", "08:00"));
    expect(endAfterStart(base(), t, 20_000, event)).toMatchObject({ end_time: "09:01" });
  });

  it("starts the End date the day after a start too late for any end on its own day", () => {
    const late = asTimed(timed("2026-10-12", "23:45", "2026-10-13", "00:45"));
    expect(endDateMin(base(), late, event)).toBe("2026-10-13");
    const lateish = asTimed(timed("2026-10-12", "23:30", "2026-10-13", "00:30"));
    expect(endDateMin(base(), lateish, event)).toBe("2026-10-12");
  });

  it("finds the first end on the end's own date", () => {
    const t = asTimed(timed("2026-10-12", "09:00", "2026-10-12", "08:00"));
    expect(endsAfterStart(base(), t, event)).toBe(false);
    expect(firstEndOnItsDate(base(), t, event)).toMatchObject({ end_time: "09:15" });
    // A start at 23:50 leaves nothing on that date.
    const late = asTimed(timed("2026-10-12", "23:50", "2026-10-12", "08:00"));
    expect(firstEndOnItsDate(base(), late, event)).toBeNull();
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
