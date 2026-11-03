// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Runway's pure model (PA-2): which columns a day has (week edges, windows laid over the
// defaults, cut, split, overlapping, ended, running), where each task and milestone sits
// (roll-forward and why, done ones staying the day they're ticked and then dropping back for 14
// days, stacks of one and of two counted over the whole stack, archived and parked), what a drop
// writes (a new plan, or the same days back in its own column), a step up or down a lane, the
// columns' events strips, and the tasks overlay the calendar grids draw.

import { describe, expect, it } from "vitest";
import type { RunwayWindow } from "./calendarPrefs";
import { TASK_CALENDAR_ID, weekdayShort } from "./calendar-layout";
import { formatDateLocal } from "./format";
import {
  buildColumns,
  columnForDay,
  dropPlan,
  laneTaskIds,
  LATER_EVENTS,
  linkedEvent,
  orderAfterDrop,
  orderAfterStep,
  placeCards,
  placeEvents,
  samePlan,
  taskOverlayEvents,
  type RunwayColumn,
  type RunwayModel,
} from "./runway";
import type { CalendarEvent, Milestone, Task, TaskStack } from "./types";

const day = (iso: string) => {
  const [y, m, d] = iso.split("-").map(Number);
  return new Date(y, m - 1, d, 10, 30);
};
const WED = day("2026-11-04");
const SAT = day("2026-11-07");
const SUN = day("2026-11-08");

function task(over: Partial<Task> = {}): Task {
  const t: Task = {
    id: 1,
    title: "Task",
    why: null,
    project_name: null,
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
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    ...over,
  };
  return { ...t, ordinal: over.ordinal ?? t.id };
}

function milestone(over: Partial<Milestone> = {}): Milestone {
  return {
    id: 1,
    project_name: "Atlas",
    label: "Pitch",
    due_date: null,
    event_uid: null,
    calendar_linked: false,
    event_missing: false,
    state: null,
    status: null,
    source_type: null,
    external_id: null,
    sort_order: 0,
    ...over,
  };
}

const stack = (id: number, title = "Visa"): TaskStack => ({
  id,
  title,
  kind: "sequential",
  note: null,
  created_at: "",
  updated_at: "",
});

const win = (id: string, start: string, end: string, label = id): RunwayWindow => ({
  id,
  label,
  start,
  end,
});

/** Each column as `id label start..end`, with a `*` on today's. */
const shape = (cols: RunwayColumn[]) =>
  cols.map((c) => `${c.today ? "*" : ""}${c.id} ${c.label} ${c.start ?? "…"}..${c.end ?? "…"}`);

function place(
  tasks: Task[],
  opts: {
    today?: Date;
    windows?: RunwayWindow[];
    milestones?: Milestone[];
    stacks?: TaskStack[];
    archived?: string[];
  } = {},
): { model: RunwayModel; columns: RunwayColumn[] } {
  const today = opts.today ?? WED;
  const columns = buildColumns(today, opts.windows ?? []);
  const model = placeCards({
    tasks,
    stacks: opts.stacks ?? [],
    milestones: opts.milestones ?? [],
    columns,
    today,
    archived: new Set(opts.archived ?? []),
  });
  return { model, columns };
}

/** Where each card went: `column/lane: ids`, tasks by id, milestones as `m<id>`, stacks `s<id>[…]`. */
function where(model: RunwayModel): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [col, lanes] of Object.entries(model.lanes)) {
    for (const lane of ["foreground", "background"] as const) {
      const cards = lanes[lane];
      if (cards.length === 0) continue;
      out[`${col}/${lane}`] = cards
        .map((c) =>
          c.kind === "task"
            ? `${c.task.id}${c.rolled ? `↻${c.daysAgo}` : ""}`
            : c.kind === "milestone"
              ? `m${c.milestone.id}${c.late ? "!" : ""}`
              : `s${c.stack.id}[${c.tasks.map((t) => t.id).join(",")}]${c.left}`,
        )
        .join(" ");
    }
  }
  if (model.parked.length > 0) out.parked = model.parked.map((c) => c.task.id).join(" ");
  return out;
}

describe("buildColumns", () => {
  it("lays out a midweek day: today, tomorrow, the rest of the week, next week, later", () => {
    expect(shape(buildColumns(WED, []))).toEqual([
      "past Before today …..2026-11-03",
      "*today Today 2026-11-04..2026-11-04",
      "tomorrow Tomorrow 2026-11-05..2026-11-05",
      "rest Rest of this week 2026-11-06..2026-11-08",
      "next Next week 2026-11-09..2026-11-15",
      "later Later 2026-11-16..…",
    ]);
  });

  it("has no rest of the week on a Saturday", () => {
    expect(shape(buildColumns(SAT, []))).toEqual([
      "past Before today …..2026-11-06",
      "*today Today 2026-11-07..2026-11-07",
      "tomorrow Tomorrow 2026-11-08..2026-11-08",
      "next Next week 2026-11-09..2026-11-15",
      "later Later 2026-11-16..…",
    ]);
  });

  it("on a Sunday, tomorrow is next week's Monday, so next week starts the day after", () => {
    expect(shape(buildColumns(SUN, []))).toEqual([
      "past Before today …..2026-11-07",
      "*today Today 2026-11-08..2026-11-08",
      "tomorrow Tomorrow 2026-11-09..2026-11-09",
      "next Next week 2026-11-10..2026-11-15",
      "later Later 2026-11-16..…",
    ]);
  });

  it("says each column's days", () => {
    const cols = buildColumns(WED, []);
    const sub = (id: string) => cols.find((c) => c.id === id)!.sub;
    const w = (iso: string) => `${weekdayShort(day(iso))} ${iso.slice(8)}`;
    expect(sub("today")).toBe(w("2026-11-04"));
    expect(sub("rest")).toBe(`${w("2026-11-06")} – ${w("2026-11-08")}`);
    expect(sub("later")).toBe(`from ${formatDateLocal(day("2026-11-16"))}`);
    expect(sub("past")).toBe("last 14 days");
  });

  it("cuts a window out of the defaults and splits one it lands inside", () => {
    const cols = buildColumns(WED, [win("trip", "2026-11-11", "2026-11-12", "Lisbon")]);
    expect(shape(cols)).toEqual([
      "past Before today …..2026-11-03",
      "*today Today 2026-11-04..2026-11-04",
      "tomorrow Tomorrow 2026-11-05..2026-11-05",
      "rest Rest of this week 2026-11-06..2026-11-08",
      "next Next week 2026-11-09..2026-11-10",
      "window:trip Lisbon 2026-11-11..2026-11-12",
      "next:2 Next week 2026-11-13..2026-11-15",
      "later Later 2026-11-16..…",
    ]);
    expect(cols.find((c) => c.id === "window:trip")?.windowId).toBe("trip");
  });

  it("drops a default a window covers whole, and gives today to a window holding it", () => {
    const cols = buildColumns(WED, [win("sprint", "2026-11-02", "2026-11-05", "Sprint")]);
    expect(shape(cols)).toEqual([
      "past Before today …..2026-11-03",
      // Started on Monday: its column runs from today; its earlier days are past.
      "*window:sprint Sprint 2026-11-04..2026-11-05",
      "rest Rest of this week 2026-11-06..2026-11-08",
      "next Next week 2026-11-09..2026-11-15",
      "later Later 2026-11-16..…",
    ]);
  });

  it("leaves out windows that have ended, and ones that make no sense", () => {
    const cols = buildColumns(WED, [
      win("gone", "2026-10-20", "2026-11-03"),
      win("backwards", "2026-11-12", "2026-11-10"),
      win("junk", "soon", "later"),
    ]);
    expect(cols.filter((c) => c.kind === "window")).toEqual([]);
    // A window ending today still shows.
    expect(
      buildColumns(WED, [win("last", "2026-11-01", "2026-11-04")]).find((c) => c.today)?.id,
    ).toBe("window:last");
  });

  it("splits Later round a window far out, keeping Later open-ended after it", () => {
    const cols = buildColumns(WED, [win("hols", "2026-12-20", "2026-12-31", "Holidays")]);
    expect(shape(cols).slice(-3)).toEqual([
      "later Later 2026-11-16..2026-12-19",
      "window:hols Holidays 2026-12-20..2026-12-31",
      "later:2 Later 2027-01-01..…",
    ]);
  });

  it("shows both of two overlapping windows; a day in both is the shorter one's", () => {
    const cols = buildColumns(WED, [
      win("trip", "2026-11-10", "2026-11-20", "Trip"),
      win("conf", "2026-11-12", "2026-11-13", "Conference"),
    ]);
    expect(shape(cols).filter((s) => s.includes("window"))).toEqual([
      "window:trip Trip 2026-11-10..2026-11-20",
      "window:conf Conference 2026-11-12..2026-11-13",
    ]);
    // Neither default keeps a day of either.
    expect(shape(cols)).toContain("next Next week 2026-11-09..2026-11-09");
    expect(shape(cols)).toContain("later Later 2026-11-21..…");
    expect(columnForDay(cols, "2026-11-12")?.id).toBe("window:conf");
    expect(columnForDay(cols, "2026-11-14")?.id).toBe("window:trip");
    expect(columnForDay(cols, "2026-11-01")?.id).toBe("past");
    expect(columnForDay(cols, "2027-03-01")?.id).toBe("later");
  });

  it("has exactly one column holding today", () => {
    for (const d of [WED, SAT, SUN]) {
      expect(buildColumns(d, []).filter((c) => c.today)).toHaveLength(1);
    }
  });
});

describe("placeCards", () => {
  it("puts a task on its plan start, else its effective deadline, else Later", () => {
    const { model } = place([
      task({ id: 1, do_start: "2026-11-05" }),
      task({ id: 2, effective_deadline: "2026-11-10", deadline: null, deadline_from: "event" }),
      task({ id: 3 }),
      // The plan wins over the deadline: do it Monday, due Friday.
      task({ id: 4, do_start: "2026-11-09", effective_deadline: "2026-11-13" }),
      task({ id: 5, do_start: "2026-11-04", lane: "background" }),
    ]);
    expect(where(model)).toEqual({
      "today/background": "5",
      "tomorrow/foreground": "1",
      "next/foreground": "2 4",
      "later/foreground": "3",
    });
  });

  it("rolls an open task whose day has passed into today, saying how long ago", () => {
    const { model } = place([
      task({ id: 1, do_start: "2026-11-02" }),
      task({ id: 2, effective_deadline: "2026-10-30" }),
    ]);
    expect(where(model)).toEqual({ "today/foreground": "1↻2 2↻5" });
  });

  it("keeps a task whose plan window still runs through today in today, not rolled", () => {
    const { model } = place([task({ id: 1, do_start: "2026-11-02", do_end: "2026-11-06" })]);
    expect(where(model)).toEqual({ "today/foreground": "1" });
  });

  it("rolls into a window holding today when there is one", () => {
    const { model } = place([task({ id: 1, do_start: "2026-11-01" })], {
      windows: [win("w", "2026-11-03", "2026-11-06")],
    });
    expect(where(model)).toEqual({ "window:w/foreground": "1↻3" });
  });

  it("sits a task planned exactly on a window in that window", () => {
    const { model } = place([task({ id: 1, do_start: "2026-11-12", do_end: "2026-11-13" })], {
      windows: [win("trip", "2026-11-10", "2026-11-20"), win("conf", "2026-11-12", "2026-11-13")],
    });
    expect(where(model)).toEqual({ "window:conf/foreground": "1" });
  });

  it("drops done tasks back into Before today, for 14 days only", () => {
    const { model } = place([
      task({ id: 1, status: "done", do_start: "2026-11-01", completed_at: "2026-11-02T09:00:00Z" }),
      // Planned long ago but finished lately: still shown.
      task({ id: 2, status: "done", do_start: "2026-09-01", completed_at: "2026-11-01T09:00:00Z" }),
      // Planned and finished long ago: gone.
      task({ id: 3, status: "done", do_start: "2026-09-01", completed_at: "2026-09-02T09:00:00Z" }),
      // Planned lately, its finish unknown: shown.
      task({ id: 4, status: "done", do_start: "2026-10-25" }),
      // Done ahead of its day: stays in its column.
      task({ id: 5, status: "done", do_start: "2026-11-05", completed_at: "2026-11-03T09:00:00Z" }),
      // Undated, finished before today: Before today; finished long ago: gone.
      task({ id: 6, status: "done", completed_at: "2026-11-01T09:00:00Z" }),
      task({ id: 7, status: "done", completed_at: "2026-08-01T09:00:00Z" }),
    ]);
    expect(where(model)).toEqual({
      "past/foreground": "1 2 4 6",
      "tomorrow/foreground": "5",
    });
  });

  it("an undated task finished today stays in Later", () => {
    const today = new Date(2026, 10, 4, 15);
    const { model } = place([task({ id: 1, status: "done", completed_at: today.toISOString() })]);
    expect(where(model)).toEqual({ "later/foreground": "1" });
  });

  it("folds two or more of a stack in one column and lane into one card, where the first is", () => {
    const { model } = place(
      [
        task({ id: 1, do_start: "2026-11-04" }),
        task({ id: 2, do_start: "2026-11-04", stack_id: 7 }),
        task({ id: 3, do_start: "2026-11-04" }),
        task({ id: 4, do_start: "2026-11-04", stack_id: 7, status: "done" }),
        task({ id: 5, do_start: "2026-11-04", stack_id: 7 }),
        // One of stack 8 alone: a plain card.
        task({ id: 6, do_start: "2026-11-04", stack_id: 8 }),
        // Stack 7's member in another lane isn't folded with them.
        task({ id: 9, do_start: "2026-11-04", stack_id: 7, lane: "background" }),
      ],
      { stacks: [stack(7), stack(8)] },
    );
    // "3 left": the whole stack's open tasks, the background one too.
    expect(where(model)).toEqual({
      "today/foreground": "1 s7[2,4,5]3 3 6",
      "today/background": "9",
    });
    const card = model.lanes.today.foreground[1];
    expect(card.kind === "stack" && card.members.map((m) => m.task.id)).toEqual([2, 4, 5]);
    expect(card.kind === "stack" && card.chain.map((m) => m.card.task.id)).toEqual([2, 4, 5, 9]);
  });

  it("counts a stack's left and next over the whole stack, wherever each is planned", () => {
    const { model } = place(
      [
        task({ id: 1, title: "Book", do_start: "2026-11-04", stack_id: 7, status: "done" }),
        task({ id: 2, title: "Fill", do_start: "2026-11-04", stack_id: 7 }),
        task({ id: 3, title: "Post", do_start: "2026-11-04", stack_id: 7 }),
        task({ id: 4, title: "Chase", do_start: "2026-11-10", stack_id: 7 }),
        task({ id: 5, title: "Collect", do_start: "2026-11-11", stack_id: 7 }),
        task({ id: 6, title: "Shelved", stack_id: 7, parked: true }),
        // An archived project's task isn't in the stack's count.
        task({ id: 7, title: "Old", do_start: "2026-11-11", stack_id: 7, project_archived: true }),
      ],
      { stacks: [stack(7)] },
    );
    expect(where(model)).toEqual({
      "today/foreground": "s7[1,2,3]5",
      "next/foreground": "s7[4,5]5",
      parked: "6",
    });
    const today = model.lanes.today.foreground[0];
    const next = model.lanes.next.foreground[0];
    for (const c of [today, next]) {
      expect(c.kind).toBe("stack");
      if (c.kind !== "stack") continue;
      // The next open one in the stack's order, and the whole chain with where each sits.
      expect(c.next?.title).toBe("Fill");
      expect(c.chain.map((m) => `${m.card.task.id}@${m.columnId ?? "parked"}`)).toEqual([
        "1@today",
        "2@today",
        "3@today",
        "4@next",
        "5@next",
        "6@parked",
      ]);
    }
    // Only this column's part drags and moves as one.
    expect(next.kind === "stack" && next.tasks.map((t) => t.id)).toEqual([4, 5]);
  });

  it("keeps a rolled task ticked off today in today, counted as done, and lets it go tomorrow", () => {
    const tasks = [
      task({ id: 1, do_start: "2026-11-02", status: "done", completed_at: "2026-11-04T09:00:00" }),
      task({ id: 2, do_start: "2026-11-02" }),
      task({ id: 3, do_start: "2026-11-02" }),
    ];
    const { model } = place(tasks);
    expect(where(model)).toEqual({ "today/foreground": "1 2↻2 3↻2" });
    expect(model.progress).toEqual({ done: 1, total: 3, doing: 0 });
    // The next day it has gone back to Before today, with the open ones still rolled.
    const { model: tomorrow } = place(tasks, { today: day("2026-11-05") });
    expect(where(tomorrow)).toEqual({ "past/foreground": "1", "today/foreground": "2↻3 3↻3" });
    expect(tomorrow.progress).toEqual({ done: 0, total: 2, doing: 0 });
  });

  it("says why a task rolled: its plan, or a deadline it was never planned for", () => {
    const { model } = place([
      task({ id: 1, do_start: "2026-11-02", effective_deadline: "2026-11-01" }),
      task({ id: 2, effective_deadline: "2026-10-30", deadline_from: "milestone" }),
    ]);
    const [a, b] = model.lanes.today.foreground;
    expect(a.kind === "task" && [a.rolled, a.rolledBy, a.daysAgo]).toEqual([true, "plan", 2]);
    expect(b.kind === "task" && [b.rolled, b.rolledBy, b.daysAgo]).toEqual([true, "deadline", 5]);
  });

  it("keeps a task planned on a running window in it, though a shorter window holds today", () => {
    const windows = [
      win("trip", "2026-11-10", "2026-11-20"),
      win("conf", "2026-11-12", "2026-11-13"),
    ];
    const tasks = [task({ id: 1, do_start: "2026-11-10", do_end: "2026-11-20" })];
    // Before the trip, on its first day, and once the conference holds today.
    for (const d of ["2026-11-04", "2026-11-10", "2026-11-12"]) {
      const { model, columns } = place(tasks, { today: day(d), windows });
      expect(where(model)).toEqual({ "window:trip/foreground": "1" });
      const trip = columns.find((c) => c.id === "window:trip")!;
      expect(trip.windowStart).toBe("2026-11-10");
    }
    const { columns } = place(tasks, { today: day("2026-11-12"), windows });
    // The column starts at today; the window's own first day stays on it.
    expect(columns.find((c) => c.id === "window:trip")?.start).toBe("2026-11-12");
    expect(columns.find((c) => c.today)?.id).toBe("window:conf");
  });

  it("leaves off archived projects' tasks and milestones, and shelves parked ones", () => {
    const { model } = place(
      [
        task({ id: 1, do_start: "2026-11-04", project_name: "Old", project_archived: true }),
        task({ id: 2, do_start: "2026-11-04", project_name: "Gone" }),
        task({ id: 3, do_start: "2026-11-04", parked: true }),
        task({ id: 4, parked: true, effective_deadline: "2026-11-05" }),
        // A done task isn't parked any more: it sits by its day.
        task({ id: 5, do_start: "2026-11-05", parked: true, status: "done" }),
        task({ id: 6, do_start: "2026-11-04" }),
      ],
      {
        archived: ["Gone"],
        milestones: [milestone({ id: 1, project_name: "Gone", due_date: "2026-11-05" })],
      },
    );
    expect(where(model)).toEqual({
      "today/foreground": "6",
      "tomorrow/foreground": "5",
      parked: "3 4",
    });
  });

  it("places milestones: late ones in today, met past ones back, after the tasks", () => {
    const { model } = place([task({ id: 1, do_start: "2026-11-04" })], {
      milestones: [
        milestone({ id: 1, due_date: "2026-11-01" }),
        milestone({ id: 2, due_date: "2026-11-04" }),
        milestone({ id: 3, due_date: "2026-11-02", state: "met" }),
        milestone({ id: 4, due_date: "2026-11-02", status: "done" }),
        milestone({ id: 5, due_date: "2026-09-01", state: "met" }),
        milestone({ id: 6, due_date: null }),
        milestone({ id: 7, due_date: "2026-11-20T00:00:00" }),
      ],
    });
    expect(where(model)).toEqual({
      "today/foreground": "1 m1! m2",
      "past/foreground": "m3 m4",
      "later/foreground": "m7",
    });
  });

  it("counts progress from today on, and finds the next deadline", () => {
    const { model } = place(
      [
        task({ id: 1, do_start: "2026-11-04", status: "done" }),
        task({ id: 2, do_start: "2026-11-04", status: "doing", effective_deadline: "2026-11-09" }),
        task({ id: 3, effective_deadline: "2026-11-06", title: "Send the deck" }),
        task({ id: 4, parked: true, effective_deadline: "2026-11-04" }),
        task({ id: 5, status: "done", do_start: "2026-11-01" }),
      ],
      { milestones: [milestone({ id: 1, due_date: "2026-11-07", label: "Launch" })] },
    );
    expect(model.progress).toEqual({ done: 1, total: 4, doing: 1 });
    expect(model.nextUp).toEqual({
      title: "Send the deck",
      deadlineText: `2d · ${formatDateLocal(day("2026-11-06"))}`,
      tone: "soon",
    });
  });

  it("a late milestone is next up, ahead of a task due today", () => {
    const { model } = place([task({ id: 1, effective_deadline: "2026-11-04" })], {
      milestones: [milestone({ id: 1, due_date: "2026-11-02", label: "Late one" })],
    });
    expect(model.nextUp).toEqual({ title: "Late one", deadlineText: "2d late", tone: "late" });
  });

  it("has nothing next up without a dated open thing, and every column has lanes", () => {
    const { model, columns } = place([task({ id: 1 })]);
    expect(model.nextUp).toBeNull();
    for (const c of columns) expect(model.lanes[c.id]).toBeDefined();
  });
});

describe("dropPlan", () => {
  const cols = buildColumns(WED, [win("trip", "2026-11-11", "2026-11-12")]);
  const col = (id: string) => cols.find((c) => c.id === id)!;

  it("starts the plan on the column's first day; a window runs to its last", () => {
    const t = task({ id: 1, do_start: "2026-11-01", do_end: "2026-11-02", parked: true });
    expect(dropPlan({ kind: "column", column: col("tomorrow"), lane: "background" }, t)).toEqual({
      do_start: "2026-11-05",
      do_end: null,
      lane: "background",
      parked: false,
    });
    expect(dropPlan({ kind: "column", column: col("window:trip"), lane: "foreground" }, t)).toEqual(
      { do_start: "2026-11-11", do_end: "2026-11-12", lane: "foreground", parked: false },
    );
    expect(
      dropPlan({ kind: "column", column: col("later"), lane: "foreground" }, t)?.do_start,
    ).toBe("2026-11-16");
  });

  it("parks with the dates kept, and takes nothing into Before today", () => {
    const t = task({ id: 1, do_start: "2026-11-05", lane: "background" });
    expect(dropPlan({ kind: "parked" }, t)).toEqual({
      do_start: "2026-11-05",
      do_end: null,
      lane: "background",
      parked: true,
    });
    expect(dropPlan({ kind: "column", column: col("past"), lane: "foreground" }, t)).toBeNull();
  });

  it("back into its own column keeps its days and changes only the lane", () => {
    // Planned Friday, in the rest of the week (Fri 06 – Sun 08): a reorder there, or a move to
    // that column's background lane, leaves Friday alone.
    const fri = task({ id: 1, do_start: "2026-11-06" });
    const rest = col("rest");
    expect(dropPlan({ kind: "column", column: rest, lane: "foreground" }, fri, rest)).toEqual({
      do_start: "2026-11-06",
      do_end: null,
      lane: "foreground",
      parked: false,
    });
    expect(dropPlan({ kind: "column", column: rest, lane: "background" }, fri, rest)).toMatchObject(
      { do_start: "2026-11-06", do_end: null, lane: "background" },
    );
    // A running plan window in Today, a far-off day in Later, a deadline-only task: all kept.
    const running = task({ id: 2, do_start: "2026-10-31", do_end: "2026-11-06" });
    expect(
      dropPlan({ kind: "column", column: col("today"), lane: "foreground" }, running, col("today")),
    ).toMatchObject({ do_start: "2026-10-31", do_end: "2026-11-06" });
    const far = task({ id: 3, do_start: "2027-02-10" });
    expect(
      dropPlan({ kind: "column", column: col("later"), lane: "foreground" }, far, col("later")),
    ).toMatchObject({ do_start: "2027-02-10", do_end: null });
    const dueOnly = task({ id: 4, effective_deadline: "2026-11-13", deadline_from: "event" });
    expect(
      dropPlan({ kind: "column", column: col("next"), lane: "foreground" }, dueOnly, col("next")),
    ).toMatchObject({ do_start: null, do_end: null });
    // From another column it's a new plan, as before.
    expect(
      dropPlan({ kind: "column", column: rest, lane: "foreground" }, fri, col("today"))?.do_start,
    ).toBe("2026-11-06");
    expect(
      dropPlan({ kind: "column", column: col("next"), lane: "foreground" }, fri, rest)?.do_start,
    ).toBe("2026-11-09");
  });

  it("into a window that has begun, the plan runs over the window's own days", () => {
    const running = buildColumns(day("2026-11-12"), [win("trip", "2026-11-10", "2026-11-20")]);
    const trip = running.find((c) => c.id === "window:trip")!;
    const plan = dropPlan(
      { kind: "column", column: trip, lane: "foreground" },
      task({ id: 1, do_start: "2026-11-14" }),
    );
    expect(plan).toMatchObject({ do_start: "2026-11-10", do_end: "2026-11-20" });
    // And the next day it's still in the trip.
    const { model } = place([task({ id: 1, do_start: plan!.do_start, do_end: plan!.do_end })], {
      today: day("2026-11-13"),
      windows: [win("trip", "2026-11-10", "2026-11-20")],
    });
    expect(where(model)).toEqual({ "window:trip/foreground": "1" });
  });

  it("knows a plan that changes nothing", () => {
    const t = task({ id: 1, do_start: "2026-11-05" });
    expect(
      samePlan(t, { do_start: "2026-11-05", do_end: null, lane: "foreground", parked: false }),
    ).toBe(true);
    expect(
      samePlan(t, { do_start: "2026-11-05", do_end: null, lane: "background", parked: false }),
    ).toBe(false);
  });
});

describe("the order after a drop", () => {
  const { model } = place(
    [
      task({ id: 1, do_start: "2026-11-04" }),
      task({ id: 2, do_start: "2026-11-04", stack_id: 7 }),
      task({ id: 3, do_start: "2026-11-04", stack_id: 7 }),
      task({ id: 4, do_start: "2026-11-04" }),
    ],
    { stacks: [stack(7)], milestones: [milestone({ id: 1, due_date: "2026-11-04" })] },
  );
  const cards = model.lanes.today.foreground;

  it("lists a lane's tasks with stack members inline", () => {
    expect(laneTaskIds(cards)).toEqual([1, 2, 3, 4]);
  });

  it("puts what moved before the card it was dropped on, or at the end", () => {
    expect(orderAfterDrop(cards, [9], 0)).toEqual([9, 1, 2, 3, 4]);
    expect(orderAfterDrop(cards, [9], 1)).toEqual([1, 9, 2, 3, 4]);
    // Past the milestone, or past the end: last.
    expect(orderAfterDrop(cards, [9], 3)).toEqual([1, 2, 3, 4, 9]);
    expect(orderAfterDrop(cards, [9], 99)).toEqual([1, 2, 3, 4, 9]);
  });

  it("counts places without the card being moved, within its own lane", () => {
    // Task 1 dragged below the stack: it was card 0, so "before card 1" (task 4) is after the stack.
    expect(orderAfterDrop(cards, [1], 1)).toEqual([2, 3, 1, 4]);
    // The whole stack moved to the top.
    expect(orderAfterDrop(cards, [2, 3], 0)).toEqual([2, 3, 1, 4]);
  });

  it("moves a card one place up or down its lane, a stack as one, past no milestone", () => {
    expect(orderAfterStep(cards, [4], -1)).toEqual([1, 4, 2, 3]);
    expect(orderAfterStep(cards, [1], 1)).toEqual([2, 3, 1, 4]);
    expect(orderAfterStep(cards, [2, 3], -1)).toEqual([2, 3, 1, 4]);
    expect(orderAfterStep(cards, [2, 3], 1)).toEqual([1, 4, 2, 3]);
    // Already first, already last (the milestone after it doesn't count), or not in this lane.
    expect(orderAfterStep(cards, [1], -1)).toBeNull();
    expect(orderAfterStep(cards, [4], 1)).toBeNull();
    expect(orderAfterStep(cards, [99], 1)).toBeNull();
  });
});

describe("the events strip", () => {
  const ev = (id: string, start: string, over: Partial<CalendarEvent> = {}): CalendarEvent => ({
    id,
    calendar_id: "c",
    summary: id,
    description: null,
    location: null,
    start,
    end: null,
    all_day: !start.includes("T"),
    html_link: null,
    uid: id,
    ...over,
  });
  const strips = (events: CalendarEvent[], windows: RunwayWindow[] = []) => {
    const columns = buildColumns(WED, windows);
    const out = placeEvents(events, columns, WED);
    return Object.fromEntries(
      Object.entries(out)
        .filter(([, s]) => s.events.length > 0 || s.more > 0)
        .map(([id, s]) => [
          id,
          `${s.events.map((e) => e.id).join(" ")}${s.more ? ` +${s.more}` : ""}`,
        ]),
    );
  };

  it("puts each event and pinboard entry on its day's column, in day and time order", () => {
    expect(
      strips([
        ev("standup", "2026-11-04T09:00:00"),
        ev("allday", "2026-11-04"),
        ev("fri", "2026-11-06T14:00:00"),
        ev("mon", "2026-11-09"),
        ev("entry", "2026-11-05", { calendar_id: "pm:pinboard", uid: null }),
        ev("thu", "2026-11-05T08:00:00"),
        // A multi-day one still running shows on today; one over, and one before today, don't.
        ev("trip", "2026-11-02", { end: "2026-11-06" }),
        ev("gone", "2026-11-01", { end: "2026-11-03" }),
        ev("monday", "2026-11-02T10:00:00"),
      ]),
    ).toEqual({
      today: "trip allday standup",
      tomorrow: "entry thu",
      rest: "fri",
      next: "mon",
    });
  });

  it("leaves out the task and milestone overlays and the new event's ghost", () => {
    expect(
      strips([
        ev("task:1", "2026-11-04", { calendar_id: "pm:tasks", uid: null }),
        ev("milestone:1", "2026-11-04", { calendar_id: "pm:milestones", uid: null }),
        ev("draft:new", "2026-11-04T10:00:00", { uid: null }),
        ev("real", "2026-11-04T11:00:00"),
      ]),
    ).toEqual({ today: "real" });
  });

  it("fills a window's column, and lists Later's next ten, counting the rest", () => {
    const later = Array.from({ length: 13 }, (_, i) =>
      ev(`l${i + 1}`, `2026-12-${String(i + 1).padStart(2, "0")}`),
    );
    const out = strips(
      [ev("in-trip", "2026-11-11"), ...later],
      [win("trip", "2026-11-10", "2026-11-12")],
    );
    expect(out["window:trip"]).toBe("in-trip");
    expect(out.later).toBe(
      `${later
        .slice(0, 10)
        .map((e) => e.id)
        .join(" ")} +3`,
    );
    expect(LATER_EVENTS).toBe(10);
  });

  it("has a strip for every column but Before today", () => {
    const columns = buildColumns(WED, []);
    const out = placeEvents([], columns, WED);
    expect(Object.keys(out)).toEqual(columns.filter((c) => c.kind !== "past").map((c) => c.id));
  });
});

describe("a task's linked event", () => {
  const ev = (id: string, start: string, uid = "u"): CalendarEvent => ({
    id,
    calendar_id: "c",
    summary: id,
    description: null,
    location: null,
    start,
    end: null,
    all_day: !start.includes("T"),
    html_link: null,
    uid,
  });

  it("is the next occurrence, else the latest past one", () => {
    const events = [
      ev("a", "2026-10-28T10:00:00Z"),
      ev("b", "2026-11-11T10:00:00Z"),
      ev("c", "2026-11-06"),
      ev("d", "2026-11-01T10:00:00Z"),
      ev("other", "2026-11-05", "x"),
    ];
    expect(linkedEvent(events, "u", WED)?.id).toBe("c");
    expect(linkedEvent(events.slice(0, 1).concat(events[3]), "u", WED)?.id).toBe("d");
    expect(linkedEvent(events, "nope", WED)).toBeNull();
    expect(linkedEvent(events, null, WED)).toBeNull();
  });
});

describe("the tasks overlay", () => {
  it("draws open tasks on their plan window, else on their deadline as due", () => {
    const evs = taskOverlayEvents(
      [
        task({ id: 1, title: "Draft", do_start: "2026-11-09", do_end: "2026-11-11" }),
        task({ id: 2, title: "Send", effective_deadline: "2026-11-13" }),
        task({ id: 3, title: "Someday" }),
        task({ id: 4, title: "Done", status: "done", do_start: "2026-11-09" }),
        task({ id: 5, title: "Parked", parked: true, do_start: "2026-11-09" }),
        task({ id: 6, title: "Old", project_archived: true, do_start: "2026-11-09" }),
        task({ id: 7, title: "Gone", project_name: "Gone", do_start: "2026-11-09" }),
        task({ id: 8, title: "One day", do_start: "2026-11-10", effective_deadline: "2026-11-12" }),
      ],
      new Set(["Gone"]),
    );
    expect(evs.map((e) => [e.id, e.calendar_id, e.summary, e.start, e.end, e.all_day])).toEqual([
      // The window's all-day end is exclusive: to Wednesday ends on Thursday.
      ["task:1", TASK_CALENDAR_ID, "Draft", "2026-11-09", "2026-11-12", true],
      ["task:2", TASK_CALENDAR_ID, "Due: Send", "2026-11-13", null, true],
      ["task:8", TASK_CALENDAR_ID, "One day", "2026-11-10", null, true],
    ]);
    expect(evs.every((e) => e.uid === null)).toBe(true);
  });
});
