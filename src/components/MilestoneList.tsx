// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
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
import { Button, Input, Select } from "./ui";

interface Props {
  project: string;
  /** The project's milestones, resolved + date-ordered (controlled by the parent). */
  milestones: Milestone[];
  /** Upcoming events (those with a uid) for the link picker; empty hides linking. */
  calendarEvents?: CalendarEvent[];
  /** Called after any mutation so the parent refetches and reflects the new state. */
  onChanged: () => void;
  /** Read-only summary (no edit controls) — for lower depth / display surfaces. */
  readOnly?: boolean;
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
}: Props) {
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
          onMove={async (dir) => {
            const j = i + dir;
            if (j < 0 || j >= milestones.length) return;
            const ids = milestones.map((x) => x.id);
            [ids[i], ids[j]] = [ids[j], ids[i]];
            await reorderMilestones(project, ids);
            onChanged();
          }}
        />
      ))}
      <AddMilestone project={project} onChanged={onChanged} />
    </div>
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
  onMove,
}: {
  m: Milestone;
  isFirst: boolean;
  isLast: boolean;
  events: CalendarEvent[];
  onChanged: () => void;
  onMove: (dir: -1 | 1) => void;
}) {
  const [label, setLabel] = useState(m.label);
  const [date, setDate] = useState(m.due_date?.slice(0, 10) ?? "");
  const met = m.state === "met";
  const linkable = events.filter((e) => e.uid);

  // Persist label (+ PM-native date) on blur, skipping a no-op so we don't refetch needlessly.
  async function persist() {
    const nextLabel = label.trim() || "deadline";
    const nextDate = m.calendar_linked ? null : date || null;
    const curDate = m.calendar_linked ? null : (m.due_date?.slice(0, 10) ?? null);
    if (nextLabel === m.label && nextDate === curDate) return;
    await updateMilestone(m.id, nextLabel, nextDate);
    onChanged();
  }

  async function link(uid: string) {
    const ev = events.find((e) => e.uid === uid);
    await setMilestoneEvent(m.id, uid, ev?.start.slice(0, 10) ?? null);
    onChanged();
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
          onChange={async () => {
            await setMilestoneState(m.id, !met);
            onChanged();
          }}
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
          <Button
            variant="tertiary"
            onClick={async () => {
              await deleteMilestone(m.id);
              onChanged();
            }}
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
              onClick={async () => {
                await setMilestoneEvent(m.id, null, m.due_date?.slice(0, 10) ?? null);
                onChanged();
              }}
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
function AddMilestone({ project, onChanged }: { project: string; onChanged: () => void }) {
  const [label, setLabel] = useState("");
  const [date, setDate] = useState("");
  const [busy, setBusy] = useState(false);

  async function add() {
    if (!label.trim() && !date) return;
    setBusy(true);
    try {
      await addMilestone(project, label.trim() || "deadline", date || null, null);
      setLabel("");
      setDate("");
      onChanged();
    } finally {
      setBusy(false);
    }
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
