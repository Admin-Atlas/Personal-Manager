// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "which occurrences?" dialog's words and rules (#884): every row, every warning, the question a
// move to another day asks, and what is picked on open. The backend plans the rows; these pin what
// the user reads about them.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { ScopeRow, SeriesWarning, SplitAction } from "../types";
import {
  againText,
  conflictOf,
  defaultScope,
  distanceText,
  isRepeating,
  pendingOf,
  pendingText,
  repeatDayQuestion,
  rowsOf,
  seriesWarningText,
  warningLines,
  type ReadyChoices,
  type RowView,
} from "./saveChoices";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-18T12:00:00"));
});
afterEach(() => {
  vi.useRealTimers();
});

const offered = (token: string, warnings: SeriesWarning[] = []): ScopeRow => ({
  row: "offered",
  warnings,
  token,
});
const ready = (
  this_: ScopeRow,
  following: ScopeRow = offered("f"),
  all: ScopeRow = offered("a"),
): ReadyChoices => ({ outcome: "ready", this: this_, following, all });
const VISIBILITY =
  "Who can see an event applies to every occurrence of a repeating event, so it can't change for this one only.";
const pending: ScopeRow = {
  row: "pending",
  token: "p",
  offers: ["remove", "check"],
  from: "2026-11-02",
};
const JARGON = /master|instance|exception|split|series/i;

const dayMove = (
  before: string | null,
  after: string | null,
  shift_days: number,
  to = "2026-10-21",
): Extract<SeriesWarning, { warning: "repeat_day_changes" }> => ({
  warning: "repeat_day_changes",
  from: "2026-10-20",
  to,
  before,
  after,
  shift_days,
});

describe("which rows repeat", () => {
  it("asks first for a row with a series id or marked repeating", () => {
    expect(isRepeating({ series_id: "abc" })).toBe(true);
    expect(isRepeating({ recurring: true })).toBe(true);
    expect(isRepeating({ series_id: null, recurring: false })).toBe(false);
    expect(isRepeating({})).toBe(false);
  });
});

describe("what's picked on open", () => {
  it("picks This event when it's offered, and nothing otherwise", () => {
    expect(defaultScope(ready(offered("t")))).toBe("this");
    // Visibility applies to every occurrence (Bobby 17-10 #1): Save waits for a pick.
    expect(defaultScope(ready({ row: "refused", message: VISIBILITY }))).toBeNull();
    expect(defaultScope(ready(pending, pending, pending))).toBeNull();
    expect(defaultScope(ready({ row: "conflict", fields: ["summary"] }))).toBeNull();
    // "This and following" from the first occurrence is never picked for the user.
    expect(defaultScope(ready({ row: "as_all", warnings: [], token: "t" }))).toBeNull();
  });
});

describe("the rows", () => {
  it("labels them as Google Calendar does, in order", () => {
    const rows = rowsOf(ready(offered("t")), "save");
    expect(rows.map((r) => r.label)).toEqual([
      "This event",
      "This and following events",
      "All events",
    ]);
    expect(rows.map((r) => r.scope)).toEqual(["this", "following", "all"]);
    expect(rows.map((r) => [r.selectable, r.token])).toEqual([
      [true, "t"],
      [true, "f"],
      [true, "a"],
    ]);
  });

  it("says what each offered row does, a delete's 'this and following' skipping the Trash", () => {
    const save = rowsOf(ready(offered("t")), "save");
    expect(save.map((r) => r.detail)).toEqual([
      "Only this occurrence.",
      "This occurrence and every one after it. Earlier ones stay as they are.",
      "Every occurrence, earlier ones too.",
    ]);
    expect(save[1].detail).not.toMatch(/Trash/);
    const del = rowsOf(ready(offered("t")), "delete", "19-10");
    expect(del[0].detail).toBe("Only the one on 19-10.");
    expect(del[1].detail).toBe(
      "This occurrence and every one after it. Google Calendar can't restore these from its Trash.",
    );
    // No Trash claim where it isn't proved.
    expect(del[0].detail + del[2].detail).not.toMatch(/Trash/);
    expect(rowsOf(ready(offered("t")), "delete")[0].detail).toBe("Only this occurrence.");
  });

  it("says what 'this and following' means from the first occurrence", () => {
    const asAll: ScopeRow = { row: "as_all", warnings: [], token: "x" };
    const [, save] = rowsOf(ready(offered("t"), asAll), "save");
    expect(save).toMatchObject({
      label: "This and following events",
      selectable: true,
      token: "x",
    });
    expect(save.detail).toBe(
      "This is the first occurrence, so this changes every occurrence, like All events.",
    );
    const [, del] = rowsOf(ready(offered("t"), asAll), "delete");
    expect(del.detail).toBe(
      "This is the first occurrence, so this deletes every occurrence, like All events.",
    );
  });

  it("shows a row that can't be chosen disabled, with why", () => {
    const rows = rowsOf(
      ready(
        { row: "refused", message: VISIBILITY },
        { row: "conflict", fields: ["start", "end", "summary"] },
        pending,
      ),
      "save",
    );
    // The backend's words, verbatim.
    expect(rows[0]).toMatchObject({ detail: VISIBILITY, selectable: false, token: null });
    expect(rows[1]).toMatchObject({
      detail: "Changed in Google since you opened it (the time and title).",
      selectable: false,
    });
    expect(rows[2]).toMatchObject({
      detail: "Not until the earlier change below is settled.",
      selectable: false,
      token: null,
    });
    expect(rowsOf(ready({ row: "conflict", fields: [] }), "save")[0].detail).toBe(
      "Changed in Google since you opened it.",
    );
  });

  // C12: the dialog asks Send or Don't send for a row exactly when it carries who would be emailed.
  it("carries who an offered row would email, and nobody for the rest", () => {
    const guests = {
      updated: 2,
      invited: 0,
      removed: 0,
      cancelled: false,
      people: 2,
      emails: 2,
      complete: true,
    };
    const rows = rowsOf(
      ready(
        { row: "offered", warnings: [], token: "t", guests },
        { row: "as_all", warnings: [], token: "f", guests },
        { row: "refused", message: VISIBILITY },
      ),
      "save",
    );
    expect(rows.map((r) => r.guests)).toEqual([guests, guests, null]);
    expect(rowsOf(ready(offered("t")), "save").map((r) => r.guests)).toEqual([null, null, null]);
  });
});

describe("an earlier change PM couldn't finish", () => {
  it("is one settle block, from whichever row holds it", () => {
    expect(pendingOf(ready(offered("t")))).toBeNull();
    expect(pendingOf(ready(offered("t"), pending, pending))).toEqual({
      token: "p",
      offers: ["remove", "check"],
      from: "2026-11-02",
      emails: [],
    });
    // C12: which settles would email guests comes with it.
    const sent: ScopeRow = { ...pending, emails: ["remove"] } as ScopeRow;
    expect(pendingOf(ready(offered("t"), sent, sent))?.emails).toEqual(["remove"]);
  });

  // C12 critiques C1-M4, C2-M2: a change that went with Send says, before a button is pressed,
  // which settles email guests; only the offers that would, and none after Don't send.
  it("says which settles would email guests", () => {
    expect(pendingText(["finish", "remove"], "2026-11-02", ["finish", "remove"])).toBe(
      "PM couldn't finish an earlier change to this repeating event: it may show twice from 02-11. Finish it or remove the new part first. If you remove the new part, guests will get a cancellation email. If you finish it, guests will get the rest of the update emails.",
    );
    expect(pendingText(["remove", "check"], "2026-11-02", ["remove"])).toMatch(
      /Remove the new part, or check again\. If you remove the new part, guests will get a cancellation email\.$/,
    );
    // Only what's offered: a Finish no longer offered says nothing of its emails.
    expect(pendingText(["remove", "check"], "2026-11-02", ["finish"])).not.toMatch(/guests/);
    expect(pendingText(["finish", "remove"], "2026-11-02", [])).not.toMatch(/guests/);
  });

  it("is worded by what Google allows now, never hinting at deleting without Remove", () => {
    const cases: [SplitAction[], string][] = [
      [
        ["finish", "remove"],
        "PM couldn't finish an earlier change to this repeating event: it may show twice from 02-11. Finish it or remove the new part first.",
      ],
      [
        ["finish"],
        "An earlier change from 02-11 is saved, but PM couldn't finish moving the occurrences you changed on their own. Finish it first.",
      ],
      [
        ["remove", "check"],
        "PM couldn't finish an earlier change to this repeating event: it may show twice from 02-11. Remove the new part, or check again.",
      ],
      [
        ["remove"],
        "PM couldn't finish an earlier change to this repeating event: it may show twice from 02-11. Remove the new part, or check again.",
      ],
      // Also a new part changed in Google since: never "lost touch" (C2-M5).
      [
        ["check"],
        "PM couldn't finish an earlier change to this repeating event from 02-11. Check again; if it still can't, look at that day in Google Calendar.",
      ],
    ];
    for (const [offers, text] of cases) expect(pendingText(offers, "2026-11-02")).toBe(text);
    expect(pendingText(["finish"], "2026-11-02")).not.toMatch(/delete|remove/i);
    expect(pendingText(["check"], "2026-11-02")).not.toMatch(/lost touch/);
  });
});

describe("a row Google changed since the editor opened", () => {
  it("is the union of the conflict rows' fields, an empty list included", () => {
    expect(conflictOf(ready(offered("t")))).toBeNull();
    expect(
      conflictOf(
        ready(
          { row: "conflict", fields: ["summary"] },
          { row: "conflict", fields: ["summary", "start"] },
        ),
      ),
    ).toEqual(["summary", "start"]);
    expect(conflictOf(ready({ row: "conflict", fields: [] }))).toEqual([]);
  });
});

describe("the warnings", () => {
  const every: SeriesWarning[] = [
    dayMove("every Sunday", "every Monday", 1),
    { warning: "becomes_one_event", day: "2026-10-05" },
    { warning: "start_moved", from: "2026-10-05", to: "2026-10-06" },
    { warning: "changes_lost", count: 2 },
    { warning: "length_changes" },
    { warning: "exceptions_may_reset", count: 1, complete: true },
    { warning: "not_carried", attachments: true, conference: true, label: true },
    { warning: "new_identity" },
    { warning: "split_sibling" },
  ];

  it("words every warning, for a save and a delete, with no jargon", () => {
    for (const w of every) {
      for (const kind of ["save", "delete"] as const) {
        for (const scope of ["following", "all"] as const) {
          const line = seriesWarningText(w, kind, scope, "Gym");
          expect(line).toMatch(/^[“A-Z0-9].*[.?]$/);
          expect(line).not.toMatch(JARGON);
        }
      }
    }
    expect(seriesWarningText(every[2], "save", "all", "Gym")).toBe(
      "The first occurrence of “Gym” moves from 05-10 to 06-10.",
    );
    expect(seriesWarningText({ warning: "length_changes" }, "save", "following", "Gym")).toBe(
      "Every occurrence from this one on will last as long as this one.",
    );
    expect(seriesWarningText({ warning: "length_changes" }, "save", "all", "Gym")).toBe(
      "Every occurrence will last as long as this one.",
    );
  });

  it("hedges on occurrences changed on their own: one, several, or some PM couldn't see", () => {
    const reset = (count: number, complete: boolean) =>
      seriesWarningText({ warning: "exceptions_may_reset", count, complete }, "save", "all", "Gym");
    expect(reset(1, true)).toBe(
      "One occurrence of “Gym” was changed on its own. Google may keep or lose that change.",
    );
    expect(reset(3, true)).toBe(
      "3 occurrences of “Gym” were changed on their own. Google may keep or lose those changes.",
    );
    expect(reset(3, false)).toBe(
      "Some occurrences of “Gym” may have been changed on their own (PM couldn't see them all). Google may keep or lose those changes.",
    );
  });

  it("says what the new part won't have, and to add a call only when it had one", () => {
    const carried = (attachments: boolean, conference: boolean, label: boolean) =>
      seriesWarningText(
        { warning: "not_carried", attachments, conference, label },
        "save",
        "following",
        "Gym",
      );
    expect(carried(true, true, true)).toBe(
      "The new repeating event this makes won't have the video call, the attachments and the label. The earlier part keeps them. Add a call to it in Google Calendar if you need one.",
    );
    expect(carried(false, true, false)).toBe(
      "The new repeating event this makes won't have the video call. The earlier part keeps it. Add a call to it in Google Calendar if you need one.",
    );
    expect(carried(true, false, true)).toBe(
      "The new repeating event this makes won't have the attachments and the label. The earlier part keeps them.",
    );
    expect(carried(false, false, true)).not.toMatch(/call/);
  });

  it("says where links go when it becomes a new repeating event, and what an earlier change made", () => {
    // C9b-2: a milestone is offered a move after saving (hedged: only when PM can make the offer),
    // and the flags' warning stays before the save (scope-L9).
    expect(seriesWarningText({ warning: "new_identity" }, "save", "following", "Gym")).toBe(
      "From this occurrence on, it becomes a new repeating event in Google. If a milestone linked to it falls on or after this day, PM offers to move it to the new part after saving. Reminders to prepare may show again for the new part.",
    );
    const sibling = (kind: "save" | "delete") =>
      seriesWarningText({ warning: "split_sibling" }, kind, "all", "Gym");
    expect(sibling("save")).toBe(
      "Part of “Gym” was made into a repeating event of its own by an earlier “this and following” change. Google may treat that part as this one too.",
    );
    expect(sibling("delete")).toMatch(
      /earlier “this and following” change\. Google may delete that part too\.$/,
    );
  });

  it("puts the question first, then the rest in a fixed order", () => {
    const row: RowView = {
      scope: "following",
      label: "This and following events",
      detail: "",
      selectable: true,
      asAll: false,
      token: "f",
      warnings: [...every].reverse(),
      guests: null,
    };
    const lines = warningLines(row, "save", "Gym");
    expect(lines).toEqual(every.map((w) => seriesWarningText(w, "save", "following", "Gym")));
    expect(lines[0]).toMatch(/\?$/);
    // A "not carried" naming nothing says nothing, rather than an empty line.
    const nothing: SeriesWarning = {
      warning: "not_carried",
      attachments: false,
      conference: false,
      label: false,
    };
    expect(
      warningLines({ ...row, warnings: [nothing, { warning: "new_identity" }] }, "save", "Gym"),
    ).toEqual([seriesWarningText({ warning: "new_identity" }, "save", "following", "Gym")]);
  });

  it("words a new repeat and 'Does not repeat' for each row (C11 §9.9)", () => {
    const say = (w: SeriesWarning, scope: "following" | "all") =>
      seriesWarningText(w, "save", scope, "Gym");
    const stops: SeriesWarning = { warning: "becomes_one_event", day: "2026-10-05" };
    expect(say(stops, "all")).toBe(
      "Only the occurrence on 05-10 stays, as an event that doesn't repeat. Every other occurrence of “Gym” goes.",
    );
    // Never "a new repeating event" for one that doesn't repeat (C11 critique 2 M2).
    expect(say(stops, "following")).toBe(
      "From this occurrence on, “Gym” becomes a separate event that doesn't repeat: this occurrence stays, and every one after it goes.",
    );
    expect(say({ warning: "changes_lost", count: 1 }, "following")).toBe(
      "One occurrence of “Gym” you changed on its own falls on a day the new repeat skips, so that change goes.",
    );
    expect(say({ warning: "changes_lost", count: 3 }, "following")).toBe(
      "3 occurrences of “Gym” you changed on their own fall on days the new repeat skips, so those changes go.",
    );
    const moved: SeriesWarning = { warning: "start_moved", from: "2026-10-20", to: "2026-10-21" };
    expect(say(moved, "following")).toBe(
      "From this occurrence on, the new part of “Gym” starts on 21-10, the first day the new repeat allows.",
    );
    expect(say(moved, "all")).toBe("The first occurrence of “Gym” moves from 20-10 to 21-10.");
    // The question first, then the stop, where it starts and what's lost, before the rest.
    const row: RowView = {
      scope: "following",
      label: "This and following events",
      detail: "",
      selectable: true,
      asAll: false,
      token: "f",
      warnings: [
        { warning: "new_identity" },
        { warning: "changes_lost", count: 1 },
        moved,
        { warning: "length_changes" },
        stops,
        dayMove("every Sunday", "every Monday", 1),
      ],
      guests: null,
    };
    const fromHere = (w: SeriesWarning) => say(w, "following");
    expect(warningLines(row, "save", "Gym")).toEqual([
      fromHere(dayMove("every Sunday", "every Monday", 1)),
      fromHere(stops),
      fromHere(moved),
      fromHere({ warning: "changes_lost", count: 1 }),
      fromHere({ warning: "length_changes" }),
      fromHere({ warning: "new_identity" }),
    ]);
  });

  it("words a 'this and following' row that changes every occurrence as All", () => {
    // From the first occurrence: no new part, so never "the new part starts" or "from this one on".
    const warnings: SeriesWarning[] = [
      { warning: "start_moved", from: "2026-10-05", to: "2026-10-06" },
      { warning: "becomes_one_event", day: "2026-10-06" },
    ];
    const [, asAll] = rowsOf(ready(offered("t"), { row: "as_all", warnings, token: "x" }), "save");
    expect(asAll).toMatchObject({ scope: "following", asAll: true, token: "x" });
    expect(warningLines(asAll, "save", "Gym")).toEqual([
      "Only the occurrence on 06-10 stays, as an event that doesn't repeat. Every other occurrence of “Gym” goes.",
      "The first occurrence of “Gym” moves from 05-10 to 06-10.",
    ]);
    // The same warnings on a row that makes a new part keep that row's words.
    const [, split] = rowsOf(ready(offered("t"), offered("f", warnings)), "save");
    expect(split.asAll).toBe(false);
    expect(warningLines(split, "save", "Gym")).toEqual([
      "From this occurrence on, “Gym” becomes a separate event that doesn't repeat: this occurrence stays, and every one after it goes.",
      "From this occurrence on, the new part of “Gym” starts on 06-10, the first day the new repeat allows.",
    ]);
  });

  it("says a re-ask once per kind", () => {
    expect(againText("save")).toBe(
      "This repeating event changed since you chose which occurrences to change. Choose again; nothing was saved.",
    );
    expect(againText("delete")).toMatch(/to delete\. Choose again; nothing was deleted\.$/);
  });
});

describe("the question a move to another day asks", () => {
  it("asks the six examples exactly", () => {
    expect(repeatDayQuestion("Gym", dayMove("every Sunday", "every Monday", 1), "all")).toBe(
      "“Gym” repeats every Sunday. Change it to repeat every Monday?",
    );
    expect(repeatDayQuestion("Gym", dayMove("every Sunday", "every Monday", 8), "all")).toBe(
      "“Gym” repeats every Sunday. Change it to repeat every Monday? Every occurrence moves a week and a day later.",
    );
    expect(repeatDayQuestion("Gym", dayMove("every Monday", "every Monday", 7), "all")).toBe(
      "“Gym” repeats every Monday. Move every occurrence a week later?",
    );
    expect(
      repeatDayQuestion(
        "Rent",
        dayMove("on day 15 of each month", "on day 16 of each month", 1),
        "all",
      ),
    ).toBe("“Rent” repeats on day 15 of each month. Change it to repeat on day 16 of each month?");
    expect(repeatDayQuestion("Walk", dayMove(null, null, 1), "all")).toBe(
      "Move every occurrence of “Walk” a day later?",
    );
    expect(repeatDayQuestion("Gym", dayMove("every Sunday", "every Monday", 1), "following")).toBe(
      "“Gym” repeats every Sunday. Change it to repeat every Monday from 21-10?",
    );
  });

  it("words every before/after case for both scopes, never naming one day twice", () => {
    const pairs: [string | null, string | null][] = [
      ["every Sunday", "every Monday"],
      ["every Monday", "every Monday"],
      ["every Monday", null],
      [null, "every Monday"],
      [null, null],
    ];
    for (const [before, after] of pairs) {
      for (const scope of ["following", "all"] as const) {
        for (const shift of [1, 6, -6, 7, 8, 14, 15, 30]) {
          const q = repeatDayQuestion("Gym", dayMove(before, after, shift), scope);
          expect(q).toContain("“Gym”");
          expect(q).toMatch(/\?/);
          expect(q).toMatch(shift > 0 ? /later|repeat every/ : /earlier|repeat every/);
          if (before !== null && before === after) {
            expect(q.split(before).length - 1, q).toBe(1);
          }
          if (scope === "following" && after !== null && after !== before) {
            expect(q).toContain("from 21-10");
          }
        }
      }
    }
    expect(repeatDayQuestion("Gym", dayMove("every Monday", null, -6), "following")).toBe(
      "“Gym” repeats every Monday. Move this occurrence and every one after it 6 days earlier?",
    );
    expect(repeatDayQuestion("Gym", dayMove(null, "every Tuesday", 15), "all")).toBe(
      "Change “Gym” to repeat every Tuesday? Every occurrence moves 2 weeks and a day later.",
    );
    expect(repeatDayQuestion("Gym", dayMove(null, null, 14), "following")).toBe(
      "Move this occurrence of “Gym” and every one after it 2 weeks later?",
    );
  });

  it("says no distance for a move of no days (C1-L10)", () => {
    for (const [before, after] of [
      ["every Sunday", "every Monday"],
      ["every Monday", "every Monday"],
      [null, "every Monday"],
      [null, null],
    ] as [string | null, string | null][]) {
      const q = repeatDayQuestion("Gym", dayMove(before, after, 0), "all");
      expect(q).not.toMatch(/\bmoves?\b|\ba day\b|\d days|week|later|earlier/);
    }
  });

  it("says a distance in days and weeks", () => {
    const cases: [number, string][] = [
      [1, "a day"],
      [2, "2 days"],
      [6, "6 days"],
      [7, "a week"],
      [8, "a week and a day"],
      [9, "a week and 2 days"],
      [13, "a week and 6 days"],
      [14, "2 weeks"],
      [15, "2 weeks and a day"],
      [21, "3 weeks"],
      [27, "3 weeks and 6 days"],
      [28, "28 days"],
      [31, "31 days"],
    ];
    for (const [n, text] of cases) expect(distanceText(n)).toBe(text);
  });
});
