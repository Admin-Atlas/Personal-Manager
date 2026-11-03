// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared task wording and order. "Today" is always passed in, so no assertion here depends on
// the wall clock; any DD-MM text is built with the same `formatDateLocal` the code uses, which
// drops the year in the current year (so the expected text can't drift with the date the suite
// runs on).

import { describe, expect, it } from "vitest";
import { addDays } from "./calendar-layout";
import { formatDateLocal } from "./format";
import {
  compareTasks,
  deadlineInfo,
  emptyTaskInput,
  eventChoices,
  filterEventChoices,
  isOpen,
  nextStatus,
  STACK_KIND_LABEL,
  taskToInput,
  TONE_CLASS,
} from "./tasks";
import type { CalendarEvent, Task } from "./types";

const TODAY = new Date(2026, 10, 3, 15, 30); // Tue 03-11-2026, mid-afternoon
const iso = (offset: number) => {
  const d = addDays(TODAY, offset);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

function task(over: Partial<Task> = {}): Task {
  return {
    id: 1,
    title: "Write the pitch",
    why: null,
    project_name: "Atlas",
    project_archived: false,
    milestone_id: null,
    event_uid: null,
    do_start: null,
    do_end: null,
    deadline: null,
    deadline_kind: "hard",
    deadline_from: "date",
    effective_deadline: null,
    deadline_missing: false,
    milestone_label: null,
    event_title: null,
    status: "todo",
    completed_at: null,
    lane: "foreground",
    stack_id: null,
    parked: false,
    ordinal: 0,
    source: "user",
    created_at: "2026-11-01T00:00:00.000Z",
    updated_at: "2026-11-01T00:00:00.000Z",
    ...over,
  };
}

function event(over: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id: "e1",
    calendar_id: "cal",
    summary: "Demo day",
    description: null,
    location: null,
    start: iso(4),
    end: null,
    all_day: true,
    html_link: null,
    uid: "uid-1",
    ...over,
  };
}

describe("nextStatus", () => {
  it("cycles todo → doing → done → todo", () => {
    expect(nextStatus("todo")).toBe("doing");
    expect(nextStatus("doing")).toBe("done");
    expect(nextStatus("done")).toBe("todo");
  });
});

describe("deadlineInfo", () => {
  it("is null without a readable date", () => {
    expect(deadlineInfo(null, "hard", TODAY)).toBeNull();
    expect(deadlineInfo("", "hard", TODAY)).toBeNull();
    expect(deadlineInfo("soon", "hard", TODAY)).toBeNull();
  });

  it("says how late a past deadline is, toned late", () => {
    expect(deadlineInfo(iso(-1), "hard", TODAY)).toEqual({ text: "1d late", tone: "late" });
    expect(deadlineInfo(iso(-9), "hard", TODAY)).toEqual({ text: "9d late", tone: "late" });
  });

  it("reads today as late, tomorrow as soon", () => {
    expect(deadlineInfo(iso(0), "hard", TODAY)).toEqual({ text: "today", tone: "late" });
    expect(deadlineInfo(iso(1), "hard", TODAY)).toEqual({ text: "tomorrow", tone: "soon" });
  });

  it("counts days with the date from 2 to 6 days out, toned soon", () => {
    for (const n of [2, 6]) {
      expect(deadlineInfo(iso(n), "hard", TODAY)).toEqual({
        text: `${n}d · ${formatDateLocal(addDays(TODAY, n))}`,
        tone: "soon",
      });
    }
  });

  it("shows the plain date a week or more out, toned normal", () => {
    expect(deadlineInfo(iso(7), "hard", TODAY)).toEqual({
      text: formatDateLocal(addDays(TODAY, 7)),
      tone: "normal",
    });
    expect(deadlineInfo(iso(40), "hard", TODAY)?.tone).toBe("normal");
  });

  it("prefixes a soft deadline with aim and never tones it late", () => {
    expect(deadlineInfo(iso(-2), "soft", TODAY)).toEqual({ text: "aim 2d late", tone: "soon" });
    expect(deadlineInfo(iso(0), "soft", TODAY)).toEqual({ text: "aim today", tone: "soon" });
    expect(deadlineInfo(iso(1), "soft", TODAY)).toEqual({ text: "aim tomorrow", tone: "soon" });
    expect(deadlineInfo(iso(10), "soft", TODAY)).toEqual({
      text: `aim ${formatDateLocal(addDays(TODAY, 10))}`,
      tone: "normal",
    });
  });

  it("gives a done task no tone", () => {
    expect(deadlineInfo(iso(-3), "hard", TODAY, true)).toEqual({ text: "3d late", tone: "normal" });
    expect(deadlineInfo(iso(1), "soft", TODAY, true)?.tone).toBe("normal");
  });

  it("counts whole local days whatever the time of day", () => {
    const lateEvening = new Date(2026, 10, 3, 23, 59);
    expect(deadlineInfo(iso(1), "hard", lateEvening)?.text).toBe("tomorrow");
  });
});

describe("TONE_CLASS", () => {
  it("maps late to due, soon to look, normal to the quiet ink", () => {
    expect(TONE_CLASS).toEqual({ late: "text-st-due", soon: "text-st-look", normal: "text-ink4" });
  });
});

describe("isOpen and compareTasks", () => {
  it("counts to do and doing as open", () => {
    expect(isOpen({ status: "todo" })).toBe(true);
    expect(isOpen({ status: "doing" })).toBe(true);
    expect(isOpen({ status: "done" })).toBe(false);
  });

  it("orders by ordinal, then id", () => {
    const list = [
      task({ id: 3, ordinal: 1 }),
      task({ id: 2, ordinal: 0 }),
      task({ id: 1, ordinal: 1 }),
    ];
    expect([...list].sort(compareTasks).map((t) => t.id)).toEqual([2, 1, 3]);
  });
});

describe("STACK_KIND_LABEL", () => {
  it("names every stack kind", () => {
    expect(STACK_KIND_LABEL).toEqual({
      sequential: "In sequence: each one gates the next",
      in_order: "In order",
      waiting_on: "Waiting on someone",
    });
  });
});

describe("editor input", () => {
  it("starts blank, with the defaults laid over", () => {
    const input = emptyTaskInput({ project: "Atlas", doStart: "2026-11-03" });
    expect(input.title).toBe("");
    expect(input.project).toBe("Atlas");
    expect(input.doStart).toBe("2026-11-03");
    expect(input.deadlineKind).toBe("hard");
    expect(input.deadlineFrom).toBe("date");
    expect(input.lane).toBe("foreground");
    expect(input.parked).toBe(false);
  });

  it("carries a stored task's fields across", () => {
    const input = taskToInput(
      task({ milestone_id: 7, deadline_from: "milestone", deadline: "2026-11-20", stack_id: 2 }),
    );
    expect(input).toMatchObject({
      title: "Write the pitch",
      project: "Atlas",
      milestoneId: 7,
      deadline: "2026-11-20",
      deadlineFrom: "milestone",
      stackId: 2,
    });
  });

  it("puts a deadline whose link is gone back on the task's own date", () => {
    expect(taskToInput(task({ deadline_from: "milestone", milestone_id: null })).deadlineFrom).toBe(
      "date",
    );
    expect(taskToInput(task({ deadline_from: "event", event_uid: null })).deadlineFrom).toBe(
      "date",
    );
    expect(taskToInput(task({ deadline_from: "event", event_uid: "u" })).deadlineFrom).toBe(
      "event",
    );
  });
});

describe("eventChoices", () => {
  it("keeps one choice per UID, at its next occurrence on or after today", () => {
    const out = eventChoices(
      [
        event({ id: "a", start: iso(-2) }),
        event({ id: "b", start: iso(9) }),
        event({ id: "c", start: iso(2) }),
        event({ id: "d", start: iso(0) }),
      ],
      TODAY,
    );
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ uid: "uid-1", date: iso(0), past: false });
    expect(out[0].label).toBe(`${formatDateLocal(addDays(TODAY, 0))} · Demo day`);
  });

  it("falls back to the latest past occurrence within 30 days, and drops older ones", () => {
    const out = eventChoices(
      [
        event({ uid: "recent", start: iso(-5) }),
        event({ uid: "recent", start: iso(-12) }),
        event({ uid: "old", start: iso(-31) }),
        event({ uid: "edge", start: iso(-30) }),
      ],
      TODAY,
    );
    expect(out.map((c) => [c.uid, c.date, c.past])).toEqual([
      ["recent", iso(-5), true],
      ["edge", iso(-30), true],
    ]);
  });

  it("lists upcoming choices soonest first, then past ones latest first", () => {
    const out = eventChoices(
      [
        event({ uid: "later", start: iso(10) }),
        event({ uid: "past-far", start: iso(-20) }),
        event({ uid: "soon", start: iso(1) }),
        event({ uid: "past-near", start: iso(-1) }),
      ],
      TODAY,
    );
    expect(out.map((c) => c.uid)).toEqual(["soon", "later", "past-near", "past-far"]);
  });

  it("buckets a timed event on its local day, skips UID-less events, names untitled ones", () => {
    const at = addDays(TODAY, 3);
    at.setHours(9, 0, 0, 0);
    const out = eventChoices(
      [
        event({ uid: "timed", start: at.toISOString(), all_day: false, summary: "  " }),
        event({ uid: null, start: iso(1) }),
      ],
      TODAY,
    );
    expect(out).toEqual([
      {
        uid: "timed",
        date: iso(3),
        title: "Untitled event",
        label: `${formatDateLocal(addDays(TODAY, 3))} · Untitled event`,
        past: false,
      },
    ]);
  });

  it("filters by every word of the query, ignoring case", () => {
    const choices = eventChoices(
      [
        event({ uid: "a", summary: "Demo day", start: iso(1) }),
        event({ uid: "b", summary: "Team lunch", start: iso(2) }),
      ],
      TODAY,
    );
    expect(filterEventChoices(choices, "").map((c) => c.uid)).toEqual(["a", "b"]);
    expect(filterEventChoices(choices, "DEMO").map((c) => c.uid)).toEqual(["a"]);
    expect(filterEventChoices(choices, "team lunch").map((c) => c.uid)).toEqual(["b"]);
    expect(filterEventChoices(choices, "team demo")).toEqual([]);
  });
});
