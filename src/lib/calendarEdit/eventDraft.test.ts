// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every zone is named, so no case depends on the zone the tests run in.

import { describe, expect, it } from "vitest";

import type { DayCode, EventForEdit, FieldPermissions, RepeatRule, TimeDraft } from "../types";
import {
  NEW_EVENT_BASE,
  allDayOf,
  canEdit,
  changedFields,
  checkDraft,
  endAfterStart,
  endDateMin,
  endTimeChoices,
  endsAfterStart,
  firstEndOnItsDate,
  followStart,
  guestNotice,
  halfInstant,
  newEventSeed,
  rebaseDraft,
  repeatSeed,
  sameField,
  seedDraft,
  switchNewKind,
  toChanges,
  toNewEvent,
  type EditorFields,
  type GuestDraft,
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
  guests: [],
  guests_can_modify: false,
  guests_can_invite_others: true,
  guests_can_see_other_guests: true,
  guests_omitted: false,
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
      guests: false,
      delete: false,
      reasons: ["locked"],
    };
    expect(canEdit(perms, "summary")).toBe(false);
    expect(canEdit(perms, "show_as")).toBe(true);
    // How it repeats goes with the time: Google locks both together.
    expect(canEdit(perms, "recurrence")).toBe(false);
    expect(canEdit({ ...perms, time: true }, "recurrence")).toBe(true);
  });
});

describe("how an existing event repeats (C11)", () => {
  const MONDAYS: RepeatRule = {
    pattern: { kind: "weekly", days: ["mon"] },
    interval: 1,
    ends: { kind: "never" },
  };
  const series = (rule: RepeatRule | null) => ({
    first: false,
    repeats: rule ? "Weekly on Monday" : "Repeats (custom rule)",
    rule,
    starts: "2026-10-05",
    zone: "Europe/London",
  });

  it("recurrence counts as a change only when it differs", () => {
    const weekly = seedDraft(event, repeatSeed(series(MONDAYS), true));
    // The same rule, its days listed again, is no change.
    const same = {
      ...weekly,
      recurrence: {
        kind: "rule" as const,
        rule: {
          ...MONDAYS,
          pattern: { kind: "weekly" as const, days: ["mon", "mon"] as DayCode[] },
        },
      },
    };
    expect(changedFields(weekly, same)).toEqual([]);
    expect(toChanges(weekly, same)).toEqual({});
    const daily: RepeatRule = { ...MONDAYS, pattern: { kind: "daily" } };
    const other = { ...weekly, recurrence: { kind: "rule" as const, rule: daily } };
    expect(changedFields(weekly, other)).toEqual(["recurrence"]);
    expect(toChanges(weekly, other)).toEqual({ recurrence: { kind: "rule", rule: daily } });
    // What is sent shares nothing with the draft: the choices bind the very object sent.
    const sent = toChanges(weekly, other).recurrence;
    expect(sent?.kind === "rule" && sent.rule).not.toBe(daily);
    const stop = { ...weekly, recurrence: { kind: "once" as const } };
    expect(toChanges(weekly, stop)).toEqual({ recurrence: { kind: "once" } });
    // A one-off given a rule.
    const oneOff = base();
    expect(toChanges(oneOff, { ...oneOff, recurrence: { kind: "rule", rule: MONDAYS } })).toEqual({
      recurrence: { kind: "rule", rule: MONDAYS },
    });
  });

  it("repeatSeed never gives once for a repeating event", () => {
    expect(repeatSeed(series(MONDAYS), true)).toEqual({ kind: "rule", rule: MONDAYS });
    // Even when the row didn't say it repeats: its series did.
    expect(repeatSeed(series(MONDAYS), false)).toEqual({ kind: "rule", rule: MONDAYS });
    expect(repeatSeed(series(null), true)).toEqual({
      kind: "kept",
      words: "Repeats (custom rule)",
    });
    expect(repeatSeed(null, true)).toEqual({ kind: "kept", words: "Repeats" });
    expect(repeatSeed(null, false)).toEqual({ kind: "once" });
    // Its own copy: the draft can't change the series it came from.
    const seeded = repeatSeed(series(MONDAYS), true);
    expect(seeded.kind === "rule" && seeded.rule).not.toBe(MONDAYS);
  });

  it("a rebase over a changed rule keeps Google's and says repeat", () => {
    const old = seedDraft(event, repeatSeed(series(MONDAYS), true));
    const daily: RepeatRule = { ...MONDAYS, pattern: { kind: "daily" } };
    const tuesdays: RepeatRule = { ...MONDAYS, pattern: { kind: "weekly", days: ["tue"] } };
    const theirs = seedDraft(event, { kind: "rule", rule: daily });
    const mine = { ...old, recurrence: { kind: "rule" as const, rule: tuesdays } };
    const r = rebaseDraft(old, theirs, mine);
    expect(r.draft.recurrence).toEqual({ kind: "rule", rule: daily });
    expect(r.overwritten).toEqual(["recurrence"]);
    // Only Google changed it: taken, said nowhere.
    const untouched = rebaseDraft(old, theirs, { ...old, summary: "Gym" });
    expect(untouched.draft.recurrence).toEqual({ kind: "rule", rule: daily });
    expect(untouched.draft.summary).toBe("Gym");
    expect(untouched.overwritten).toEqual([]);
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

// --- guests (#884, C12) ---

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
const ME = guest("me@x.org", { name: "Me", organizer: true, self: true });
const A = guest("A@x.org", { name: "Ann", response: "needsAction" });
const withGuests = (guests: GuestDraft[], over: Partial<EventForEdit> = {}): EditorFields =>
  seedDraft({ ...event, guests, ...over });

describe("guests in the draft", () => {
  it("seeds the list and Google's permissions from the copy opened, its own copy", () => {
    const opened: EventForEdit = {
      ...event,
      guests: [ME, A],
      guests_can_modify: true,
      guests_can_invite_others: false,
      guests_can_see_other_guests: true,
    };
    const b = seedDraft(opened);
    expect(b.guests).toEqual([ME, A]);
    expect(b.guests[1]).not.toBe(opened.guests[1]);
    expect(b.guest_perms).toEqual({ modify: true, invite_others: false, see_others: true });
    // Guests come after the time and how it repeats, as Google lists them.
    expect(changedFields(b, { ...b, summary: "x", guests: [ME], location: "y" })).toEqual([
      "summary",
      "guests",
      "location",
    ]);
  });

  it("compares by who is on it and who is optional, never replies, names, case or order", () => {
    const b = withGuests([ME, A]);
    const replied = {
      ...b,
      guests: [{ ...A, email: "a@X.ORG", name: "A. N.", response: "accepted" }, ME],
    };
    expect(sameField("guests", b, replied)).toBe(true);
    expect(changedFields(b, replied)).toEqual([]);
    expect(toChanges(b, replied)).toEqual({});
    expect(sameField("guests", b, { ...b, guests: [ME, { ...A, optional: true }] })).toBe(false);
    expect(sameField("guests", b, { ...b, guests: [ME] })).toBe(false);
  });

  it("sends changes to Google's list, sorted by address, never the list", () => {
    const c = guest("Cat@x.org", { response: null });
    const z = guest("zed@x.org");
    const b = withGuests([ME, z, A]);
    const draft = { ...b, guests: [ME, { ...A, optional: true }, guest("bob@x.org"), c] };
    expect(toChanges(b, draft)).toEqual({
      guests: [
        { email: "A@x.org", to: "optional" },
        { email: "bob@x.org", to: "required" },
        { email: "Cat@x.org", to: "required" },
        { email: "zed@x.org", to: "absent" },
      ],
    });
    // Made optional from the start.
    expect(toChanges(b, { ...b, guests: [ME, z, A, { ...c, optional: true }] }).guests).toEqual([
      { email: "Cat@x.org", to: "optional" },
    ]);
    // Required again is a change of its own.
    const opt = withGuests([ME, { ...A, optional: true }]);
    expect(toChanges(opt, { ...opt, guests: [ME, A] }).guests).toEqual([
      { email: "A@x.org", to: "required" },
    ]);
  });

  it("never sends a change to the organiser's or the user's own entry", () => {
    const organiser = guest("o@x.org", { organizer: true });
    const me = guest("me@x.org", { self: true });
    const b = withGuests([organiser, me, A]);
    // Taken off or made optional: nothing goes for either.
    expect(toChanges(b, { ...b, guests: [A] })).toEqual({});
    expect(
      toChanges(b, {
        ...b,
        guests: [{ ...organiser, optional: true }, { ...me, optional: true }, A],
      }),
    ).toEqual({});
    // Nor for an entry marked as theirs that the opened copy didn't have (a hand-over could).
    const theirs = [guest("o2@x.org", { organizer: true }), guest("me2@x.org", { self: true })];
    expect(toChanges(b, { ...b, guests: [...b.guests, ...theirs] })).toEqual({});
  });

  it("sends only the permissions that changed", () => {
    const b = base();
    expect(
      toChanges(b, { ...b, guest_perms: { modify: true, invite_others: true, see_others: true } }),
    ).toEqual({ guests_can_modify: true });
    expect(
      toChanges(b, {
        ...b,
        guest_perms: { modify: false, invite_others: false, see_others: false },
      }),
    ).toEqual({ guests_can_invite_others: false, guests_can_see_other_guests: false });
    expect(changedFields(b, { ...b, guest_perms: { ...b.guest_perms } })).toEqual([]);
  });

  it("refuses an added guest whose address isn't one, never Google's own", () => {
    const b = withGuests([ME, guest("not an address")]);
    const draft = { ...b, guests: [...b.guests, guest("bob at x"), guest("b@x.org")] };
    expect(checkDraft(b, draft).problems).toEqual([{ kind: "bad_guest", email: "bob at x" }]);
    // Even with the time untouched, which the rest of the check skips.
    expect(checkDraft(b, b).problems).toEqual([]);
  });

  it("goes with its own right for both the list and its permissions", () => {
    const perms: FieldPermissions = {
      summary: true,
      time: true,
      location: true,
      description: true,
      show_as: true,
      visibility: true,
      guests: false,
      delete: true,
      reasons: [],
    };
    expect(canEdit(perms, "guests")).toBe(false);
    expect(canEdit(perms, "guest_perms")).toBe(false);
    expect(canEdit({ ...perms, guests: true }, "guests")).toBe(true);
    expect(canEdit({ ...perms, guests: true }, "guest_perms")).toBe(true);
    expect(canEdit(perms, "summary")).toBe(true);
  });

  it("starts a new event with nobody and Google's permissions", () => {
    const seed = newEventSeed(
      "2026-10-12",
      new Date("2026-10-12T08:00:00Z"),
      "Europe/London",
      HOUR,
    );
    expect(seed.guests).toEqual([]);
    expect(seed.guest_perms).toEqual({ modify: false, invite_others: true, see_others: true });
    expect(NEW_EVENT_BASE.guest_perms).toEqual(seed.guest_perms);
    expect(changedFields(NEW_EVENT_BASE, seed)).not.toContain("guest_perms");
    // A create doesn't send guests yet (C12d).
    expect(Object.keys(toNewEvent(seed))).not.toContain("guests");
  });

  it("asks before a round trip only when the save would email someone", () => {
    const b = withGuests([ME, A]);
    expect(guestNotice(b, { ...b, show_as: "free" }, false)).toBeNull();
    expect(guestNotice(b, { ...b, summary: "Retro" }, false)).toMatchObject({ updated: 1 });
  });
});

describe("carrying guests onto Google's newer copy", () => {
  const b = guest("b@x.org", { name: "Bea", response: null });

  it("a rebase after someone else's reply keeps the user's guests and shows the reply", () => {
    const oldBase = withGuests([ME, A]);
    const newBase = withGuests([ME, { ...A, response: "accepted" }]);
    const draft = { ...oldBase, guests: [...oldBase.guests, b] };
    const r = rebaseDraft(oldBase, newBase, draft);
    expect(r.draft.guests).toEqual([ME, { ...A, response: "accepted" }, b]);
    expect(r.overwritten).toEqual([]);
    expect(toChanges(r.base, r.draft)).toEqual({ guests: [{ email: "b@x.org", to: "required" }] });
  });

  it("Google took a guest off that the user made optional: dropped, and said", () => {
    const oldBase = withGuests([ME, A]);
    const newBase = withGuests([ME]);
    const draft = { ...oldBase, guests: [ME, { ...A, optional: true }] };
    const r = rebaseDraft(oldBase, newBase, draft);
    expect(r.draft.guests).toEqual([ME]);
    expect(r.overwritten).toEqual(["guests"]);
  });

  it("Google added one and the user another: both, nothing lost", () => {
    const c = guest("c@x.org", { name: "Cy" });
    const oldBase = withGuests([ME, A]);
    const newBase = withGuests([ME, A, c]);
    const draft = { ...oldBase, guests: [ME, A, b] };
    const r = rebaseDraft(oldBase, newBase, draft);
    expect(r.draft.guests).toEqual([ME, A, c, b]);
    expect(r.overwritten).toEqual([]);
  });

  it("makes the user's other changes on Google's list, and a change Google made too is no loss", () => {
    const c = guest("c@x.org");
    const oldBase = withGuests([ME, A, c]);
    // Google: A made optional, c taken off. The user: A made optional too, c made optional, b added.
    const newBase = withGuests([ME, { ...A, optional: true }]);
    const draft = {
      ...oldBase,
      guests: [ME, { ...A, optional: true }, { ...c, optional: true }, b],
    };
    const r = rebaseDraft(oldBase, newBase, draft);
    expect(r.draft.guests).toEqual([ME, { ...A, optional: true }, b]);
    expect(r.overwritten).toEqual(["guests"]);
    // The same change on both sides: Google's list, nothing said.
    const same = rebaseDraft(oldBase, newBase, {
      ...oldBase,
      guests: [ME, { ...A, optional: true }],
    });
    expect(same.draft.guests).toEqual(newBase.guests);
    expect(same.overwritten).toEqual([]);
  });

  it("merges the permissions one by one", () => {
    const oldBase = base();
    const newBase = {
      ...oldBase,
      guest_perms: { modify: false, invite_others: false, see_others: true },
    };
    // Only the user changed see_others; only Google invite_others: each keeps its own.
    const draft = {
      ...oldBase,
      guest_perms: { modify: false, invite_others: true, see_others: false },
    };
    const r = rebaseDraft(oldBase, newBase, draft);
    expect(r.draft.guest_perms).toEqual({ modify: false, invite_others: false, see_others: false });
    expect(r.overwritten).toEqual([]);
    expect(toChanges(r.base, r.draft)).toEqual({ guests_can_see_other_guests: false });
    // A box has two states, so both changing one is the same change: nothing to say.
    const both = rebaseDraft(oldBase, newBase, {
      ...oldBase,
      guest_perms: { modify: false, invite_others: false, see_others: true },
    });
    expect(both.overwritten).toEqual([]);
    expect(toChanges(both.base, both.draft)).toEqual({});
    // The rebase never shares the new copy's objects with the draft it makes.
    expect(r.draft.guest_perms).not.toBe(newBase.guest_perms);
    expect(r.draft.guests).not.toBe(newBase.guests);
  });
});

// --- creating an event (#884, C10) ---

const HOUR = 60 * 60_000;
const HALF = 30 * 60_000;

describe("a new event's starting fields", () => {
  it("newEventSeed: next half hour on today, 09:00 elsewhere, the account's length", () => {
    // 14:10 in London (BST): 14:30, for the account's 30 minutes.
    const now = new Date("2026-10-12T13:10:00Z");
    const seed = newEventSeed("2026-10-12", now, "Europe/London", HALF);
    expect(seed.time).toEqual(timed("2026-10-12", "14:30", "2026-10-12", "15:00"));
    expect(seed).toMatchObject({
      summary: "",
      location: "",
      description: "",
      show_as: "busy",
      visibility: "default",
    });
    // Strictly after: on the half hour itself, the next one.
    expect(
      newEventSeed("2026-10-12", new Date("2026-10-12T13:30:00Z"), "Europe/London", HOUR).time,
    ).toEqual(timed("2026-10-12", "15:00", "2026-10-12", "16:00"));
    // Another day: 09:00.
    expect(newEventSeed("2026-10-14", now, "Europe/London", HOUR).time).toEqual(
      timed("2026-10-14", "09:00", "2026-10-14", "10:00"),
    );
    // "Today" is the zone's: 00:30 in Tokyo is still the day before in London.
    expect(
      newEventSeed("2026-10-13", new Date("2026-10-12T15:30:00Z"), "Asia/Tokyo", HOUR).time,
    ).toEqual(timed("2026-10-13", "01:00", "2026-10-13", "02:00", "Asia/Tokyo"));
  });

  it("newEventSeed past 23:30 starts tomorrow at 00:00", () => {
    const now = new Date("2026-10-12T22:40:00Z"); // 23:40 BST
    expect(newEventSeed("2026-10-12", now, "Europe/London", HOUR).time).toEqual(
      timed("2026-10-13", "00:00", "2026-10-13", "01:00"),
    );
  });

  it("steps past a start the clocks skip", () => {
    // London's clocks go from 01:00 to 02:00 on 28-03-2027: at 00:35 the next half hour, 01:00,
    // never happens, nor does 01:30.
    const now = new Date("2027-03-28T00:35:00Z");
    const seed = newEventSeed("2027-03-28", now, "Europe/London", HOUR);
    expect(seed.time).toEqual(timed("2027-03-28", "02:00", "2027-03-28", "03:00"));
    expect(checkDraft(NEW_EVENT_BASE, seed).problems).toEqual([]);
  });

  it("starts all day, free, when there's no zone to show times in", () => {
    expect(newEventSeed("2026-10-14", new Date(), null, HOUR)).toMatchObject({
      time: { kind: "all_day", first_day: "2026-10-14", last_day: "2026-10-14" },
      show_as: "free",
    });
  });

  it("checkDraft against NEW_EVENT_BASE judges a seeded gap", () => {
    const fields = (time: TimeDraft): EditorFields => ({ ...NEW_EVENT_BASE, time });
    // Every half is judged: there's nothing Google holds to keep.
    expect(
      checkDraft(NEW_EVENT_BASE, fields(timed("2027-03-28", "01:30", "2027-03-28", "02:30")))
        .problems,
    ).toEqual([{ kind: "gap", half: "start", zone: "Europe/London", time: "01:30" }]);
    expect(
      checkDraft(NEW_EVENT_BASE, fields(timed("2026-10-12", "10:00", "2026-10-12", "10:00")))
        .problems,
    ).toEqual([{ kind: "end_before_start" }]);
    expect(
      checkDraft(
        NEW_EVENT_BASE,
        fields({ kind: "all_day", first_day: "2026-10-14", last_day: "2026-10-13" }),
      ).problems,
    ).toEqual([{ kind: "end_before_start" }]);
    // The night the clocks go back: the first 01:30, said.
    expect(
      checkDraft(NEW_EVENT_BASE, fields(timed("2026-10-25", "01:30", "2026-10-25", "03:00")))
        .ambiguous,
    ).toEqual(["start"]);
    expect(
      checkDraft(NEW_EVENT_BASE, fields(timed("2026-10-12", "10:00", "2026-10-12", "11:00"))),
    ).toEqual({ problems: [], ambiguous: [] });
  });

  it("toNewEvent sends every field", () => {
    const draft: EditorFields = {
      summary: "Dentist",
      location: "",
      description: "Bring the form",
      time: timed("2026-10-12", "09:00", "2026-10-12", "10:00"),
      recurrence: { kind: "once" },
      guests: [],
      guest_perms: { modify: false, invite_others: true, see_others: true },
      show_as: "free",
      visibility: "private",
    };
    const sent = toNewEvent(draft);
    expect(sent).toEqual({
      summary: "Dentist",
      time: timed("2026-10-12", "09:00", "2026-10-12", "10:00"),
      location: "",
      description: "Bring the form",
      show_as: "free",
      visibility: "private",
      recurrence: null,
    });
    // A copy: a later edit to the draft can't change what was sent.
    expect(sent.time).not.toBe(draft.time);
  });

  // --- repeating (C11) ---

  const weeklyOn = (...days: DayCode[]): RepeatRule => ({
    pattern: { kind: "weekly", days },
    interval: 1,
    ends: { kind: "never" },
  });

  it("toNewEvent sends the chosen repeat as a copy, and Does not repeat as null", () => {
    const days: DayCode[] = ["mon", "wed"];
    const rule: RepeatRule = { ...weeklyOn(), pattern: { kind: "weekly", days } };
    const draft: EditorFields = {
      ...NEW_EVENT_BASE,
      time: timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
      recurrence: { kind: "rule", rule },
    };
    const sent = toNewEvent(draft);
    expect(sent.recurrence).toEqual(rule);
    // What was sent is kept for a retry: nothing the draft holds can change it.
    days.push("fri");
    expect(sent.recurrence).toEqual(weeklyOn("mon", "wed"));
    expect(toNewEvent({ ...draft, recurrence: { kind: "once" } }).recurrence).toBeNull();
    // A new event's seed doesn't repeat.
    expect(newEventSeed("2026-10-13", new Date(), "Europe/London", HOUR).recurrence).toEqual({
      kind: "once",
    });
  });

  it("a changed repeat is a change, and a moved start takes the rule's own day with it", () => {
    const tuesday: EditorFields = {
      ...NEW_EVENT_BASE,
      time: timed("2026-10-13", "09:00", "2026-10-13", "10:00"),
      recurrence: { kind: "rule", rule: weeklyOn("tue") },
    };
    expect(changedFields(NEW_EVENT_BASE, tuesday)).toContain("recurrence");
    // The same rule with its days in another order is no change.
    const both = {
      ...tuesday,
      recurrence: { kind: "rule" as const, rule: weeklyOn("mon", "tue") },
    };
    expect(
      changedFields(both, { ...both, recurrence: { kind: "rule", rule: weeklyOn("tue", "mon") } }),
    ).toEqual([]);
    // Weekly on Tuesday, started on a Tuesday, moved to Wednesday: weekly on Wednesday.
    const moved = followStart(tuesday, {
      ...tuesday,
      time: timed("2026-10-14", "09:00", "2026-10-14", "10:00"),
    });
    expect(moved.recurrence).toEqual({ kind: "rule", rule: weeklyOn("wed") });
    // Two days chosen don't come from the start: they stay.
    const kept = { ...both, time: timed("2026-10-14", "09:00", "2026-10-14", "10:00") };
    expect(followStart(both, kept)).toBe(kept);
  });

  it("switchNewKind keeps the day and length and follows Google's free/busy until touched", () => {
    const seed = newEventSeed(
      "2026-10-12",
      new Date("2026-10-12T13:10:00Z"),
      "Europe/London",
      HALF,
    );
    const allDay = switchNewKind(seed, seed, "Europe/London", HALF, false);
    expect(allDay.time).toEqual({
      kind: "all_day",
      first_day: "2026-10-12",
      last_day: "2026-10-12",
    });
    expect(allDay.show_as).toBe("free");
    const back = switchNewKind(
      { ...allDay, time: { kind: "all_day", first_day: "2026-10-20", last_day: "2026-10-22" } },
      seed,
      "Europe/London",
      HALF,
      false,
    );
    expect(back.time).toEqual(timed("2026-10-20", "14:30", "2026-10-20", "15:00"));
    expect(back.show_as).toBe("busy");
    // Picked by the user: kept, both ways.
    expect(
      switchNewKind({ ...seed, show_as: "busy" }, seed, "Europe/London", HALF, true).show_as,
    ).toBe("busy");
    expect(
      switchNewKind({ ...allDay, show_as: "free" }, seed, "Europe/London", HALF, true).show_as,
    ).toBe("free");
  });

  it("switchNewKind reads an end at midnight as the day before, as editing does", () => {
    const late: EditorFields = {
      ...NEW_EVENT_BASE,
      time: timed("2026-10-12", "23:00", "2026-10-13", "00:00"),
    };
    expect(switchNewKind(late, late, "Europe/London", HOUR, false).time).toEqual({
      kind: "all_day",
      first_day: "2026-10-12",
      last_day: "2026-10-12",
    });
    const twoNights = timed("2026-10-12", "23:00", "2026-10-14", "00:00");
    if (twoNights.kind !== "timed") throw new Error("timed");
    expect(allDayOf(twoNights)).toEqual({
      kind: "all_day",
      first_day: "2026-10-12",
      last_day: "2026-10-13",
    });
  });
});
