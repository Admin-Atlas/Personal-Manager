// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useMemo, useState } from "react";
import type { CalendarEvent, Milestone } from "../lib/types";
import {
  addMilestone,
  deleteMilestone,
  reorderMilestones,
  setMilestoneEvent,
  setMilestoneState,
  updateMilestone,
} from "../lib/ipc";
import { formatDateOnly } from "../lib/format";
import { runMutation } from "../lib/runMutation";
import { Button, Input, Select } from "./ui";

interface Props {
  project: string;
  /** The project's milestones, in the order the parent wants them shown (controlled). */
  milestones: Milestone[];
  /** Calendar events (those with a uid) for the link picker; empty hides linking. Deduped by
   *  uid and date-ordered here, so the parent can pass the whole mirror. */
  calendarEvents?: CalendarEvent[];
  /** Called after any mutation so the parent refetches and reflects the new state. */
  onChanged: () => void;
  /** Read-only summary (no edit controls) — for lower depth / display surfaces. */
  readOnly?: boolean;
  /** Manual reorder mode: show the per-row ↑/↓ arrows. False under an active sort, where reordering
   *  would persist a wrong `sort_order` (onMove swaps by array index). Defaults to true so callers
   *  that don't sort (e.g. FocusView's project-edit form) keep the arrows. */
  manualOrder?: boolean;
}

/** A project's milestones (multi-deadline, card 7): add / edit / remove / reorder, mark
 *  met, and link a milestone to a calendar event (its date then syncs read-only). The
 *  focus view's single status is derived from the nearest unmet one. Controlled by the
 *  parent: each mutation persists then calls `onChanged` to refetch. */
export function MilestoneList({
  project,
  milestones,
  calendarEvents = [],
  onChanged,
  readOnly = false,
  manualOrder = true,
}: Props) {
  // A single error line for the whole list: any row's failed mutation surfaces here instead
  // of silently no-opping (F3-8). Declared before the read-only early return to keep the hook
  // order stable.
  const [error, setError] = useState<string | null>(null);
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
          events={calendarEvents}
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
      {milestones.map((m) => (
        <li key={m.id} className={m.state === "met" ? "line-through opacity-60" : ""}>
          {m.label}
          {m.due_date ? ` · ${formatDateOnly(m.due_date.slice(0, 10))}` : ""}
          {m.calendar_linked ? " 📅" : ""}
        </li>
      ))}
    </ul>
  );
}

function MilestoneRow({
  m,
  isFirst,
  isLast,
  events,
  onChanged,
  onError,
  onMove,
}: {
  m: Milestone;
  isFirst: boolean;
  isLast: boolean;
  events: CalendarEvent[];
  onChanged: () => void;
  onError: (message: string | null) => void;
  /** Reorder within the list. Omitted (undefined) hides the ↑/↓ arrows — see `manualOrder`. */
  onMove?: (dir: -1 | 1) => void;
}) {
  const [label, setLabel] = useState(m.label);
  const [date, setDate] = useState(m.due_date?.slice(0, 10) ?? "");
  const met = m.state === "met";
  // Linkable targets: events with a uid (the link re-resolves its date by uid each sync, so a
  // uid is required — milestones.rs). The parent can hand us the whole calendar mirror, which
  // repeats a recurring series and mirrors an event across calendars, so dedup by uid (keep the
  // soonest) and show them earliest→latest for a clean, chronological picker.
  const linkable = useMemo(() => {
    const byUid = new Map<string, CalendarEvent>();
    for (const e of events) {
      if (!e.uid) continue;
      const prev = byUid.get(e.uid);
      if (!prev || e.start.localeCompare(prev.start) < 0) byUid.set(e.uid, e);
    }
    return [...byUid.values()].sort((a, b) => a.start.localeCompare(b.start));
  }, [events]);

  // Persist label (+ PM-native date) on blur, skipping a no-op so we don't refetch needlessly.
  async function persist() {
    const nextLabel = label.trim() || "deadline";
    const nextDate = m.calendar_linked ? null : date || null;
    const curDate = m.calendar_linked ? null : (m.due_date?.slice(0, 10) ?? null);
    if (nextLabel === m.label && nextDate === curDate) return;
    await runMutation(async () => {
      await updateMilestone(m.id, nextLabel, nextDate);
      onChanged();
    }, onError);
  }

  async function link(uid: string) {
    const ev = events.find((e) => e.uid === uid);
    await runMutation(async () => {
      await setMilestoneEvent(m.id, uid, ev?.start.slice(0, 10) ?? null);
      onChanged();
    }, onError);
  }

  // Two explicit lines so the row never has to side-scroll, however narrow the panel:
  // line 1 is the checkbox + label + Done + reorder/remove; line 2 is the date (or the
  // synced calendar date) + the link picker. Every text field is `min-w-0 flex-1` so it
  // shrinks rather than forcing horizontal overflow.
  return (
    <div
      className={`rounded-[var(--radius-sm)] border border-border bg-surface px-2.5 py-1.5 ${
        met ? "opacity-70" : ""
      }`}
    >
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={met}
          title={met ? "Mark not done" : "Mark done"}
          onChange={() =>
            void runMutation(async () => {
              await setMilestoneState(m.id, !met);
              onChanged();
            }, onError)
          }
          className="shrink-0 accent-[var(--accent)]"
        />
        <Input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          onBlur={persist}
          placeholder="label"
          className={`h-7 min-w-0 flex-1 text-xs ${met ? "line-through" : ""}`}
        />
        {met && <DonePill />}
        <div className="flex shrink-0 items-center">
          {onMove && (
            <>
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
            </>
          )}
          <Button
            variant="tertiary"
            onClick={() =>
              void runMutation(async () => {
                await deleteMilestone(m.id);
                onChanged();
              }, onError)
            }
            title="Remove milestone"
            className="px-1.5 py-0.5 hover:text-st-blocked"
          >
            ×
          </Button>
        </div>
      </div>

      <div className="mt-1.5 flex items-center gap-2 pl-6">
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
              title="Unlink from calendar (date becomes editable)"
              className="shrink-0 px-1 py-0.5 text-[10px]"
            >
              Unlink
            </Button>
          </span>
        ) : (
          <>
            <Input
              type="date"
              value={date}
              onChange={(e) => setDate(e.target.value)}
              onBlur={persist}
              className={`h-7 min-w-0 flex-1 text-xs ${met ? "line-through" : ""}`}
            />
            {linkable.length > 0 && (
              <Select
                value=""
                onChange={(e) => e.target.value && void link(e.target.value)}
                title="Link to a calendar event — its date will sync automatically"
                className="h-7 w-7 shrink-0 px-1 text-xs"
              >
                <option value="">📅</option>
                {linkable.map((e) => (
                  <option key={e.id} value={e.uid!}>
                    {e.summary} · {formatDateOnly(e.start.slice(0, 10))}
                  </option>
                ))}
              </Select>
            )}
          </>
        )}
      </div>
    </div>
  );
}

/** A calm "ticked off" tag, shown on a met milestone next to its struck-through label. */
function DonePill() {
  return (
    <span
      className="shrink-0 rounded-full px-1.5 py-0.5 text-[10px] font-medium"
      style={{
        color: "var(--st-track)",
        background: "color-mix(in oklab, var(--st-track) 16%, transparent)",
      }}
    >
      Done
    </span>
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
      <Input
        type="date"
        value={date}
        onChange={(e) => setDate(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && void add()}
        className="h-7 min-w-0 flex-1 basis-28 text-xs"
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
