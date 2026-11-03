// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The hooks and words both Runways share (PA-2): the pixel board (`RunwayBoard`, Slate and
// Editorial) and the mono one (`TerminalRunway`) lay out differently but read the same model and
// make the same writes. The pieces they draw alike are in `runwayParts.tsx`.
//
// Every write goes through the task IPC (`planTask`, `reorderTasks`, `changeTaskStatus`), never a
// calendar write wrapper: the Runway changes tasks, and a calendar event only ever opens its
// details. The backend fires `tasks://changed` after each write, which CalendarView listens to, so
// nothing here keeps a copy of a task.

import { useCallback, useMemo, useState } from "react";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { dayKey, parseLocal, startOfDay } from "../../../lib/calendar-layout";
import { changeTaskStatus, planTask, reorderTasks } from "../../../lib/ipc";
import { formatClockIso, formatDateLocal, formatDateOnly } from "../../../lib/format";
import {
  buildColumns,
  placeCards,
  samePlan,
  type RunwayColumn,
  type RunwayModel,
  type TaskPlan,
} from "../../../lib/runway";
import { runMutation } from "../../../lib/runMutation";
import type {
  CalendarEvent,
  Milestone,
  Task,
  TaskInput,
  TaskLane,
  TaskStack,
  TaskStatus,
} from "../../../lib/types";

/** What both Runways are given. */
export interface RunwayProps {
  tasks: Task[];
  stacks: TaskStack[];
  milestones: Milestone[];
  /** The synced events a task's link can point at (the overlays left out). */
  events: CalendarEvent[];
  /** Archived project names: their tasks and milestones stay off the board. */
  archived: ReadonlySet<string>;
  windows: RunwayWindow[];
  now: Date;
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onOpenProject?: (project: string) => void;
  /** A linked event's details, anchored at what was clicked. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  /** The tab's polite live region. */
  announce: (text: string) => void;
}

/** The columns and where everything sits, for `now`'s day. */
export function useRunwayModel({
  tasks,
  stacks,
  milestones,
  archived,
  windows,
  now,
}: Pick<RunwayProps, "tasks" | "stacks" | "milestones" | "archived" | "windows" | "now">): {
  today: Date;
  columns: RunwayColumn[];
  model: RunwayModel;
} {
  // Keyed on the day, not the minute tick, so the model is rebuilt only when the date turns.
  const todayKey = dayKey(startOfDay(now));
  const today = useMemo(() => parseLocal(todayKey, true) ?? startOfDay(new Date()), [todayKey]);
  const columns = useMemo(() => buildColumns(today, windows), [today, windows]);
  const model = useMemo(
    () => placeCards({ tasks, stacks, milestones, columns, today, archived }),
    [tasks, stacks, milestones, columns, today, archived],
  );
  return { today, columns, model };
}

/** "Tomorrow", or "Tomorrow, background" for the background lane. */
export function placeName(column: RunwayColumn, lane: TaskLane): string {
  return lane === "background" ? `${column.label}, background` : column.label;
}

/** The writes a Runway makes, with their error and their announcement. */
export function useRunwayActions(announce: (text: string) => void) {
  const [error, setError] = useState<string | null>(null);

  /** Tick a task on to its next status. */
  const cycle = useCallback(
    (task: Task, next: TaskStatus) =>
      void runMutation(() => changeTaskStatus(task.id, next), setError),
    [],
  );

  /** Re-plan `tasks` (each by `planFor`), then, when given, put a lane in `order`; then say where
   *  they went. A plan that changes nothing isn't sent. */
  const replan = useCallback(
    (
      tasks: readonly Task[],
      planFor: (t: Task) => TaskPlan | null,
      where: string,
      order?: number[],
    ) =>
      void runMutation(async () => {
        let moved = false;
        for (const t of tasks) {
          const plan = planFor(t);
          if (!plan) continue;
          moved = true;
          if (samePlan(t, plan)) continue;
          await planTask(t.id, plan.do_start, plan.do_end, plan.lane, plan.parked);
        }
        if (order && order.length > 1) await reorderTasks(order);
        if (moved) announce(`Moved to ${where}`);
      }, setError),
    [announce],
  );

  return { error, setError, cycle, replan };
}

/** A linked event's day and time, for a card: "06-11 14:00", or the day alone for an all-day one. */
export function eventWhen(ev: CalendarEvent): string {
  const at = parseLocal(ev.start, ev.all_day);
  if (!at) return "";
  return ev.all_day ? formatDateLocal(at) : `${formatDateLocal(at)} ${formatClockIso(ev.start)}`;
}

/** "due 06-11 · Friday review", for a deadline that follows a linked milestone or event. Null for
 *  a task's own date, or a link that didn't resolve (the deadline chip says that). */
export function linkedDue(t: Task): string | null {
  if (t.deadline_from === "date" || t.deadline_missing || !t.effective_deadline) return null;
  const what = t.deadline_from === "event" ? t.event_title : t.milestone_label;
  const when = `due ${formatDateOnly(t.effective_deadline)}`;
  return what ? `${when} · ${what}` : when;
}

/** "planned 2d ago", for a task rolled forward into today. */
export function rolledWords(daysAgo: number): string {
  return daysAgo === 1 ? "planned yesterday" : `planned ${daysAgo}d ago`;
}

/** A new task's starting values on a Runway: it starts today. */
export function newTaskDefaults(today: Date): Partial<TaskInput> {
  return { doStart: dayKey(today) };
}

/** A new window's id: unique enough within one person's handful of windows. */
export function newWindowId(): string {
  return `w${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
}
