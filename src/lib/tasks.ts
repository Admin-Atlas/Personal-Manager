// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure half of the shared task UI (PA-1): status cycling, deadline wording and tone, sorting,
// and the editor's input shape. Every surface that shows a task (the project page, the pinboard
// Task list, the Runway, the calendar overlay) reads these, so a task looks and sorts the same
// everywhere. Display logic reads `effective_deadline` (the task's own date, or the linked
// milestone's or event's), never the stored `deadline`.
//
// No wall clock in here: every function that cares about "today" takes it as an argument, so the
// tests pin a day and the components pass `useNowTick()`.

import { addDays, dayDiff, dayKey, parseLocal, startOfDay } from "./calendar-layout";
import { formatDateLocal } from "./format";
import type {
  CalendarEvent,
  DeadlineFrom,
  DeadlineKind,
  Task,
  TaskInput,
  TaskStackKind,
  TaskStatus,
} from "./types";

/** The status cycle the tick button walks: todo → doing → done → todo. */
export function nextStatus(s: TaskStatus): TaskStatus {
  return s === "todo" ? "doing" : s === "doing" ? "done" : "todo";
}

/** How each status reads, in the status control and the tick's accessible name. */
export const TASK_STATUS_LABEL: Record<TaskStatus, string> = {
  todo: "To do",
  doing: "Doing",
  done: "Done",
};

export type DeadlineTone = "late" | "soon" | "normal";

export interface DeadlineInfo {
  text: string;
  tone: DeadlineTone;
}

/** A deadline's chip text and tone, relative to `today` (a local day).
 *
 *  - before today: `Nd late`, late
 *  - today: `today`, late
 *  - tomorrow: `tomorrow`, soon
 *  - 2 to 6 days out: `Nd · DD-MM`, soon
 *  - further out: the date, normal
 *
 *  A soft deadline ("aim for") reads `aim …` and is never toned late: its tone caps at soon. A done
 *  task keeps the words but loses the tone, since nothing is pressing any more. `null` when there's
 *  no date (or it can't be read). */
export function deadlineInfo(
  deadline: string | null,
  kind: DeadlineKind,
  today: Date,
  done = false,
): DeadlineInfo | null {
  if (!deadline) return null;
  const day = parseLocal(deadline, true);
  if (!day) return null;
  const days = dayDiff(startOfDay(today), day);
  let text: string;
  let tone: DeadlineTone;
  if (days < 0) {
    text = `${-days}d late`;
    tone = "late";
  } else if (days === 0) {
    text = "today";
    tone = "late";
  } else if (days === 1) {
    text = "tomorrow";
    tone = "soon";
  } else if (days <= 6) {
    text = `${days}d · ${formatDateLocal(day)}`;
    tone = "soon";
  } else {
    text = formatDateLocal(day);
    tone = "normal";
  }
  if (kind === "soft") {
    text = `aim ${text}`;
    if (tone === "late") tone = "soon";
  }
  if (done) tone = "normal";
  return { text, tone };
}

/** The text class for each deadline tone: late = due, soon = look, otherwise the quiet ink. */
export const TONE_CLASS: Record<DeadlineTone, string> = {
  late: "text-st-due",
  soon: "text-st-look",
  normal: "text-ink4",
};

/** A task still to be done (to do or doing). */
export function isOpen(t: Pick<Task, "status">): boolean {
  return t.status !== "done";
}

/** The one global order: by ordinal, then id. */
export function compareTasks(
  a: Pick<Task, "ordinal" | "id">,
  b: Pick<Task, "ordinal" | "id">,
): number {
  return a.ordinal - b.ordinal || a.id - b.id;
}

/** How each stack kind reads. */
export const STACK_KIND_LABEL: Record<TaskStackKind, string> = {
  sequential: "In sequence: each one gates the next",
  in_order: "In order",
  waiting_on: "Waiting on someone",
};

/** Where a deadline can come from, as the editor's Deadline row names it. */
export const DEADLINE_FROM_LABEL: Record<DeadlineFrom, string> = {
  date: "My own date",
  milestone: "Milestone's date",
  event: "Event's date",
};

/** A blank editor input, with `defaults` laid over it (a list's project, a Runway column's day). */
export function emptyTaskInput(defaults?: Partial<TaskInput>): TaskInput {
  return {
    title: "",
    why: null,
    project: null,
    milestoneId: null,
    eventUid: null,
    doStart: null,
    doEnd: null,
    deadline: null,
    deadlineKind: "hard",
    deadlineFrom: "date",
    lane: "foreground",
    stackId: null,
    parked: false,
    ...defaults,
  };
}

/** A stored task as the editor's input. A deadline that follows a link that's gone (its milestone
 *  was deleted, so `milestone_id` was nulled under it) goes back to the task's own date: the save
 *  would otherwise be refused for naming a link the task no longer has. */
export function taskToInput(t: Task): TaskInput {
  const dangling =
    (t.deadline_from === "milestone" && t.milestone_id === null) ||
    (t.deadline_from === "event" && t.event_uid === null);
  return {
    title: t.title,
    why: t.why,
    project: t.project_name,
    milestoneId: t.milestone_id,
    eventUid: t.event_uid,
    doStart: t.do_start,
    doEnd: t.do_end,
    deadline: t.deadline,
    deadlineKind: t.deadline_kind,
    deadlineFrom: dangling ? "date" : t.deadline_from,
    lane: t.lane,
    stackId: t.stack_id,
    parked: t.parked,
  };
}

/** One calendar event a task can link to: one per iCal UID, at the occurrence that matters. */
export interface EventChoice {
  uid: string;
  /** The chosen occurrence's local day, `YYYY-MM-DD`. */
  date: string;
  title: string;
  /** "DD-MM · title", as the picker lists it. */
  label: string;
  /** The occurrence is before today (no later one exists). */
  past: boolean;
}

/** How far back a past event still counts as something a task might link to. */
export const EVENT_CHOICE_PAST_DAYS = 30;

/** The events a task can link to, from the raw calendar mirror: one per iCal UID (a recurring
 *  series or an event on two calendars is one choice), at its next occurrence on or after `today`,
 *  or, when it has none, its latest occurrence within the last 30 days. Older ones and events with
 *  no UID are left out. Upcoming choices come first, soonest first; then the past ones, latest
 *  first. */
export function eventChoices(events: readonly CalendarEvent[], today: Date): EventChoice[] {
  const t0 = startOfDay(today);
  const floor = addDays(t0, -EVENT_CHOICE_PAST_DAYS);
  const next = new Map<string, { day: Date; title: string }>();
  const last = new Map<string, { day: Date; title: string }>();
  for (const e of events) {
    if (!e.uid) continue;
    const at = parseLocal(e.start, e.all_day);
    if (!at) continue;
    const day = startOfDay(at);
    const title = e.summary.trim() || "Untitled event";
    if (day >= t0) {
      const cur = next.get(e.uid);
      if (!cur || day < cur.day) next.set(e.uid, { day, title });
    } else if (day >= floor) {
      const cur = last.get(e.uid);
      if (!cur || day > cur.day) last.set(e.uid, { day, title });
    }
  }
  const choice = (uid: string, v: { day: Date; title: string }, past: boolean): EventChoice => ({
    uid,
    date: dayKey(v.day),
    title: v.title,
    label: `${formatDateLocal(v.day)} · ${v.title}`,
    past,
  });
  const upcoming = [...next.entries()]
    .sort((a, b) => a[1].day.getTime() - b[1].day.getTime())
    .map(([uid, v]) => choice(uid, v, false));
  const past = [...last.entries()]
    .filter(([uid]) => !next.has(uid))
    .sort((a, b) => b[1].day.getTime() - a[1].day.getTime())
    .map(([uid, v]) => choice(uid, v, true));
  return [...upcoming, ...past];
}

/** The choices whose label contains every word of `query` (case-insensitive), in order. */
export function filterEventChoices(choices: readonly EventChoice[], query: string): EventChoice[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return [...choices];
  return choices.filter((c) => {
    const hay = c.label.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}
