// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one task editor (PA-1). The project page, the pinboard Task list, the Runway and the calendar
// overlay all open this same dialog, add through `addTask` and save through `updateTask`, so a task
// edited anywhere reads the same everywhere (the backend fires `tasks://changed` after each write,
// and every list re-reads on it).
//
// Every link is optional and independent: a project, a milestone (of that project, or of any
// project when none is set, in which case the task takes on the milestone's project) and a calendar
// event. The deadline can be the task's own date, or follow the linked milestone's or event's date:
// "do it Monday, the deadline is Friday's event" is a Monday plan with the event's date as deadline.
// An event deadline is pinned to the occurrence that was picked (written to the task's own
// `deadline`, spec R1), so a recurring event's Friday stays the deadline once it has passed.
//
// The dialog is portalled to <body>: a host inside a transformed or clipped card (a pinboard tile)
// would otherwise trap a `position: fixed` scrim inside the card. React still bubbles a portal's
// events to its React parents, so the portal root stops `pointerdown` from reaching a host's drag
// handler. Mousedown and keydown are left alone (the date popover and Escape listen for those on
// document and window).

import { useEffect, useId, useMemo, useState } from "react";
import { createPortal } from "react-dom";
import {
  addTask,
  addTaskStack,
  changeTaskStatus,
  deleteTask,
  listAllCalendarEvents,
  listAllMilestones,
  listProjects,
  listTaskStacks,
  updateTask,
} from "../../lib/ipc";
import { formatDateOnly } from "../../lib/format";
import { runMutation } from "../../lib/runMutation";
import {
  DEADLINE_FROM_LABEL,
  emptyTaskInput,
  eventChoices,
  STACK_KIND_LABEL,
  STACK_KINDS,
  TASK_STATUS_LABEL,
  taskToInput,
  type EventChoice,
} from "../../lib/tasks";
import type {
  DeadlineFrom,
  DeadlineKind,
  Milestone,
  Task,
  TaskInput,
  TaskLane,
  TaskStack,
  TaskStackKind,
  TaskStatus,
} from "../../lib/types";
import { DateField } from "../DateField";
import {
  Button,
  Callout,
  ConfirmDialog,
  Dialog,
  Input,
  SegmentedControl,
  Select,
  Textarea,
  Toggle,
} from "../ui";
import { EventPicker } from "./EventPicker";
import { StackEditor } from "./StackEditor";

/** The longest title and note the backend takes. */
const TITLE_MAX = 500;
const WHY_MAX = 10_000;

/** The stack picker's "make a new one" entry. */
const NEW_STACK = "new";

const STATUSES: TaskStatus[] = ["todo", "doing", "done"];

export interface TaskEditorProps {
  open: boolean;
  /** The task to edit; absent (or null) adds a new one. */
  task?: Task | null;
  /** A new task's starting values (a list's project, a Runway column's day). */
  defaults?: Partial<TaskInput>;
  /** The task belongs to this project and the editor doesn't offer to change it. */
  fixedProject?: string;
  onClose: () => void;
  /** After a save, with the task's id. */
  onSaved?: (id: number) => void;
  /** After the task was deleted. */
  onDeleted?: () => void;
}

export function TaskEditor(props: TaskEditorProps) {
  if (!props.open) return null;
  // The body mounts fresh each time the editor opens (and per task), so its fields start from the
  // task or the defaults rather than from whatever the last opening left behind.
  return createPortal(
    <div onPointerDown={(e) => e.stopPropagation()}>
      <TaskEditorBody key={props.task?.id ?? "new"} {...props} />
    </div>,
    document.body,
  );
}

/** Two spellings of one project, as the backend matches them (case-insensitive; it also resolves
 *  aliases, which the editor can't see, so an alias still reads as another project here). */
function sameProject(a: string, b: string): boolean {
  return a.toLocaleLowerCase() === b.toLocaleLowerCase();
}

/** Empty text is no value. */
function clean(s: string): string | null {
  const t = s.trim();
  return t ? t : null;
}

function TaskEditorBody({
  task,
  defaults,
  fixedProject,
  onClose,
  onSaved,
  onDeleted,
}: TaskEditorProps) {
  // Seeded once per mount: the body is keyed by task id and remounted on every opening.
  const [initial] = useState(() =>
    task
      ? taskToInput(task)
      : emptyTaskInput({ ...defaults, ...(fixedProject ? { project: fixedProject } : {}) }),
  );
  const [title, setTitle] = useState(initial.title);
  const [why, setWhy] = useState(initial.why ?? "");
  const [project, setProject] = useState(initial.project ?? "");
  const [milestoneId, setMilestoneId] = useState<number | null>(initial.milestoneId);
  const [eventUid, setEventUid] = useState<string | null>(initial.eventUid);
  // The day of the linked event's occurrence the deadline is pinned to (spec R1): a task whose
  // deadline follows an event stores it in its own `deadline` column, so a recurring event's chosen
  // Friday stays the deadline after it passes. Kept apart from the own date while the dialog is
  // open, so switching between "My own date" and "Event's date" loses neither.
  const [eventPin, setEventPin] = useState<string | null>(
    initial.deadlineFrom === "event" ? initial.deadline : null,
  );
  const [deadline, setDeadline] = useState(initial.deadline ?? "");
  const [deadlineKind, setDeadlineKind] = useState<DeadlineKind>(initial.deadlineKind);
  const [deadlineFrom, setDeadlineFrom] = useState<DeadlineFrom>(initial.deadlineFrom);
  const [doStart, setDoStart] = useState(initial.doStart ?? "");
  const [doEnd, setDoEnd] = useState(initial.doEnd ?? "");
  const [lane, setLane] = useState<TaskLane>(initial.lane);
  const [parked, setParked] = useState(initial.parked);
  const [stackSel, setStackSel] = useState(initial.stackId === null ? "" : String(initial.stackId));
  const [newStackTitle, setNewStackTitle] = useState("");
  const [newStackKind, setNewStackKind] = useState<TaskStackKind>("sequential");
  const [newStackNote, setNewStackNote] = useState("");
  const [status, setStatus] = useState<TaskStatus>(task?.status ?? "todo");
  const [editingStack, setEditingStack] = useState<TaskStack | null>(null);

  const [projects, setProjects] = useState<string[]>([]);
  const [milestones, setMilestones] = useState<Milestone[] | null>(null);
  const [stacks, setStacks] = useState<TaskStack[]>([]);
  const [choices, setChoices] = useState<EventChoice[] | null>(null);
  const [today] = useState(() => new Date());

  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const ids = {
    title: useId(),
    why: useId(),
    project: useId(),
    projectList: useId(),
    milestone: useId(),
    event: useId(),
    deadlineFrom: useId(),
    deadline: useId(),
    deadlineKind: useId(),
    doStart: useId(),
    doEnd: useId(),
    lane: useId(),
    parked: useId(),
    stack: useId(),
    stackTitle: useId(),
    stackKind: useId(),
    stackNote: useId(),
    status: useId(),
  };

  // What the pickers choose from. Each read fails on its own: a calendar that won't load leaves the
  // event picker empty, not the editor broken.
  useEffect(() => {
    let live = true;
    if (!fixedProject)
      listProjects()
        .then((p) => live && setProjects(p))
        .catch(() => {});
    listAllMilestones()
      .then((m) => live && setMilestones(m))
      .catch(() => live && setMilestones([]));
    listTaskStacks()
      .then((s) => live && setStacks(s))
      .catch(() => {});
    // The event the task already links to stays a choice however old it is, as the backend still
    // resolves a deadline that follows it.
    listAllCalendarEvents()
      .then((ev) => live && setChoices(eventChoices(ev, today, initial.eventUid)))
      .catch(() => live && setChoices([]));
    return () => {
      live = false;
    };
  }, [fixedProject, today, initial.eventUid]);

  const scope = fixedProject ?? clean(project);
  const milestoneById = useMemo(
    () => new Map((milestones ?? []).map((m) => [m.id, m])),
    [milestones],
  );
  const linkedMilestone = milestoneId === null ? undefined : milestoneById.get(milestoneId);
  const milestoneOptions = useMemo(() => {
    const all = milestones ?? [];
    const inScope = scope ? all.filter((m) => sameProject(m.project_name, scope)) : all;
    // Keep the linked one listed even when the project field has moved off it, so the Select never
    // shows a value it has no option for.
    if (linkedMilestone && !inScope.includes(linkedMilestone)) return [linkedMilestone, ...inScope];
    return inScope;
  }, [milestones, scope, linkedMilestone]);
  const linkedChoice = eventUid === null ? undefined : choices?.find((c) => c.uid === eventUid);
  // The stored link, unchanged in this dialog: the backend's `effective_deadline` and
  // `deadline_missing` already say what it resolves to (the pinned occurrence, or a moved one).
  const savedEventLink =
    task !== undefined &&
    task !== null &&
    task.deadline_from === "event" &&
    task.event_uid !== null &&
    eventUid === task.event_uid &&
    eventPin === task.deadline;
  // The date the event field shows beside the title. For the link as saved, the occurrence the
  // backend resolved (a moved event shows its new day, as the deadline line does); for one chosen
  // here, the pin; and none while the deadline isn't the event's, where the pin does nothing.
  const pickerDate =
    deadlineFrom !== "event"
      ? null
      : savedEventLink && task && !task.deadline_missing && task.effective_deadline
        ? task.effective_deadline
        : eventPin;
  const selectedStack =
    stackSel && stackSel !== NEW_STACK ? stacks.find((s) => String(s.id) === stackSel) : undefined;

  function clearMilestone() {
    setMilestoneId(null);
    if (deadlineFrom === "milestone") setDeadlineFrom("date");
  }
  function changeEvent(uid: string | null) {
    setEventUid(uid);
    // Picking an event pins its occurrence the picker showed; the deadline takes that day when it
    // follows the event.
    setEventPin(uid === null ? null : (choices?.find((c) => c.uid === uid)?.date ?? null));
    if (uid === null && deadlineFrom === "event") setDeadlineFrom("date");
  }
  function chooseDeadlineFrom(from: DeadlineFrom) {
    setDeadlineFrom(from);
    // Switching to the event's date with no pin yet (a link made earlier, with the task's own date
    // as its deadline) pins the occurrence the picker shows. An existing pin is kept.
    if (from === "event" && eventPin === null && linkedChoice) setEventPin(linkedChoice.date);
  }
  function clearProject() {
    setProject("");
    // A milestone brings its project with it on save, so a cleared project lets it go too.
    if (milestoneId !== null) clearMilestone();
  }
  function chooseMilestone(value: string) {
    if (!value) return clearMilestone();
    const id = Number(value);
    setMilestoneId(id);
    // A milestone picked with no project set brings its project along (the backend would fill it
    // in on save anyway; showing it here says so).
    const m = milestoneById.get(id);
    if (m && !fixedProject && !clean(project)) setProject(m.project_name);
  }
  // Leaving the project field on a different project than the linked milestone's lets the
  // milestone go: the save would be refused for it. Leaving it empty lets it go too: the save
  // would otherwise file the task under the milestone's project while the field says "No project".
  function settleProject() {
    const p = clean(project);
    if (p && linkedMilestone && !sameProject(linkedMilestone.project_name, p)) clearMilestone();
    else if (!p && milestoneId !== null) clearMilestone();
  }
  function changeDoStart(iso: string) {
    setDoStart(iso);
    if (!iso || (doEnd && doEnd < iso)) setDoEnd("");
  }

  async function save() {
    const name = title.trim();
    if (!name) {
      setError("A task needs a title.");
      return;
    }
    if (stackSel === NEW_STACK && !newStackTitle.trim()) {
      setError("Give the new stack a name.");
      return;
    }
    setBusy(true);
    let savedId = task?.id ?? 0;
    const ok = await runMutation(async () => {
      let stackId: number | null = stackSel === "" ? null : Number(stackSel);
      if (stackSel === NEW_STACK) {
        const note = clean(newStackNote);
        const stackTitle = newStackTitle.trim();
        const id = await addTaskStack(stackTitle, newStackKind, note);
        // Should the task save below fail, a retry reuses this stack rather than making another.
        setStacks((s) => [
          ...s,
          { id, title: stackTitle, kind: newStackKind, note, created_at: "", updated_at: "" },
        ]);
        setStackSel(String(id));
        stackId = id;
      }
      const input: TaskInput = {
        title: name,
        why: clean(why),
        project: fixedProject ?? clean(project),
        milestoneId,
        eventUid,
        doStart: doStart || null,
        doEnd: doStart && doEnd ? doEnd : null,
        // An event deadline writes its pinned occurrence (the own date only when there is none,
        // an event that isn't in the calendars).
        deadline:
          deadlineFrom === "event"
            ? (eventPin ?? linkedChoice?.date ?? (deadline || null))
            : deadline || null,
        deadlineKind,
        deadlineFrom,
        lane,
        stackId,
        parked,
      };
      if (task) {
        await updateTask(task.id, input);
        if (status !== task.status) await changeTaskStatus(task.id, status);
      } else {
        savedId = await addTask(input);
      }
    }, setError);
    setBusy(false);
    if (ok) {
      onSaved?.(savedId);
      onClose();
    }
  }

  async function remove() {
    if (!task) return;
    setBusy(true);
    const ok = await runMutation(() => deleteTask(task.id), setError);
    setBusy(false);
    setConfirmDelete(false);
    if (ok) {
      onDeleted?.();
      onClose();
    }
  }

  const fieldLabel = "mb-1 block text-xs text-ink3";
  const milestoneText = (m: Milestone) =>
    `${scope ? "" : `${m.project_name} · `}${m.label}${
      m.due_date ? ` · ${formatDateOnly(m.due_date.slice(0, 10))}` : ""
    }`;

  // What a linked deadline resolves to right now, in words.
  let linkedDeadline: string | null = null;
  if (deadlineFrom === "milestone") {
    linkedDeadline = linkedMilestone?.due_date
      ? `◆ ${linkedMilestone.label}: ${formatDateOnly(linkedMilestone.due_date.slice(0, 10))}`
      : milestones === null
        ? "Reading the milestone…"
        : `This milestone has no date yet, so your own date is used${
            deadline ? ` (${formatDateOnly(deadline)})` : ", and you haven't set one"
          }.`;
  } else if (deadlineFrom === "event") {
    const ownDate = (iso: string | null) =>
      `That event isn't in your calendars right now, so your own date is used${
        iso ? ` (${formatDateOnly(iso)})` : ", and you haven't set one"
      }.`;
    if (savedEventLink) {
      // The link as saved: say what the backend resolved it to, not what the picker's list holds.
      const eventTitle = linkedChoice?.title ?? task?.event_title ?? "The event";
      linkedDeadline =
        task?.deadline_missing || !task?.effective_deadline
          ? ownDate(task?.effective_deadline ?? task?.deadline ?? null)
          : `📅 ${eventTitle}: ${formatDateOnly(task.effective_deadline)}`;
    } else if (linkedChoice) {
      const pin = eventPin ?? linkedChoice.date;
      linkedDeadline = `📅 ${linkedChoice.title}: ${formatDateOnly(pin)}`;
    } else {
      linkedDeadline = choices === null ? "Reading your calendars…" : ownDate(deadline || null);
    }
  }

  return (
    <>
      <Dialog
        open
        onClose={busy ? () => {} : onClose}
        title={task ? "Edit task" : "New task"}
        widthClassName="max-w-lg"
        footer={
          <>
            {task && (
              <Button
                variant="danger"
                onClick={() => setConfirmDelete(true)}
                disabled={busy}
                className="mr-auto"
              >
                Delete task
              </Button>
            )}
            <Button variant="tertiary" onClick={onClose} disabled={busy}>
              Cancel
            </Button>
            <Button variant="primary" onClick={() => void save()} disabled={busy || !title.trim()}>
              {busy ? "Saving…" : task ? "Save" : "Add task"}
            </Button>
          </>
        }
      >
        <div className="mt-4 flex flex-col gap-4">
          <div>
            <label htmlFor={ids.title} className={fieldLabel}>
              Title
            </label>
            <Input
              id={ids.title}
              value={title}
              maxLength={TITLE_MAX}
              required
              data-autofocus
              placeholder="What needs doing?"
              onChange={(e) => setTitle(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void save();
                }
              }}
            />
          </div>

          <div>
            <label htmlFor={ids.why} className={fieldLabel}>
              Why (optional)
            </label>
            <Textarea
              id={ids.why}
              value={why}
              rows={2}
              maxLength={WHY_MAX}
              placeholder="A note for later you"
              onChange={(e) => setWhy(e.target.value)}
            />
          </div>

          <fieldset className="flex flex-col gap-3 rounded-[var(--radius-sm)] border border-border p-3">
            <legend className="px-1 text-xs text-ink3">Links (all optional)</legend>
            {!fixedProject && (
              <div>
                <label htmlFor={ids.project} className={fieldLabel}>
                  Project
                </label>
                <div className="flex items-center gap-2">
                  <Input
                    id={ids.project}
                    list={ids.projectList}
                    value={project}
                    placeholder="No project"
                    onChange={(e) => setProject(e.target.value)}
                    onBlur={settleProject}
                    className="h-8 min-w-0 flex-1 text-xs"
                  />
                  <datalist id={ids.projectList}>
                    {projects.map((p) => (
                      <option key={p} value={p} />
                    ))}
                  </datalist>
                  {project && (
                    <Button
                      variant="tertiary"
                      size="xs"
                      onClick={clearProject}
                      aria-label="Clear the project"
                      className="shrink-0"
                    >
                      Clear
                    </Button>
                  )}
                </div>
              </div>
            )}

            <div>
              <label htmlFor={ids.milestone} className={fieldLabel}>
                Milestone
              </label>
              <div className="flex items-center gap-2">
                <Select
                  id={ids.milestone}
                  compact
                  value={milestoneId === null ? "" : String(milestoneId)}
                  onChange={(e) => chooseMilestone(e.target.value)}
                  className="min-w-0 flex-1"
                >
                  <option value="">None</option>
                  {milestoneId !== null && !linkedMilestone && (
                    <option value={String(milestoneId)}>
                      {task?.milestone_label ?? "Linked milestone"}
                    </option>
                  )}
                  {milestoneOptions.map((m) => (
                    <option key={m.id} value={String(m.id)}>
                      {milestoneText(m)}
                    </option>
                  ))}
                </Select>
                {milestoneId !== null && (
                  <Button
                    variant="tertiary"
                    size="xs"
                    onClick={clearMilestone}
                    aria-label="Clear the milestone"
                    className="shrink-0"
                  >
                    Clear
                  </Button>
                )}
              </div>
              {milestones !== null && milestoneOptions.length === 0 && (
                <p className="mt-1 text-xs text-ink4">
                  {scope ? `${scope} has no milestones yet.` : "No project has milestones yet."}
                </p>
              )}
            </div>

            <div>
              <span id={ids.event} className={fieldLabel}>
                Calendar event
              </span>
              <EventPicker
                labelledBy={ids.event}
                value={eventUid}
                choices={choices ?? []}
                loading={choices === null}
                linkedTitle={task?.event_uid === eventUid ? task?.event_title : null}
                pinnedDate={pickerDate}
                onChange={changeEvent}
              />
            </div>
          </fieldset>

          <div>
            <span id={ids.deadlineFrom} className={fieldLabel}>
              Deadline
            </span>
            <SegmentedControl
              aria-labelledby={ids.deadlineFrom}
              value={deadlineFrom}
              onChange={chooseDeadlineFrom}
              options={[
                { value: "date", label: DEADLINE_FROM_LABEL.date },
                {
                  value: "milestone",
                  label: DEADLINE_FROM_LABEL.milestone,
                  disabled: milestoneId === null,
                  title: milestoneId === null ? "Link a milestone first" : undefined,
                },
                {
                  value: "event",
                  label: DEADLINE_FROM_LABEL.event,
                  disabled: eventUid === null,
                  title: eventUid === null ? "Link a calendar event first" : undefined,
                },
              ]}
            />
            {deadlineFrom === "date" ? (
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <DateField
                  id={ids.deadline}
                  value={deadline}
                  onCommit={setDeadline}
                  ariaLabel="Deadline date"
                  wrapperClassName="min-w-0 flex-1 basis-32"
                  className="h-8 px-2 text-xs"
                />
                <SegmentedControl
                  ariaLabel="How firm the deadline is"
                  value={deadlineKind}
                  onChange={setDeadlineKind}
                  options={[
                    { value: "hard", label: "Hard deadline" },
                    {
                      value: "soft",
                      label: "Aim for",
                      title: "A date to aim for, not a hard stop",
                    },
                  ]}
                />
              </div>
            ) : (
              <p className="mt-2 text-xs text-ink2">{linkedDeadline}</p>
            )}
          </div>

          <div>
            <span className={fieldLabel}>When I'll do it (optional)</span>
            <div className="flex flex-wrap items-center gap-2">
              <label htmlFor={ids.doStart} className="text-xs text-ink4">
                From
              </label>
              <DateField
                id={ids.doStart}
                value={doStart}
                onCommit={changeDoStart}
                ariaLabel="Start doing it on"
                wrapperClassName="min-w-0 flex-1 basis-28"
                className="h-8 px-2 text-xs"
              />
              <label htmlFor={ids.doEnd} className="text-xs text-ink4">
                to
              </label>
              <DateField
                id={ids.doEnd}
                value={doEnd}
                onCommit={setDoEnd}
                disabled={!doStart}
                min={doStart || undefined}
                ariaLabel="Finish doing it by"
                title={doStart ? undefined : "Pick a start day first"}
                wrapperClassName="min-w-0 flex-1 basis-28"
                className="h-8 px-2 text-xs"
              />
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <div className="flex items-center gap-2">
              <span id={ids.lane} className="text-xs text-ink3">
                Lane
              </span>
              <SegmentedControl
                aria-labelledby={ids.lane}
                value={lane}
                onChange={setLane}
                options={[
                  { value: "foreground", label: "Foreground" },
                  {
                    value: "background",
                    label: "Background",
                    title: "Ticks along beside your main work",
                  },
                ]}
              />
            </div>
            <div className="flex items-center gap-2">
              <span id={ids.parked} className="text-xs text-ink3">
                Parked
              </span>
              <Toggle
                aria-labelledby={ids.parked}
                checked={parked}
                onChange={setParked}
                title="Set aside: kept, but off the plan until you bring it back"
              />
            </div>
          </div>

          <div>
            <label htmlFor={ids.stack} className={fieldLabel}>
              Stack
            </label>
            <div className="flex items-center gap-2">
              <Select
                id={ids.stack}
                compact
                value={stackSel}
                onChange={(e) => setStackSel(e.target.value)}
                className="min-w-0 flex-1"
              >
                <option value="">No stack</option>
                {stacks.map((s) => (
                  <option key={s.id} value={String(s.id)}>
                    {s.title || STACK_KIND_LABEL[s.kind]}
                  </option>
                ))}
                <option value={NEW_STACK}>New stack…</option>
              </Select>
              {selectedStack && (
                <Button
                  variant="tertiary"
                  size="xs"
                  onClick={() => setEditingStack(selectedStack)}
                  aria-label="Edit the stack"
                  title="Rename it, change how its tasks relate, edit its note or delete it"
                  className="shrink-0"
                >
                  Edit
                </Button>
              )}
            </div>
            {selectedStack && (
              <p className="mt-1 whitespace-pre-wrap break-words text-xs text-ink4">
                {STACK_KIND_LABEL[selectedStack.kind]}
                {selectedStack.note ? ` · ${selectedStack.note}` : ""}
              </p>
            )}
            {stackSel === NEW_STACK && (
              <div className="mt-2 flex flex-col gap-2 rounded-[var(--radius-sm)] border border-dashed border-border p-2">
                <div>
                  <label htmlFor={ids.stackTitle} className={fieldLabel}>
                    Stack name
                  </label>
                  <Input
                    id={ids.stackTitle}
                    value={newStackTitle}
                    maxLength={TITLE_MAX}
                    placeholder="e.g. Visa paperwork"
                    onChange={(e) => setNewStackTitle(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
                <div>
                  <label htmlFor={ids.stackKind} className={fieldLabel}>
                    How its tasks relate
                  </label>
                  <Select
                    id={ids.stackKind}
                    compact
                    value={newStackKind}
                    onChange={(e) => setNewStackKind(e.target.value as TaskStackKind)}
                    className="w-full"
                  >
                    {STACK_KINDS.map((k) => (
                      <option key={k} value={k}>
                        {STACK_KIND_LABEL[k]}
                      </option>
                    ))}
                  </Select>
                </div>
                <div>
                  <label htmlFor={ids.stackNote} className={fieldLabel}>
                    Note (optional)
                  </label>
                  <Input
                    id={ids.stackNote}
                    value={newStackNote}
                    maxLength={WHY_MAX}
                    onChange={(e) => setNewStackNote(e.target.value)}
                    className="h-8 text-xs"
                  />
                </div>
              </div>
            )}
          </div>

          {task && (
            <div>
              <label htmlFor={ids.status} className={fieldLabel}>
                Status
              </label>
              <Select
                id={ids.status}
                compact
                value={status}
                onChange={(e) => setStatus(e.target.value as TaskStatus)}
              >
                {STATUSES.map((s) => (
                  <option key={s} value={s}>
                    {TASK_STATUS_LABEL[s]}
                  </option>
                ))}
              </Select>
            </div>
          )}

          {error && <Callout as="p">{error}</Callout>}
        </div>
        {/* The confirm and the stack editor sit inside this dialog, not beside it: a dialog is the
            topmost one when no open dialog is its DOM descendant (useDialogLayer), so a sibling
            would be topmost too and one Escape would close both, losing this editor's edits. */}
        <ConfirmDialog
          open={confirmDelete}
          title="Delete this task?"
          confirmLabel="Delete"
          danger
          busy={busy}
          onConfirm={() => void remove()}
          onClose={() => setConfirmDelete(false)}
        >
          “{task?.title}” goes from every list it shows on. Its project, milestone and event stay as
          they are.
        </ConfirmDialog>
        <StackEditor
          stack={editingStack}
          onClose={() => setEditingStack(null)}
          onSaved={(saved) => setStacks((all) => all.map((s) => (s.id === saved.id ? saved : s)))}
          onDeleted={(id) => {
            setStacks((all) => all.filter((s) => s.id !== id));
            // The backend has already taken this task out of the deleted stack.
            if (stackSel === String(id)) setStackSel("");
          }}
        />
      </Dialog>
    </>
  );
}
