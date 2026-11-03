// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure half of the Runway (PA-2): the Agenda's board of tasks in columns running forward from
// today. Three things, all free of React and the DOM so the fiddly cases (week edges, windows cut out
// of the default columns, what rolls forward and what drops away) are pinned by node tests:
//
//   - `buildColumns`: the columns for a day. Before today, Today, Tomorrow, the rest of this week,
//     next week and Later, with the user's named windows (a trip, a sprint) laid over them as
//     columns of their own, their days cut out of the defaults.
//   - `placeCards`: where every task and milestone sits. A task's day is its plan start, else its
//     effective deadline (its own date, or the linked milestone's or event's); an open task whose
//     day has passed rolls forward into today; done ones drop back into "Before today" for 14 days.
//   - `dropPlan`: what a drop (or a pick in the Move… menu) writes: a new plan window, lane and
//     parked flag, through `planTask` alone.
//
// Days are local `YYYY-MM-DD` keys, compared as strings, as the rest of the Calendar tab buckets
// them (`dayKey(startOfDay(now))`). A milestone's `due_date` arrives already resolved in the user's
// zone. Display logic reads `effective_deadline`, never a task's stored `deadline`.

import type { RunwayWindow } from "./calendarPrefs";
import {
  addDays,
  dayDiff,
  dayKey,
  parseLocal,
  startOfDay,
  startOfWeek,
  TASK_CALENDAR_ID,
  weekdayShort,
} from "./calendar-layout";
import { formatDateLocal, pad2 } from "./format";
import { compareTasks, deadlineInfo, isOpen, type DeadlineTone } from "./tasks";
import type { CalendarEvent, Milestone, Task, TaskLane, TaskStack } from "./types";

// --- columns ---------------------------------------------------------------------------------------

export type RunwayColumnKind = "past" | "default" | "window";

export interface RunwayColumn {
  /** Stable for the day: `past`, `today`, `tomorrow`, `rest`, `next`, `later` (a default split by a
   *  window keeps its id for the first piece, and `<id>:2`… for the rest), or `window:<id>`. */
  id: string;
  label: string;
  /** The column's days in words: "Wed 05 – Sun 09", "from 16-11". */
  sub: string;
  /** First day, `YYYY-MM-DD`; null only for "Before today". */
  start: string | null;
  /** Last day, `YYYY-MM-DD`; null for an open end (Later). */
  end: string | null;
  kind: RunwayColumnKind;
  /** This column holds today. Exactly one column does. */
  today: boolean;
  /** For a window column, the window it shows. */
  windowId?: string;
}

/** The "Before today" column's id. */
export const PAST_COLUMN_ID = "past";
/** The column undated tasks go to. */
export const LATER_COLUMN_ID = "later";
/** How far back "Before today" keeps done things. */
export const PAST_DAYS = 14;

const ISO_DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The `YYYY-MM-DD` part of a stored date, or null when it isn't one. */
function dayOf(value: string | null | undefined): string | null {
  if (!value) return null;
  const d = value.slice(0, 10);
  return ISO_DAY.test(d) ? d : null;
}

/** A day near today reads by its weekday ("Wed 05"); one further out by its date ("16-11"). */
function dayWords(d: Date, today: Date): string {
  const n = dayDiff(today, d);
  return n >= 0 && n <= 13 ? `${weekdayShort(d)} ${pad2(d.getDate())}` : formatDateLocal(d);
}

function spanWords(start: Date, end: Date | null, today: Date): string {
  if (end === null) return `from ${formatDateLocal(start)}`;
  if (dayKey(start) === dayKey(end)) return dayWords(start, today);
  return `${dayWords(start, today)} – ${dayWords(end, today)}`;
}

/** A stretch of whole days counted from today (0 = today); `end` Infinity for an open end. */
interface Span {
  start: number;
  end: number;
}

/** `pieces` with `cut` taken out of each. */
function subtract(pieces: Span[], cut: Span): Span[] {
  const out: Span[] = [];
  for (const p of pieces) {
    if (cut.end < p.start || cut.start > p.end) {
      out.push(p);
      continue;
    }
    if (p.start < cut.start) out.push({ start: p.start, end: cut.start - 1 });
    if (cut.end < p.end) out.push({ start: cut.end + 1, end: p.end });
  }
  return out;
}

/** The Runway's columns for `today`, with `windows` laid over the defaults.
 *
 *  Before today; Today; Tomorrow; the rest of this week (from the day after tomorrow to Sunday,
 *  left out on Saturday and Sunday, when there is none); next week (Monday to Sunday); Later (from
 *  the Monday after, open-ended). The defaults never overlap: on a Sunday, tomorrow is next week's
 *  Monday, so "Next week" starts the day after.
 *
 *  A window that has ended is left out. One still running starts at today (its earlier days are
 *  past). Each becomes a column of its own and its days are cut out of the defaults; a default left
 *  with no days goes, and one cut in two keeps its label on both pieces. Windows may overlap each
 *  other: both are shown, and a day in both belongs to the shorter one (`columnForDay`). Columns are
 *  ordered by their first day. */
export function buildColumns(today: Date, windows: readonly RunwayWindow[]): RunwayColumn[] {
  const t = startOfDay(today);
  const monday = startOfWeek(t);
  const toIndex = (d: Date) => dayDiff(t, d);
  const toDay = (i: number) => addDays(t, i);

  const defaults: { id: string; label: string; start: number; end: number }[] = [
    { id: "today", label: "Today", start: 0, end: 0 },
    { id: "tomorrow", label: "Tomorrow", start: 1, end: 1 },
    { id: "rest", label: "Rest of this week", start: 2, end: toIndex(addDays(monday, 6)) },
    {
      id: "next",
      label: "Next week",
      start: toIndex(addDays(monday, 7)),
      end: toIndex(addDays(monday, 13)),
    },
    { id: LATER_COLUMN_ID, label: "Later", start: toIndex(addDays(monday, 14)), end: Infinity },
  ];

  // Windows still running or to come, from today at the earliest.
  const live: (Span & { w: RunwayWindow })[] = [];
  for (const w of windows) {
    if (!ISO_DAY.test(w.start) || !ISO_DAY.test(w.end) || w.end < w.start) continue;
    const s = parseLocal(w.start, true);
    const e = parseLocal(w.end, true);
    if (!s || !e) continue;
    const end = toIndex(e);
    if (end < 0) continue;
    live.push({ start: Math.max(0, toIndex(s)), end, w });
  }

  const columns: (RunwayColumn & { order: number; span: number })[] = [];
  let floor = 0;
  for (const d of defaults) {
    const start = Math.max(d.start, floor);
    if (start > d.end) continue;
    floor = d.end + 1;
    let pieces: Span[] = [{ start, end: d.end }];
    for (const l of live) pieces = subtract(pieces, l);
    pieces.forEach((p, i) => {
      const open = p.end === Infinity;
      columns.push({
        id: i === 0 ? d.id : `${d.id}:${i + 1}`,
        label: d.label,
        sub: spanWords(toDay(p.start), open ? null : toDay(p.end), t),
        start: dayKey(toDay(p.start)),
        end: open ? null : dayKey(toDay(p.end)),
        kind: "default",
        today: false,
        order: p.start,
        span: open ? Infinity : p.end - p.start,
      });
    });
  }
  for (const l of live) {
    columns.push({
      id: `window:${l.w.id}`,
      label: l.w.label,
      sub: spanWords(toDay(l.start), toDay(l.end), t),
      start: dayKey(toDay(l.start)),
      end: dayKey(toDay(l.end)),
      kind: "window",
      today: false,
      windowId: l.w.id,
      order: l.start,
      span: l.end - l.start,
    });
  }
  columns.sort((a, b) => a.order - b.order || a.span - b.span || a.label.localeCompare(b.label));

  const past: RunwayColumn = {
    id: PAST_COLUMN_ID,
    label: "Before today",
    sub: `last ${PAST_DAYS} days`,
    start: null,
    end: dayKey(addDays(t, -1)),
    kind: "past",
    today: false,
  };
  const out: RunwayColumn[] = [past];
  for (const c of columns) {
    const col: RunwayColumn = {
      id: c.id,
      label: c.label,
      sub: c.sub,
      start: c.start,
      end: c.end,
      kind: c.kind,
      today: false,
    };
    if (c.windowId !== undefined) col.windowId = c.windowId;
    out.push(col);
  }
  const holder = columnForDay(out, dayKey(t));
  if (holder) holder.today = true;
  return out;
}

/** The column a day belongs to: "Before today" for a past day; otherwise a window that holds it
 *  (the shortest, when windows overlap, then the earliest), else the default that does. */
export function columnForDay(columns: readonly RunwayColumn[], day: string): RunwayColumn | null {
  const past = columns.find((c) => c.kind === "past");
  if (past && past.end !== null && day <= past.end) return past;
  const holds = (c: RunwayColumn) =>
    c.start !== null && c.start <= day && (c.end === null || day <= c.end);
  let best: RunwayColumn | null = null;
  let bestSpan = Infinity;
  for (const c of columns) {
    if (c.kind !== "window" || !holds(c)) continue;
    const span = dayDiff(parseLocal(c.start!, true)!, parseLocal(c.end!, true)!);
    if (span < bestSpan) {
      best = c;
      bestSpan = span;
    }
  }
  return best ?? columns.find((c) => c.kind === "default" && holds(c)) ?? null;
}

/** Whether a task can be dropped into (or moved to) `column`. "Before today" takes nothing. */
export function isDropColumn(column: RunwayColumn): boolean {
  return column.kind !== "past";
}

// --- cards -----------------------------------------------------------------------------------------

export interface RunwayTaskCard {
  kind: "task";
  task: Task;
  /** Open, and its day has passed: rolled forward into today. */
  rolled: boolean;
  /** How many days ago its day was, when rolled (else 0). */
  daysAgo: number;
}

export interface RunwayMilestoneCard {
  kind: "milestone";
  milestone: Milestone;
  /** Unmet and its day has passed: shown in today, as late. */
  late: boolean;
}

export interface RunwayStackCard {
  kind: "stack";
  stack: TaskStack;
  /** Its tasks in this column and lane, in order. */
  tasks: Task[];
  /** The same, as cards (a member keeps its "rolled" flag). */
  members: RunwayTaskCard[];
  /** How many of them are still open. */
  left: number;
}

export type RunwayCard = RunwayTaskCard | RunwayMilestoneCard | RunwayStackCard;

export interface RunwayLanes {
  foreground: RunwayCard[];
  background: RunwayCard[];
}

export interface RunwayProgress {
  done: number;
  total: number;
  doing: number;
}

export interface RunwayNextUp {
  title: string;
  deadlineText: string;
  tone: DeadlineTone;
}

export interface RunwayModel {
  /** Each column's two lanes, by column id. Every column has an entry. */
  lanes: Record<string, RunwayLanes>;
  /** Parked open tasks, in order. */
  parked: RunwayTaskCard[];
  /** Over the tasks and milestones from today on (not parked, not "Before today"). */
  progress: RunwayProgress;
  /** The open task or milestone with the earliest deadline. */
  nextUp: RunwayNextUp | null;
}

export interface PlaceInput {
  tasks: readonly Task[];
  stacks: readonly TaskStack[];
  milestones: readonly Milestone[];
  columns: readonly RunwayColumn[];
  today: Date;
  /** Archived project names: their tasks and milestones are left off. */
  archived: ReadonlySet<string>;
}

/** The task cards in a lane's display order, stack members inline: what a reorder works over. */
export function laneTaskIds(cards: readonly RunwayCard[]): number[] {
  const ids: number[] = [];
  for (const c of cards) {
    if (c.kind === "task") ids.push(c.task.id);
    else if (c.kind === "stack") ids.push(...c.tasks.map((t) => t.id));
  }
  return ids;
}

/** A milestone that's reached (met, or its status done). */
function milestoneDone(m: Milestone): boolean {
  return m.state === "met" || m.status === "done";
}

/** Whether a task belongs to an archived project. */
function archivedTask(t: Task, archived: ReadonlySet<string>): boolean {
  return t.project_archived || (t.project_name !== null && archived.has(t.project_name));
}

/** The day a task sits on: its plan start, else its effective deadline. */
export function planDay(t: Task): string | null {
  return dayOf(t.do_start) ?? dayOf(t.effective_deadline);
}

/** Where every task and milestone sits on the Runway.
 *
 *  - Archived projects' tasks and milestones are left off; parked open tasks go to `parked`.
 *  - A task sits on its plan start, else its effective deadline; with neither, in Later. A plan
 *    window that matches a window column exactly sits in that window.
 *  - An open task whose day has passed rolls into today's column (`rolled`), unless its plan window
 *    still runs through today, when it simply sits in today's column.
 *  - A done task whose day has passed (or, undated, that was finished before today) goes to "Before
 *    today", for 14 days by its finished day or its day; older ones are left off.
 *  - Milestones with a date: an unmet one past its day sits in today's column as late; a met one
 *    past its day goes to "Before today" for 14 days. Always in the foreground lane.
 *  - In a lane, tasks run in the global order and milestones follow by date. Two or more tasks of
 *    one stack in the same column and lane are one stack card, where the first of them is. */
export function placeCards(input: PlaceInput): RunwayModel {
  const { tasks, stacks, milestones, columns, archived } = input;
  const t0 = startOfDay(input.today);
  const tk = dayKey(t0);
  const floor = dayKey(addDays(t0, -PAST_DAYS));
  const byId = new Map(columns.map((c) => [c.id, c]));
  const todayCol = columns.find((c) => c.today) ?? columnForDay(columns, tk);
  const later = byId.get(LATER_COLUMN_ID) ?? todayCol;
  const past = columns.find((c) => c.kind === "past") ?? null;
  const stackById = new Map(stacks.map((s) => [s.id, s]));

  type Placed = { card: RunwayTaskCard | RunwayMilestoneCard; lane: TaskLane };
  const at = new Map<string, Placed[]>();
  const put = (col: RunwayColumn | null | undefined, p: Placed) => {
    if (!col) return;
    const list = at.get(col.id);
    if (list) list.push(p);
    else at.set(col.id, [p]);
  };

  const parked: RunwayTaskCard[] = [];
  const progress: RunwayProgress = { done: 0, total: 0, doing: 0 };
  let next: { day: string; rank: number; title: string; kind: "hard" | "soft" } | null = null;
  const consider = (day: string | null, rank: number, title: string, kind: "hard" | "soft") => {
    if (!day) return;
    if (!next || day < next.day || (day === next.day && rank < next.rank)) {
      next = { day, rank, title, kind };
    }
  };

  const windowMatch = (t: Task): RunwayColumn | undefined =>
    t.do_start && t.do_end
      ? columns.find((c) => c.kind === "window" && c.start === t.do_start && c.end === t.do_end)
      : undefined;
  const sortedTasks = [...tasks].sort(compareTasks);
  sortedTasks.forEach((t, rank) => {
    if (archivedTask(t, archived)) return;
    const open = isOpen(t);
    if (open && t.parked) {
      parked.push({ kind: "task", task: t, rolled: false, daysAgo: 0 });
      return;
    }
    const day = planDay(t);
    const lane = t.lane;
    const card = (rolled: boolean, daysAgo = 0): Placed => ({
      card: { kind: "task", task: t, rolled, daysAgo },
      lane,
    });
    let col: RunwayColumn | null | undefined;
    let placed: Placed;
    if (day === null) {
      const finished = open ? null : dayOf(completedDay(t.completed_at));
      if (finished !== null && finished < tk) {
        if (finished < floor) return;
        col = past;
      } else {
        col = later;
      }
      placed = card(false);
    } else if (day < tk) {
      const runsOn = dayOf(t.do_end) !== null && dayOf(t.do_end)! >= tk && t.do_start !== null;
      if (runsOn) {
        col = windowMatch(t) ?? todayCol;
        placed = card(false);
      } else if (open) {
        col = todayCol;
        placed = card(true, dayDiff(parseLocal(day, true)!, t0));
      } else {
        const finished = dayOf(completedDay(t.completed_at));
        if (day < floor && (finished === null || finished < floor)) return;
        col = past;
        placed = card(false);
      }
    } else {
      col = windowMatch(t) ?? columnForDay(columns, day);
      placed = card(false);
    }
    put(col, placed);
    if (col && col.kind !== "past") {
      progress.total += 1;
      if (!open) progress.done += 1;
      else if (t.status === "doing") progress.doing += 1;
      if (open) consider(dayOf(t.effective_deadline), rank, t.title, t.deadline_kind);
    }
  });

  const mRank = sortedTasks.length;
  milestones.forEach((m, i) => {
    if (archived.has(m.project_name)) return;
    const due = dayOf(m.due_date);
    if (!due) return;
    const done = milestoneDone(m);
    let col: RunwayColumn | null | undefined;
    let late = false;
    if (due < tk) {
      if (done) {
        if (due < floor) return;
        col = past;
      } else {
        col = todayCol;
        late = true;
      }
    } else {
      col = columnForDay(columns, due);
    }
    put(col, { card: { kind: "milestone", milestone: m, late }, lane: "foreground" });
    if (col && col.kind !== "past") {
      progress.total += 1;
      if (done) progress.done += 1;
      else consider(due, mRank + i, m.label, "hard");
    }
  });

  const lanes: Record<string, RunwayLanes> = {};
  for (const c of columns) {
    const placed = at.get(c.id) ?? [];
    lanes[c.id] = {
      foreground: arrange(
        placed.filter((p) => p.lane === "foreground").map((p) => p.card),
        stackById,
      ),
      background: arrange(
        placed.filter((p) => p.lane === "background").map((p) => p.card),
        stackById,
      ),
    };
  }

  const n = next as { day: string; kind: "hard" | "soft"; title: string } | null;
  const info = n ? deadlineInfo(n.day, n.kind, t0) : null;
  return {
    lanes,
    parked,
    progress,
    nextUp: n && info ? { title: n.title, deadlineText: info.text, tone: info.tone } : null,
  };
}

/** The local day a `completed_at` instant fell on, `YYYY-MM-DD`, or null. */
function completedDay(at: string | null): string | null {
  if (!at) return null;
  const d = new Date(at);
  return Number.isNaN(d.getTime()) ? null : dayKey(startOfDay(d));
}

/** One lane's cards: tasks in the global order (stacks of two or more folded into one card where
 *  the first of them is), then milestones by date. */
function arrange(
  cards: (RunwayTaskCard | RunwayMilestoneCard)[],
  stackById: Map<number, TaskStack>,
): RunwayCard[] {
  const taskCards = cards
    .filter((c): c is RunwayTaskCard => c.kind === "task")
    .sort((a, b) => compareTasks(a.task, b.task));
  const ms = cards
    .filter((c): c is RunwayMilestoneCard => c.kind === "milestone")
    .sort(
      (a, b) =>
        (a.milestone.due_date ?? "").localeCompare(b.milestone.due_date ?? "") ||
        a.milestone.sort_order - b.milestone.sort_order ||
        a.milestone.id - b.milestone.id,
    );
  const members = new Map<number, RunwayTaskCard[]>();
  for (const c of taskCards) {
    const sid = c.task.stack_id;
    if (sid === null || !stackById.has(sid)) continue;
    const list = members.get(sid);
    if (list) list.push(c);
    else members.set(sid, [c]);
  }
  const out: RunwayCard[] = [];
  const emitted = new Set<number>();
  for (const c of taskCards) {
    const sid = c.task.stack_id;
    const group = sid === null ? undefined : members.get(sid);
    if (sid === null || !group || group.length < 2) {
      out.push(c);
      continue;
    }
    if (emitted.has(sid)) continue;
    emitted.add(sid);
    out.push({
      kind: "stack",
      stack: stackById.get(sid)!,
      tasks: group.map((g) => g.task),
      members: group,
      left: group.filter((g) => isOpen(g.task)).length,
    });
  }
  return [...out, ...ms];
}

// --- drops -----------------------------------------------------------------------------------------

/** Where a task is dropped: a column's lane, or the parked area. */
export type RunwayDropTarget =
  { kind: "column"; column: RunwayColumn; lane: TaskLane } | { kind: "parked" };

/** What `planTask` writes. */
export interface TaskPlan {
  do_start: string | null;
  do_end: string | null;
  lane: TaskLane;
  parked: boolean;
}

/** The plan a drop writes, or null where nothing can be dropped ("Before today"; milestones are
 *  never dragged).
 *
 *  Into a column: the plan starts on its first day (Later: its first day too) and, for a window,
 *  runs to its last; a default column leaves no end. The lane is the target's, and a parked task
 *  comes off the shelf. Onto the parked area: the dates and lane stay, and it's parked. */
export function dropPlan(target: RunwayDropTarget, task: Task): TaskPlan | null {
  if (target.kind === "parked") {
    return { do_start: task.do_start, do_end: task.do_end, lane: task.lane, parked: true };
  }
  const { column, lane } = target;
  if (!isDropColumn(column) || column.start === null) return null;
  return {
    do_start: column.start,
    do_end: column.kind === "window" ? column.end : null,
    lane,
    parked: false,
  };
}

/** A plan that changes nothing about `task`. */
export function samePlan(task: Task, plan: TaskPlan): boolean {
  return (
    task.do_start === plan.do_start &&
    task.do_end === plan.do_end &&
    task.lane === plan.lane &&
    task.parked === plan.parked
  );
}

/** A lane's task order after `moved` lands before the card at `cardIndex` (its index among the
 *  lane's cards as drawn, counted without the moved ones; past the end appends). What
 *  `reorderTasks` is given. */
export function orderAfterDrop(
  cards: readonly RunwayCard[],
  moved: readonly number[],
  cardIndex: number,
): number[] {
  const movedSet = new Set(moved);
  const before: number[] = [];
  const after: number[] = [];
  let i = 0;
  for (const c of cards) {
    const ids =
      c.kind === "task" ? [c.task.id] : c.kind === "stack" ? c.tasks.map((t) => t.id) : [];
    const rest = ids.filter((id) => !movedSet.has(id));
    // A card made only of moved tasks isn't counted: it is where the drag came from.
    if (ids.length > 0 && rest.length === 0) continue;
    (i < cardIndex ? before : after).push(...rest);
    i += 1;
  }
  return [...before, ...moved, ...after];
}

// --- a task's linked event -------------------------------------------------------------------------

/** The occurrence of the event a task links to that matters: the first on or after today, else the
 *  latest before it. Null when no loaded event has that UID. */
export function linkedEvent(
  events: readonly CalendarEvent[],
  uid: string | null,
  today: Date,
): CalendarEvent | null {
  if (!uid) return null;
  const t0 = startOfDay(today).getTime();
  let next: { ev: CalendarEvent; at: number } | null = null;
  let last: { ev: CalendarEvent; at: number } | null = null;
  for (const ev of events) {
    if (ev.uid !== uid) continue;
    const start = parseLocal(ev.start, ev.all_day);
    if (!start) continue;
    const at = start.getTime();
    if (startOfDay(start).getTime() >= t0) {
      if (!next || at < next.at) next = { ev, at };
    } else if (!last || at > last.at) {
      last = { ev, at };
    }
  }
  return next?.ev ?? last?.ev ?? null;
}

/** The task overlay on the calendar's grids and list (PA-1): one all-day event per open, unparked
 *  task of a live project, on its plan window, or with none on its effective deadline ("Due: …");
 *  a task with neither isn't drawn. Ids are `task:<id>`, on the `pm:tasks` pseudo-calendar. */
export function taskOverlayEvents(
  tasks: readonly Task[],
  archived: ReadonlySet<string>,
): CalendarEvent[] {
  const out: CalendarEvent[] = [];
  for (const t of [...tasks].sort(compareTasks)) {
    if (!isOpen(t) || t.parked || archivedTask(t, archived)) continue;
    const start = dayOf(t.do_start);
    const end = dayOf(t.do_end);
    const due = dayOf(t.effective_deadline);
    let first: string;
    let last: string | null = null;
    let summary = t.title;
    if (start) {
      first = start;
      if (end && end > start) last = end;
    } else if (due) {
      first = due;
      summary = `Due: ${t.title}`;
    } else {
      continue;
    }
    const lastDay = last ? parseLocal(last, true) : null;
    out.push({
      id: `task:${t.id}`,
      calendar_id: TASK_CALENDAR_ID,
      summary,
      description: null,
      location: null,
      start: first,
      // An all-day end is exclusive: a window to Friday ends on Saturday.
      end: lastDay ? dayKey(addDays(lastDay, 1)) : null,
      all_day: true,
      html_link: null,
      uid: null,
    });
  }
  return out;
}
