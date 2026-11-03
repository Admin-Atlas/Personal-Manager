// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pieces both Runways draw alike (PA-2): the header strip, the Move… menu (the keyboard's way
// to do what a drag does, and the Terminal's only way) and the dialog that adds, edits or deletes a
// named window. The hooks and words they share are in `runwayLogic.ts`.

import { useId, useState } from "react";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { dayKey } from "../../../lib/calendar-layout";
import {
  dropPlan,
  isDropColumn,
  type RunwayColumn,
  type RunwayModel,
  type TaskPlan,
} from "../../../lib/runway";
import { TONE_CLASS } from "../../../lib/tasks";
import type { Task, TaskLane } from "../../../lib/types";
import { useDepth } from "../../../theme";
import { DateField } from "../../DateField";
import { Button, Callout, ConfirmDialog, Dialog, Input, Popover, Progress, cn } from "../../ui";
import { newWindowId, placeName } from "./runwayLogic";

/** The header strip: how far along, what's next, and the two add buttons. */
export function RunwayStrip({
  model,
  onAddTask,
  onAddWindow,
  terminal = false,
}: {
  model: RunwayModel;
  onAddTask: () => void;
  onAddWindow: () => void;
  terminal?: boolean;
}) {
  const { showMeta } = useDepth();
  const { done, total, doing } = model.progress;
  const words = `${done}/${total} done${doing > 0 ? ` · ${doing} in progress` : ""}`;
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-rule px-4 py-2 text-xs",
        terminal && "font-mono",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="text-ink2">{words}</span>
        {showMeta && !terminal && total > 0 && (
          <Progress
            value={done / total}
            label="Done so far"
            valueText={`${done} of ${total} done`}
            className="w-24"
          />
        )}
      </div>
      {model.nextUp && (
        <span className="min-w-0 truncate text-ink3">
          Next up · <span className="text-ink">{model.nextUp.title}</span> ·{" "}
          <span className={TONE_CLASS[model.nextUp.tone]}>{model.nextUp.deadlineText}</span>
        </span>
      )}
      <div className="ml-auto flex items-center gap-2">
        <Button variant="secondary" size="sm" onClick={onAddTask}>
          + Task
        </Button>
        <Button variant="tertiary" size="sm" onClick={onAddWindow} title="Name a stretch of days">
          + Window
        </Button>
      </div>
    </div>
  );
}

/** The Move… menu: the columns a task can go to, then the other lane, then parking. For a stack, it
 *  moves all of its tasks. */
export function MoveMenu({
  name,
  tasks,
  columns,
  here,
  onReplan,
  terminal = false,
}: {
  /** What's being moved, for the trigger's name ("Move Draft the deck"). */
  name: string;
  tasks: Task[];
  columns: RunwayColumn[];
  /** The column the card is in (null when parked). */
  here: string | null;
  onReplan: (planFor: (t: Task) => TaskPlan | null, where: string) => void;
  terminal?: boolean;
}) {
  const first = tasks[0];
  if (!first) return null;
  const lane = first.lane;
  const parked = first.parked;
  const other: TaskLane = lane === "foreground" ? "background" : "foreground";
  const item =
    "block w-full rounded-[var(--radius-sm)] px-2 py-1 text-left text-xs text-ink2 hover:bg-surface hover:text-ink disabled:cursor-default disabled:text-ink4 disabled:hover:bg-transparent";
  return (
    <Popover
      escapeClipping
      align="right"
      ariaLabel={`Move ${name}`}
      trigger={({ open, toggle }) => (
        <Button
          variant="tertiary"
          size="xs"
          aria-expanded={open}
          aria-haspopup="true"
          aria-label={`Move ${name}`}
          title="Move to another day, lane or the parked shelf"
          onClick={toggle}
          className={terminal ? "font-mono" : undefined}
        >
          Move…
        </Button>
      )}
    >
      {({ close }) => {
        const pick = (planFor: (t: Task) => TaskPlan | null, where: string) => {
          close();
          onReplan(planFor, where);
        };
        return (
          <div className={cn("flex flex-col", terminal && "font-mono")}>
            <p className="px-2 pb-1 pt-0.5 text-[0.6875rem] text-ink4">Move to</p>
            {columns.filter(isDropColumn).map((c) => {
              const current = !parked && c.id === here;
              return (
                <button
                  key={c.id}
                  type="button"
                  className={item}
                  disabled={current}
                  aria-current={current ? "true" : undefined}
                  onClick={() =>
                    pick(
                      (t) => dropPlan({ kind: "column", column: c, lane: t.lane }, t),
                      placeName(c, lane),
                    )
                  }
                >
                  {c.label}
                  <span className="ml-1 text-ink4">{c.sub}</span>
                </button>
              );
            })}
            <div className="my-1 border-t border-rule" />
            <button
              type="button"
              className={item}
              onClick={() =>
                pick(
                  (t) => ({
                    do_start: t.do_start,
                    do_end: t.do_end,
                    lane: other,
                    parked: t.parked,
                  }),
                  other === "background" ? "the background lane" : "the foreground lane",
                )
              }
            >
              {other === "background" ? "Background lane" : "Foreground lane"}
            </button>
            <button
              type="button"
              className={item}
              onClick={() =>
                pick(
                  (t) => ({
                    do_start: t.do_start,
                    do_end: t.do_end,
                    lane: t.lane,
                    parked: !parked,
                  }),
                  parked ? "the plan" : "Parked",
                )
              }
            >
              {parked ? "Bring back from Parked" : "Park it"}
            </button>
          </div>
        );
      }}
    </Popover>
  );
}

/** Add, rename, re-date or delete a named window. `window` absent adds one. */
export function WindowDialog({
  open,
  window: w,
  today,
  windows,
  onWindowsChange,
  onClose,
}: {
  open: boolean;
  window?: RunwayWindow | null;
  today: Date;
  windows: RunwayWindow[];
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onClose: () => void;
}) {
  if (!open) return null;
  return (
    <WindowDialogBody
      key={w?.id ?? "new"}
      window={w ?? null}
      today={today}
      windows={windows}
      onWindowsChange={onWindowsChange}
      onClose={onClose}
    />
  );
}

function WindowDialogBody({
  window: w,
  today,
  windows,
  onWindowsChange,
  onClose,
}: {
  window: RunwayWindow | null;
  today: Date;
  windows: RunwayWindow[];
  onWindowsChange: (windows: RunwayWindow[]) => void;
  onClose: () => void;
}) {
  const ids = { label: useId(), start: useId(), end: useId() };
  const [label, setLabel] = useState(w?.label ?? "");
  const [start, setStart] = useState(w?.start ?? dayKey(today));
  const [end, setEnd] = useState(w?.end ?? "");
  const [tried, setTried] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const problem = !label.trim()
    ? "Give the window a name."
    : !start || !end
      ? "Pick its first and last day."
      : end < start
        ? "Its last day can't be before its first."
        : null;

  function save() {
    setTried(true);
    if (problem) return;
    const next: RunwayWindow = { id: w?.id ?? newWindowId(), label: label.trim(), start, end };
    onWindowsChange(w ? windows.map((x) => (x.id === w.id ? next : x)) : [...windows, next]);
    onClose();
  }

  const fieldLabel = "mb-1 block text-xs text-ink3";
  return (
    <>
      <Dialog
        open
        onClose={onClose}
        title={w ? "Edit window" : "New window"}
        subtitle="A stretch of days with a name of its own (a trip, a sprint), shown as its own column."
        widthClassName="max-w-sm"
        footer={
          <>
            {w && (
              <Button variant="danger" onClick={() => setConfirmDelete(true)} className="mr-auto">
                Delete window
              </Button>
            )}
            <Button variant="tertiary" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={save}>
              {w ? "Save" : "Add window"}
            </Button>
          </>
        }
      >
        <div className="mt-4 flex flex-col gap-3">
          <div>
            <label htmlFor={ids.label} className={fieldLabel}>
              Name
            </label>
            <Input
              id={ids.label}
              value={label}
              maxLength={80}
              data-autofocus
              placeholder="e.g. Lisbon trip"
              onChange={(e) => setLabel(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  save();
                }
              }}
            />
          </div>
          <div className="flex flex-wrap items-end gap-2">
            <div className="min-w-0 flex-1 basis-28">
              <label htmlFor={ids.start} className={fieldLabel}>
                First day
              </label>
              <DateField
                id={ids.start}
                value={start}
                onCommit={(iso) => {
                  setStart(iso);
                  if (iso && end && end < iso) setEnd("");
                }}
                ariaLabel="First day"
                className="h-8 px-2 text-xs"
              />
            </div>
            <div className="min-w-0 flex-1 basis-28">
              <label htmlFor={ids.end} className={fieldLabel}>
                Last day
              </label>
              <DateField
                id={ids.end}
                value={end}
                onCommit={setEnd}
                min={start || undefined}
                ariaLabel="Last day"
                className="h-8 px-2 text-xs"
              />
            </div>
          </div>
          {tried && problem && <Callout as="p">{problem}</Callout>}
        </div>
      </Dialog>
      <ConfirmDialog
        open={confirmDelete}
        title="Delete this window?"
        confirmLabel="Delete"
        danger
        onConfirm={() => {
          setConfirmDelete(false);
          if (w) onWindowsChange(windows.filter((x) => x.id !== w.id));
          onClose();
        }}
        onClose={() => setConfirmDelete(false)}
      >
        “{w?.label}” goes from the Runway. Its tasks keep their days and sit in the columns those
        days fall in.
      </ConfirmDialog>
    </>
  );
}
