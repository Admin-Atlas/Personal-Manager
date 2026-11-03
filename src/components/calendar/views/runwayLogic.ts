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
  placeEvents,
  samePlan,
  type RunwayColumn,
  type RunwayEventStrip,
  type RunwayModel,
  type RunwayTaskCard,
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
  /** What the calendar shows, hidden calendars left out, less the task and milestone overlays
   *  (those are the board's own cards): the synced events and the pinboard's timeline entries.
   *  The columns' events strips, and what a task's link can point at. */
  events: CalendarEvent[];
  /** A calendar's colour, for the dot beside an event in a strip. */
  colorOf?: (calendarId: string) => string;
  /** Switch the Agenda to its List, where Later's events past the first few are. */
  onShowList?: () => void;
  /** Archived project names: their tasks and milestones stay off the board. */
  archived: ReadonlySet<string>;
  windows: RunwayWindow[];
  now: Date;
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onOpenProject?: (project: string) => void;
  /** An event's details (a pinboard entry opens the Pinboard), anchored at what was clicked. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  /** The tab's polite live region. */
  announce: (text: string) => void;
}

/** The columns, where everything sits and each column's events, for `now`'s day. */
export function useRunwayModel({
  tasks,
  stacks,
  milestones,
  events,
  archived,
  windows,
  now,
}: Pick<
  RunwayProps,
  "tasks" | "stacks" | "milestones" | "events" | "archived" | "windows" | "now"
>): {
  today: Date;
  columns: RunwayColumn[];
  model: RunwayModel;
  strips: Record<string, RunwayEventStrip>;
} {
  // Keyed on the day, not the minute tick, so the model is rebuilt only when the date turns.
  const todayKey = dayKey(startOfDay(now));
  const today = useMemo(() => parseLocal(todayKey, true) ?? startOfDay(new Date()), [todayKey]);
  const columns = useMemo(() => buildColumns(today, windows), [today, windows]);
  const model = useMemo(
    () => placeCards({ tasks, stacks, milestones, columns, today, archived }),
    [tasks, stacks, milestones, columns, today, archived],
  );
  const strips = useMemo(() => placeEvents(events, columns, today), [events, columns, today]);
  return { today, columns, model, strips };
}

/** Where a column's events strip says an event is: its clock time in a one-day column (or
 *  "all-day"), else its day too, as a linked event's chip says it. */
export function stripWhen(ev: CalendarEvent, column: RunwayColumn): string {
  const at = parseLocal(ev.start, ev.all_day);
  // A one-day column whose day the event starts on; one still running from before says its day.
  if (at && column.start !== null && column.start === column.end) {
    if (dayKey(startOfDay(at)) === column.start) {
      return ev.all_day ? "all-day" : formatClockIso(ev.start);
    }
  }
  return eventWhen(ev);
}

/** Where a stack's task is planned, as its opened card says it: the column's name, or "Parked". */
export function memberPlace(columns: readonly RunwayColumn[], columnId: string | null): string {
  if (columnId === null) return "Parked";
  return columns.find((c) => c.id === columnId)?.label ?? "";
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

  /** Put a lane in `order` (the Move… menu's Move up / Move down), then say so. */
  const reorder = useCallback(
    (order: number[], said: string) =>
      void runMutation(async () => {
        await reorderTasks(order);
        announce(said);
      }, setError),
    [announce],
  );

  return { error, setError, cycle, replan, reorder };
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

/** "planned 2d ago", for a task whose plan rolled forward into today. Null for one that rolled
 *  because its deadline passed with no plan: it was never planned, and its deadline chip already
 *  says how late it is. */
export function rolledWords(card: RunwayTaskCard): string | null {
  if (!card.rolled || card.rolledBy !== "plan") return null;
  return card.daysAgo === 1 ? "planned yesterday" : `planned ${card.daysAgo}d ago`;
}

/** A new task's starting values on a Runway: it starts today. */
export function newTaskDefaults(today: Date): Partial<TaskInput> {
  return { doStart: dayKey(today) };
}

/** A new window's id: unique enough within one person's handful of windows. */
export function newWindowId(): string {
  return `w${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
}
