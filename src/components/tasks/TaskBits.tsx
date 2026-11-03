// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The small pieces every task surface shares: the status tick, the deadline chip and the stack
// chip. The project page and the pinboard Task list (TaskList) and the Runway's cards all render
// these, so a task's status and deadline read the same wherever it shows up.

import {
  deadlineInfo,
  nextStatus,
  STACK_KIND_LABEL,
  TASK_STATUS_LABEL,
  TONE_CLASS,
} from "../../lib/tasks";
import type { Task, TaskStack, TaskStatus } from "../../lib/types";
import { useTheme } from "../../theme";
import { cn } from "../ui";

/** The tick that cycles a task's status: an empty box (to do), a half-filled box (doing), a ticked
 *  box (done), drawn in CSS from tokens. Terminal draws `[ ]` / `[~]` / `[x]` instead. Its name
 *  says the status now and what a press makes it. */
export function TaskStatusButton({
  status,
  onCycle,
  disabled,
  className,
}: {
  status: TaskStatus;
  onCycle: (next: TaskStatus) => void;
  disabled?: boolean;
  className?: string;
}) {
  const { system } = useTheme();
  const next = nextStatus(status);
  const name = `${TASK_STATUS_LABEL[status]}: mark as ${TASK_STATUS_LABEL[next].toLowerCase()}`;
  return (
    <button
      type="button"
      aria-label={name}
      title={name}
      disabled={disabled}
      onClick={() => onCycle(next)}
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center disabled:cursor-not-allowed",
        className,
      )}
    >
      {system === "terminal" ? (
        <span
          aria-hidden
          className={cn("font-mono text-xs", status === "todo" ? "text-ink3" : "text-accent-text")}
        >
          {status === "todo" ? "[ ]" : status === "doing" ? "[~]" : "[x]"}
        </span>
      ) : (
        <span
          aria-hidden
          className={cn(
            "relative inline-flex h-3.5 w-3.5 items-center justify-center overflow-hidden rounded-[2px] border",
            status === "done" ? "border-accent bg-accent" : "border-ink4 bg-surface",
          )}
        >
          {status === "doing" && <span className="absolute inset-y-0 left-0 w-1/2 bg-accent" />}
          {status === "done" && (
            // A tick: the bottom and right edges of a narrow box, turned 45°.
            <span className="mb-0.5 h-2 w-1 rotate-45 border-b-2 border-r-2 border-accent-ink" />
          )}
        </span>
      )}
    </button>
  );
}

/** Where a task's deadline comes from, for the chip's tooltip. */
function deadlineSource(t: Task): string {
  if (t.deadline_missing) {
    return t.deadline_from === "milestone"
      ? "The linked milestone has no date, so this is your own date"
      : "The linked event isn't in your calendars, so this is your own date";
  }
  if (t.deadline_from === "milestone")
    return `Milestone's date${t.milestone_label ? `: ${t.milestone_label}` : ""}`;
  if (t.deadline_from === "event")
    return `Event's date${t.event_title ? `: ${t.event_title}` : ""}`;
  return t.deadline_kind === "soft" ? "A date to aim for" : "Deadline";
}

/** A task's deadline as a small toned chip (from `effective_deadline`), or nothing without one. A
 *  deadline that follows a milestone or event carries ◆ or a calendar mark; one whose link didn't
 *  resolve carries a ⚠. */
export function DeadlineChip({
  task,
  today,
  className,
}: {
  task: Task;
  today: Date;
  className?: string;
}) {
  const info = deadlineInfo(
    task.effective_deadline,
    task.deadline_kind,
    today,
    task.status === "done",
  );
  if (!info && !task.deadline_missing) return null;
  const mark = task.deadline_missing
    ? "⚠ "
    : task.deadline_from === "milestone"
      ? "◆ "
      : task.deadline_from === "event"
        ? "📅 "
        : "";
  return (
    <span
      title={deadlineSource(task)}
      className={cn(
        "shrink-0 whitespace-nowrap font-mono text-[0.6875rem]",
        TONE_CLASS[info?.tone ?? "normal"],
        className,
      )}
    >
      {mark}
      {info?.text ?? "no date"}
    </span>
  );
}

/** A small chip naming the stack a task belongs to (its title, or its kind when untitled). */
export function StackChip({ stack, className }: { stack: TaskStack; className?: string }) {
  const kind = STACK_KIND_LABEL[stack.kind];
  return (
    <span
      title={stack.title ? `${stack.title} · ${kind}` : kind}
      className={cn(
        "max-w-[8rem] shrink-0 truncate rounded-[var(--radius-sm)] border border-border px-1 text-[0.6875rem] text-ink3",
        className,
      )}
    >
      ≡ {stack.title || kind}
    </span>
  );
}

/** A small chip naming a task's project. A button when `onOpen` is given (the Runway opens the
 *  project from it). */
export function ProjectChip({
  project,
  onOpen,
  className,
}: {
  project: string;
  onOpen?: (project: string) => void;
  className?: string;
}) {
  const cls = cn(
    "max-w-[9rem] shrink-0 truncate rounded-[var(--radius-sm)] bg-surface px-1 text-[0.6875rem] text-ink3",
    onOpen && "hover:text-ink",
    className,
  );
  return onOpen ? (
    <button type="button" className={cls} title={`Open ${project}`} onClick={() => onOpen(project)}>
      {project}
    </button>
  ) : (
    <span className={cls} title={project}>
      {project}
    </span>
  );
}
