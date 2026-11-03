// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The live task list (PA-1): the project page's Tasks panel and the pinboard Task list widget are
// both this. It keeps no copy of its own: it reads `listTasks` on mount, on window focus and on
// every `tasks://changed` (fired after any task write, from any surface), so ticking a task here
// ticks it on the Runway and the calendar too.
//
// Open tasks lead, in the one global order (↑/↓ reorder them within it); parked ones and done ones
// sit under folds that start closed. Below Standard depth the list is a plain summary with no
// controls, as the milestones panel is.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  addTask,
  changeTaskStatus,
  listTasks,
  listTaskStacks,
  onTasksChanged,
  reorderTasks,
} from "../../lib/ipc";
import { startOfDay } from "../../lib/calendar-layout";
import { runMutation } from "../../lib/runMutation";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import { compareTasks, deadlineInfo, emptyTaskInput, isOpen } from "../../lib/tasks";
import type { Task, TaskInput, TaskStack } from "../../lib/types";
import { useNowTick } from "../../lib/useNowTick";
import { useDepth } from "../../theme";
import { DateField } from "../DateField";
import { Button, Callout, Collapsible, Input, cn } from "../ui";
import { DeadlineChip, ProjectChip, StackChip, TaskStatusButton } from "./TaskBits";
import { TaskEditor } from "./TaskEditor";

export interface TaskListProps {
  /** One project's tasks; undefined lists every task. */
  project?: string;
  /** Show tasks of archived projects too (always shown when `project` is set). */
  includeArchived?: boolean;
  /** Tighter rows, for a pinboard card. */
  compact?: boolean;
  /** A plain summary with no controls. */
  readOnly?: boolean;
  /** Each row names its project (for an all-tasks list). */
  showProject?: boolean;
}

type Editing = { task: Task } | { defaults: Partial<TaskInput> } | null;

/** Done tasks, most recently finished first. */
function byCompleted(a: Task, b: Task): number {
  return (b.completed_at ?? "").localeCompare(a.completed_at ?? "") || compareTasks(a, b);
}

export function TaskList({
  project,
  includeArchived = false,
  compact = false,
  readOnly = false,
  showProject = false,
}: TaskListProps) {
  const [tasks, setTasks] = useState<Task[] | null>(null);
  const [stacks, setStacks] = useState<TaskStack[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  const { showMeta } = useDepth();
  const today = startOfDay(useNowTick());

  // A read that lands after a newer one began (a project switch, a burst of changes) is dropped,
  // so the list can't flash back to an older state.
  const seq = useRef(0);
  const reload = useCallback(() => {
    const n = ++seq.current;
    listTasks(project ?? null)
      .then((t) => {
        if (n === seq.current) setTasks(t);
      })
      .catch((e) => {
        if (n === seq.current) setError(String(e));
      });
    listTaskStacks()
      .then((s) => {
        if (n === seq.current) setStacks(s);
      })
      .catch(() => {});
  }, [project]);

  useEffect(() => {
    reload();
    window.addEventListener("focus", reload);
    const off = subscribeUntilCleanup(() => onTasksChanged(reload));
    return () => {
      window.removeEventListener("focus", reload);
      off();
    };
  }, [reload]);

  const visible = (tasks ?? []).filter(
    (t) => includeArchived || project !== undefined || !t.project_archived,
  );
  const open = visible.filter((t) => isOpen(t) && !t.parked).sort(compareTasks);
  const parked = visible.filter((t) => isOpen(t) && t.parked).sort(compareTasks);
  const done = visible.filter((t) => !isOpen(t)).sort(byCompleted);
  const stackById = new Map(stacks.map((s) => [s.id, s]));

  if (readOnly || !showMeta) {
    return (
      <TaskSummary loaded={tasks !== null} open={open} parked={parked} done={done} today={today} />
    );
  }

  const mutate = (fn: () => Promise<unknown>) =>
    void runMutation(async () => {
      await fn();
      reload();
    }, setError);

  const row = (t: Task, i: number, list: Task[], reorderable: boolean) => (
    <TaskRow
      key={t.id}
      task={t}
      stack={t.stack_id === null ? undefined : stackById.get(t.stack_id)}
      showProject={showProject}
      compact={compact}
      today={today}
      onCycle={(next) => mutate(() => changeTaskStatus(t.id, next))}
      onEdit={() => setEditing({ task: t })}
      isFirst={i === 0}
      isLast={i === list.length - 1}
      onMove={
        reorderable
          ? (dir) => {
              const j = i + dir;
              if (j < 0 || j >= list.length) return;
              const ids = list.map((x) => x.id);
              [ids[i], ids[j]] = [ids[j], ids[i]];
              mutate(() => reorderTasks(ids));
            }
          : undefined
      }
    />
  );

  return (
    <div className={cn("flex flex-col", compact ? "gap-1" : "gap-1.5")}>
      {tasks !== null && open.length === 0 && (
        <p className="rounded-[var(--radius-sm)] border border-dashed border-border px-3 py-2 text-xs text-ink4">
          {parked.length + done.length > 0
            ? "Nothing open right now. Add the next thing to do below."
            : project
              ? "No tasks for this project yet. Add one below."
              : "No tasks yet. Add one below, or from a project."}
        </p>
      )}
      {open.map((t, i) => row(t, i, open, true))}
      <AddTaskRow
        project={project}
        compact={compact}
        onAdd={(input) =>
          runMutation(async () => {
            await addTask(input);
            reload();
          }, setError)
        }
        onMore={(defaults) => setEditing({ defaults })}
      />
      {error && <Callout as="p">{error}</Callout>}
      {parked.length > 0 && (
        <Collapsible title="Parked" meta={parked.length} defaultOpen={false} className="mt-1">
          <div className={cn("flex flex-col pt-1", compact ? "gap-1" : "gap-1.5")}>
            {parked.map((t, i) => row(t, i, parked, false))}
          </div>
        </Collapsible>
      )}
      {done.length > 0 && (
        <Collapsible title={`Done (${done.length})`} defaultOpen={false} className="mt-1">
          <div className={cn("flex flex-col pt-1", compact ? "gap-1" : "gap-1.5")}>
            {done.map((t, i) => row(t, i, done, false))}
          </div>
        </Collapsible>
      )}
      <TaskEditor
        open={editing !== null}
        task={editing && "task" in editing ? editing.task : null}
        defaults={editing && "defaults" in editing ? editing.defaults : undefined}
        onClose={() => setEditing(null)}
        onSaved={reload}
        onDeleted={reload}
      />
    </div>
  );
}

/** The plain list shown below Standard depth: titles and deadlines, done ones struck through. */
function TaskSummary({
  loaded,
  open,
  parked,
  done,
  today,
}: {
  loaded: boolean;
  open: Task[];
  parked: Task[];
  done: Task[];
  today: Date;
}) {
  if (!loaded) return null;
  if (open.length + parked.length + done.length === 0)
    return <p className="font-mono text-xs text-ink4">No tasks yet.</p>;
  const line = (t: Task, note?: string) => {
    const due = deadlineInfo(t.effective_deadline, t.deadline_kind, today, t.status === "done");
    return (
      <li key={t.id} className={t.status === "done" ? "line-through opacity-60" : ""}>
        {t.status === "doing" ? "▸ " : ""}
        {t.title}
        {due ? ` · ${due.text}` : ""}
        {note ? ` · ${note}` : ""}
      </li>
    );
  };
  return (
    <ul className="flex flex-col gap-0.5 font-mono text-xs text-ink4">
      {open.map((t) => line(t))}
      {parked.map((t) => line(t, "parked"))}
      {done.map((t) => line(t))}
    </ul>
  );
}

function TaskRow({
  task: t,
  stack,
  showProject,
  compact,
  today,
  onCycle,
  onEdit,
  onMove,
  isFirst,
  isLast,
}: {
  task: Task;
  stack?: TaskStack;
  showProject: boolean;
  compact: boolean;
  today: Date;
  onCycle: (next: Task["status"]) => void;
  onEdit: () => void;
  /** Reorder within the list; absent hides the ↑/↓ arrows. */
  onMove?: (dir: -1 | 1) => void;
  isFirst: boolean;
  isLast: boolean;
}) {
  const [whyOpen, setWhyOpen] = useState(false);
  const done = t.status === "done";
  const titleCls = cn(
    "min-w-0 break-words text-left text-xs text-ink2",
    done && "line-through opacity-60",
  );
  return (
    <div
      className={cn(
        "flex items-start gap-1.5 rounded-[var(--radius-sm)] border border-border bg-surface",
        compact ? "px-1.5 py-0.5" : "px-2 py-1",
      )}
    >
      <TaskStatusButton status={t.status} onCycle={onCycle} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5 py-1">
        {t.why ? (
          <button
            type="button"
            aria-expanded={whyOpen}
            title={whyOpen ? "Hide the note" : "Show why"}
            onClick={() => setWhyOpen(!whyOpen)}
            className={cn(titleCls, "hover:text-ink")}
          >
            {t.title}
          </button>
        ) : (
          <span className={titleCls}>{t.title}</span>
        )}
        {whyOpen && t.why && (
          <p className="whitespace-pre-wrap break-words text-xs text-ink3">{t.why}</p>
        )}
        <div className="flex min-w-0 flex-wrap items-center gap-1">
          <DeadlineChip task={t} today={today} />
          {t.status === "doing" && (
            <span className="shrink-0 font-mono text-[0.6875rem] text-accent-text">doing</span>
          )}
          {showProject && t.project_name && <ProjectChip project={t.project_name} />}
          {stack && <StackChip stack={stack} />}
          {!compact && t.milestone_label && t.deadline_from !== "milestone" && (
            <span
              className="max-w-[9rem] shrink-0 truncate text-[0.6875rem] text-ink4"
              title="Linked milestone"
            >
              ◆ {t.milestone_label}
            </span>
          )}
          {!compact && t.event_title && t.deadline_from !== "event" && (
            <span
              className="max-w-[9rem] shrink-0 truncate text-[0.6875rem] text-ink4"
              title="Linked event"
            >
              📅 {t.event_title}
            </span>
          )}
        </div>
      </div>
      <div className="flex shrink-0 items-center">
        {onMove && (
          <>
            <Button
              variant="tertiary"
              size="xs"
              onClick={() => onMove(-1)}
              disabled={isFirst}
              title="Move up"
              aria-label={`Move ${t.title} up`}
            >
              ↑
            </Button>
            <Button
              variant="tertiary"
              size="xs"
              onClick={() => onMove(1)}
              disabled={isLast}
              title="Move down"
              aria-label={`Move ${t.title} down`}
            >
              ↓
            </Button>
          </>
        )}
        <Button
          variant="tertiary"
          size="xs"
          onClick={onEdit}
          title="Edit task"
          aria-label={`Edit ${t.title}`}
        >
          ✎
        </Button>
      </div>
    </div>
  );
}

/** The quick-add row at the foot of the list: a title (Enter adds) and an optional deadline, or
 *  "More…" for the full editor with what's typed so far. */
function AddTaskRow({
  project,
  compact,
  onAdd,
  onMore,
}: {
  project?: string;
  compact: boolean;
  onAdd: (input: TaskInput) => Promise<boolean>;
  onMore: (defaults: Partial<TaskInput>) => void;
}) {
  const [title, setTitle] = useState("");
  const [date, setDate] = useState("");
  const [busy, setBusy] = useState(false);
  const defaults = (): Partial<TaskInput> => ({
    project: project ?? null,
    title: title.trim(),
    deadline: date || null,
  });

  async function add() {
    if (!title.trim() || busy) return;
    setBusy(true);
    const ok = await onAdd(emptyTaskInput(defaults()));
    if (ok) {
      setTitle("");
      setDate("");
    }
    setBusy(false);
  }

  // flex-wrap + min-w-0 basis, as the milestone add row: the date drops to its own line when the
  // panel is narrow rather than forcing a sideways scroll.
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <Input
        value={title}
        aria-label="New task"
        maxLength={500}
        onChange={(e) => setTitle(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void add();
        }}
        placeholder="New task"
        className={cn("min-w-0 flex-1 basis-28 text-xs", compact ? "h-6" : "h-7")}
      />
      <DateField
        value={date}
        onCommit={setDate}
        ariaLabel="New task deadline"
        title="Deadline (optional)"
        wrapperClassName="min-w-0 flex-1 basis-24"
        className={cn("px-1.5 text-xs", compact ? "h-6" : "h-7")}
      />
      <div className="flex shrink-0 items-center gap-1">
        <Button
          variant="secondary"
          size="sm"
          onClick={() => void add()}
          disabled={busy || !title.trim()}
        >
          Add
        </Button>
        <Button
          variant="tertiary"
          size="xs"
          onClick={() => onMore(defaults())}
          title="Add with links, a plan and more"
        >
          More…
        </Button>
      </div>
    </div>
  );
}
