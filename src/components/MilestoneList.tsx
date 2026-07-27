// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState, type Ref } from "react";
import type { Milestone, MilestoneStatus } from "../lib/types";
import {
  addMilestone,
  deleteMilestone,
  reorderMilestones,
  setMilestoneEvent,
  setMilestoneStatus,
  updateMilestone,
} from "../lib/ipc";
import { formatDateOnly } from "../lib/format";
import { runMutation } from "../lib/runMutation";
import { DateField } from "./DateField";
import { Button, Input, Select } from "./ui";

/** Display order + labels for the progress control, coarsest-first (mirrors `milestones::STATUSES`). */
export const MILESTONE_STATUSES: readonly { value: MilestoneStatus; label: string }[] = [
  { value: "not_started", label: "Not started" },
  { value: "in_progress", label: "In progress" },
  { value: "almost_done", label: "Almost done" },
  { value: "done", label: "Done" },
];

/** A milestone's effective status: its stored value, or — for a row predating the status column —
 *  the one implied by whether it's been ticked off, so an old row never renders blank. */
export function milestoneStatus(m: Milestone): MilestoneStatus {
  return m.status ?? (m.state === "met" ? "done" : "not_started");
}

interface Props {
  project: string;
  /** The project's milestones, in the order the parent wants them shown (controlled). */
  milestones: Milestone[];
  /** Called after any mutation so the parent refetches and reflects the new state. */
  onChanged: () => void;
  /** Read-only summary (no edit controls) — for lower depth / display surfaces. */
  readOnly?: boolean;
  /** Manual reorder mode: show the per-row ↑/↓ arrows. False under an active sort, where reordering
   *  would persist a wrong `sort_order` (onMove swaps by array index). Defaults to true so callers
   *  that don't sort (e.g. FocusView's project-edit form) keep the arrows. */
  manualOrder?: boolean;
}

/** A project's milestones (multi-deadline, card 7): add / edit / remove / reorder, and mark
 *  met. An existing calendar-linked milestone still shows its synced date read-only and can be
 *  unlinked, but new links are no longer created here. The focus view's single status is derived
 *  from the nearest unmet one. Controlled by the parent: each mutation persists then calls
 *  `onChanged` to refetch. */
export function MilestoneList({
  project,
  milestones,
  onChanged,
  readOnly = false,
  manualOrder = true,
}: Props) {
  // A single error line for the whole list: any row's failed mutation surfaces here instead
  // of silently no-opping (F3-8). Declared before the read-only early return to keep the hook
  // order stable.
  const [error, setError] = useState<string | null>(null);
  // Scroll the first not-yet-completed milestone to the top of the panel once per project, so the
  // list opens on what's next with any completed ones tucked above (scroll up to see them) — only
  // when there are completed rows before it. Hooks run before the read-only early return to keep the
  // hook order stable; there the ref simply isn't attached, so the effect no-ops.
  const firstUnmetIdx = milestones.findIndex((m) => m.state !== "met");
  const anchorRef = useRef<HTMLDivElement | null>(null);
  const scrolledForRef = useRef<string | null>(null);
  useEffect(() => {
    if (milestones.length === 0 || scrolledForRef.current === project) return;
    scrolledForRef.current = project;
    if (firstUnmetIdx > 0) anchorRef.current?.scrollIntoView({ block: "start" });
  }, [project, milestones, firstUnmetIdx]);
  if (readOnly) {
    return <MilestoneSummary milestones={milestones} />;
  }

  return (
    <div className="flex flex-col gap-1.5" data-help="project-milestones">
      {milestones.length === 0 && (
        <p className="rounded-[var(--radius-sm)] border border-dashed border-border px-3 py-2 text-xs text-ink4">
          No milestones yet — add a deadline below.
        </p>
      )}
      {milestones.map((m, i) => (
        <MilestoneRow
          key={m.id}
          m={m}
          isFirst={i === 0}
          isLast={i === milestones.length - 1}
          anchorRef={i === firstUnmetIdx ? anchorRef : undefined}
          onChanged={onChanged}
          onError={setError}
          onMove={
            manualOrder
              ? (dir) => {
                  const j = i + dir;
                  if (j < 0 || j >= milestones.length) return;
                  const ids = milestones.map((x) => x.id);
                  [ids[i], ids[j]] = [ids[j], ids[i]];
                  void runMutation(async () => {
                    await reorderMilestones(project, ids);
                    onChanged();
                  }, setError);
                }
              : undefined
          }
        />
      ))}
      <AddMilestone project={project} onChanged={onChanged} onError={setError} />
      {error && <MilestoneError message={error} />}
    </div>
  );
}

/** A calm inline error line for a milestone mutation that the backend rejected. */
function MilestoneError({ message }: { message: string }) {
  return (
    <p
      role="alert"
      className="rounded-[var(--radius-sm)] border px-3 py-2 text-xs text-st-due"
      style={{
        borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
        background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
      }}
    >
      {message}
    </p>
  );
}

/** A calm read-only line per milestone — used at lower depth where editing is hidden. */
function MilestoneSummary({ milestones }: { milestones: Milestone[] }) {
  if (milestones.length === 0) return null;
  return (
    <ul className="flex flex-col gap-0.5 font-mono text-xs text-ink4">
      {milestones.map((m) => {
        // Only the in-between values add anything here: "not started" is the default and "done" is
        // already carried by the strike-through, so naming either would just be noise.
        const status = milestoneStatus(m);
        const progress =
          status === "in_progress" || status === "almost_done"
            ? MILESTONE_STATUSES.find((s) => s.value === status)?.label
            : null;
        return (
          <li key={m.id} className={m.state === "met" ? "line-through opacity-60" : ""}>
            {m.label}
            {m.due_date ? ` · ${formatDateOnly(m.due_date.slice(0, 10))}` : ""}
            {progress ? ` · ${progress}` : ""}
            {m.calendar_linked ? " 📅" : ""}
          </li>
        );
      })}
    </ul>
  );
}

function MilestoneRow({
  m,
  isFirst,
  isLast,
  anchorRef,
  onChanged,
  onError,
  onMove,
}: {
  m: Milestone;
  isFirst: boolean;
  isLast: boolean;
  /** Attached to the first not-yet-completed row so the list can scroll it into view on open. */
  anchorRef?: Ref<HTMLDivElement>;
  onChanged: () => void;
  onError: (message: string | null) => void;
  /** Reorder within the list. Omitted (undefined) hides the ↑/↓ arrows — see `manualOrder`. */
  onMove?: (dir: -1 | 1) => void;
}) {
  const [label, setLabel] = useState(m.label);
  const [date, setDate] = useState(m.due_date?.slice(0, 10) ?? "");
  const met = m.state === "met";
  const status = milestoneStatus(m);

  // Persist label (+ PM-native date) on blur, skipping a no-op so we don't refetch needlessly.
  // `dateOverride` is how DateField commits: it hands us the new value directly, because reading
  // `date` here right after a `setDate` in the same tick would see the PREVIOUS render's value and
  // silently persist the old day.
  async function persist(dateOverride?: string) {
    const nextLabel = label.trim() || "deadline";
    const effectiveDate = dateOverride ?? date;
    const nextDate = m.calendar_linked ? null : effectiveDate || null;
    const curDate = m.calendar_linked ? null : (m.due_date?.slice(0, 10) ?? null);
    if (nextLabel === m.label && nextDate === curDate) return;
    await runMutation(async () => {
      await updateMilestone(m.id, nextLabel, nextDate);
      onChanged();
    }, onError);
  }

  // Two explicit lines so the row never has to side-scroll, however narrow the panel: line 1 is
  // the label + remove ×; line 2 is the date (or the synced calendar date) plus the progress
  // dropdown and the ↑/↓ reorder arrows — so line 1 leaves the label its full width. Every text
  // field is `min-w-0 flex-1` so it shrinks rather than forcing horizontal overflow.
  //
  // There is deliberately no done tick-box beside the dropdown. It and the dropdown's "Done" were
  // two controls writing the same fact, which meant two things to keep in step and two places to
  // look to learn one answer. The dropdown is the single writer; `set_milestone_status` carries
  // `state` along with it (and reopens the flag when a milestone moves OFF done, exactly as
  // un-ticking used to), so nothing downstream can tell the difference.
  return (
    <div
      ref={anchorRef}
      className={`rounded-[var(--radius-sm)] border border-border bg-surface px-2.5 py-1.5 ${
        met ? "opacity-70" : ""
      }`}
    >
      <div className="flex items-center gap-2">
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          onBlur={() => void persist()}
          placeholder="label"
          className={`h-7 min-w-0 flex-1 text-xs ${met ? "line-through" : ""}`}
        />
        <Button
          variant="tertiary"
          onClick={() =>
            void runMutation(async () => {
              await deleteMilestone(m.id);
              onChanged();
            }, onError)
          }
          title="Remove milestone"
          className="shrink-0 px-1.5 py-0.5 hover:text-st-blocked"
        >
          ×
        </Button>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-2 pl-6">
        {m.calendar_linked ? (
          <span
            className={`flex min-w-0 flex-1 items-center gap-1 text-xs text-accent-text ${
              met ? "line-through" : ""
            }`}
            title={
              m.event_missing
                ? "Linked event not found in your synced calendars"
                : "Synced from calendar"
            }
          >
            <span className="truncate">
              📅 {m.due_date ? formatDateOnly(m.due_date.slice(0, 10)) : "—"}
            </span>
            {m.event_missing && <span className="shrink-0 text-st-due">⚠</span>}
            <Button
              variant="tertiary"
              onClick={() =>
                void runMutation(async () => {
                  await setMilestoneEvent(m.id, null, m.due_date?.slice(0, 10) ?? null);
                  onChanged();
                }, onError)
              }
              title="Unlink from calendar — the date becomes editable, but it can't be re-linked"
              className="shrink-0 px-1 py-0.5 text-[0.625rem]"
            >
              Unlink
            </Button>
          </span>
        ) : (
          <DateField
            value={date}
            onCommit={(iso) => {
              setDate(iso);
              void persist(iso);
            }}
            ariaLabel="Milestone deadline"
            wrapperClassName="min-w-0 flex-1"
            className={`h-7 px-1.5 text-xs ${met ? "line-through" : ""}`}
          />
        )}
        <Select
          compact
          value={status}
          aria-label="Milestone progress"
          title="How far along this milestone is"
          onChange={(e) =>
            void runMutation(async () => {
              await setMilestoneStatus(m.id, e.target.value as MilestoneStatus);
              onChanged();
            }, onError)
          }
          className="shrink-0"
        >
          {MILESTONE_STATUSES.map((s) => (
            <option key={s.value} value={s.value}>
              {s.label}
            </option>
          ))}
        </Select>
        {onMove && (
          <div className="flex shrink-0 items-center">
            <Button
              variant="tertiary"
              onClick={() => onMove(-1)}
              disabled={isFirst}
              title="Move up"
              className="px-1.5 py-0.5 disabled:opacity-30"
            >
              ↑
            </Button>
            <Button
              variant="tertiary"
              onClick={() => onMove(1)}
              disabled={isLast}
              title="Move down"
              className="px-1.5 py-0.5 disabled:opacity-30"
            >
              ↓
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}

/** The "add a milestone" row at the foot of the list. */
function AddMilestone({
  project,
  onChanged,
  onError,
}: {
  project: string;
  onChanged: () => void;
  onError: (message: string | null) => void;
}) {
  const [label, setLabel] = useState("");
  const [date, setDate] = useState("");
  const [busy, setBusy] = useState(false);

  async function add() {
    if (!label.trim() && !date) return;
    setBusy(true);
    const ok = await runMutation(
      () => addMilestone(project, label.trim() || "deadline", date || null, null),
      onError,
    );
    if (ok) {
      setLabel("");
      setDate("");
      onChanged();
    }
    setBusy(false);
  }

  // flex-wrap + min-w-0 basis: the name and date share the row when there's room and the
  // date drops to its own line when the panel is narrow — never a fixed width that overflows.
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && void add()}
        placeholder="New milestone (e.g. pitch)"
        className="h-7 min-w-0 flex-1 basis-28 text-xs"
      />
      <DateField
        value={date}
        onCommit={setDate}
        ariaLabel="New milestone deadline"
        wrapperClassName="min-w-0 flex-1 basis-28"
        className="h-7 px-1.5 text-xs"
      />
      <Button
        variant="secondary"
        onClick={add}
        disabled={busy || (!label.trim() && !date)}
        className="shrink-0 px-2 py-0.5 text-xs"
      >
        Add
      </Button>
    </div>
  );
}
