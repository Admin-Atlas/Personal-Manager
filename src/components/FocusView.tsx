// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import {
  calendarStatus,
  getDailyBriefing,
  listCalendarEvents,
  listProjectOverviews,
  proposeProjectMetadata,
  refreshDailyBriefing,
  setProjectMetadata,
  syncCalendar,
} from "../lib/ipc";
import type {
  CalendarEvent,
  DailyBriefing,
  ProjectOverview,
  ProjectProposal,
  ProjectSize,
  ProjectStatus,
} from "../lib/types";
import { formatDate } from "../lib/format";

interface Props {
  /** Open the per-project scoped view. */
  onOpenProject: (project: string) => void;
}

/** Badge colour + label per status (spec §4.1). `part_of` shows its parent name. */
const STATUS_META: Record<ProjectStatus, { label: string; cls: string }> = {
  due_soon: { label: "Due soon", cls: "bg-red-500/15 text-red-300 border-red-500/30" },
  blocked: { label: "Blocked", cls: "bg-rose-500/15 text-rose-300 border-rose-500/30" },
  quick_win: { label: "Quick win", cls: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30" },
  take_a_look: { label: "Take a look", cls: "bg-amber-500/15 text-amber-300 border-amber-500/30" },
  part_of: { label: "Part of", cls: "bg-sky-500/15 text-sky-300 border-sky-500/30" },
  on_track: { label: "On track", cls: "bg-neutral-700/40 text-neutral-300 border-neutral-600/50" },
};

/** Surface the most action-worthy first, mirroring the backend status precedence. */
const STATUS_ORDER: ProjectStatus[] = [
  "due_soon",
  "blocked",
  "quick_win",
  "take_a_look",
  "part_of",
  "on_track",
];

export function FocusView({ onOpenProject }: Props) {
  const [projects, setProjects] = useState<ProjectOverview[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  /** AI proposals keyed by project name, populated by "Suggest attributes". */
  const [proposals, setProposals] = useState<Record<string, ProjectProposal>>({});
  const [proposing, setProposing] = useState(false);
  /** Upcoming calendar events for the agenda (empty when not connected). */
  const [events, setEvents] = useState<CalendarEvent[]>([]);
  /** The daily briefing (Step 7); null until loaded, then refreshed when stale. */
  const [briefing, setBriefing] = useState<DailyBriefing | null>(null);
  const [briefingBusy, setBriefingBusy] = useState(false);
  // Sequence guard so a stale in-flight refresh can't overwrite a newer one — the
  // initial pre-sync load and the post-sync reload can resolve out of order.
  const refreshSeqRef = useRef(0);
  // These views fire background model calls (suggest/briefing) that can resolve
  // after the user has left; don't write state once unmounted.
  const aliveRef = useRef(true);
  useEffect(() => () => void (aliveRef.current = false), []);

  async function refresh() {
    const seq = ++refreshSeqRef.current;
    try {
      const overviews = await listProjectOverviews();
      if (aliveRef.current && seq === refreshSeqRef.current) setProjects(overviews);
    } catch (e) {
      if (aliveRef.current && seq === refreshSeqRef.current) setError(String(e));
    } finally {
      if (aliveRef.current) setLoading(false);
    }
  }

  // Regenerate the briefing from the current focus state. Best-effort: a missing key
  // or a model hiccup just leaves the previous briefing in place.
  async function regenerateBriefing() {
    setBriefingBusy(true);
    try {
      const b = await refreshDailyBriefing();
      if (aliveRef.current) setBriefing(b);
    } catch {
      /* keep whatever we have */
    } finally {
      if (aliveRef.current) setBriefingBusy(false);
    }
  }

  // Load the stored briefing, and silently regenerate it when stale (older than the
  // freshness window) so it refreshes ~once a day on open, not on every mount.
  async function loadBriefing() {
    try {
      const b = await getDailyBriefing();
      if (!aliveRef.current) return;
      setBriefing(b);
      if (b.stale) void regenerateBriefing();
    } catch {
      /* briefing is optional — focus view works without it */
    }
  }

  // On landing: load project statuses, syncing the calendar first if connected
  // (best effort) so a name-matched event can flip a project to "Due soon" (Step
  // 6). One effect with a single refresh, then today's briefing (Step 7).
  useEffect(() => {
    (async () => {
      try {
        const status = await calendarStatus();
        if (status.ics_feeds > 0 || status.oauth_connected) {
          await syncCalendar().catch(() => {});
          const evts = await listCalendarEvents();
          if (aliveRef.current) setEvents(evts);
        }
      } catch {
        /* connector optional — focus view works without it */
      }
      await refresh();
      void loadBriefing();
    })();
  }, []);

  const names = useMemo(() => projects.map((p) => p.name), [projects]);
  const sorted = useMemo(
    () =>
      [...projects].sort(
        (a, b) =>
          STATUS_ORDER.indexOf(a.status) - STATUS_ORDER.indexOf(b.status) ||
          a.name.localeCompare(b.name),
      ),
    [projects],
  );

  async function suggestAll() {
    if (proposing || projects.length === 0) return;
    setProposing(true);
    setError(null);
    try {
      await proposeProjectMetadata((event) => {
        if (!aliveRef.current) return; // view gone — drop late proposals
        if (event.type === "proposed") {
          setProposals((prev) => ({ ...prev, [event.project]: event.proposal }));
        }
      });
    } catch (e) {
      if (aliveRef.current) setError(String(e));
    } finally {
      if (aliveRef.current) setProposing(false);
    }
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-neutral-800 px-6 py-3">
        <div data-help="focus-header">
          <h1 className="text-sm font-semibold text-neutral-100">Focus</h1>
          <p className="text-xs text-neutral-500">
            Every project, one status — what to look at right now.
          </p>
        </div>
        <button
          onClick={suggestAll}
          disabled={proposing || projects.length === 0}
          data-help="focus-suggest"
          title="Let the AI propose a size, parent, blocker and deadline for each project"
          className="rounded-lg border border-neutral-700 px-3 py-1.5 text-sm text-neutral-200 hover:bg-neutral-800 disabled:opacity-40"
        >
          {proposing ? "Suggesting…" : "Suggest attributes (AI)"}
        </button>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {error && (
            <div className="mb-4 rounded-lg border border-red-900 bg-red-950/50 px-3 py-2 text-sm text-red-300">
              {error}
            </div>
          )}
          <Briefing briefing={briefing} busy={briefingBusy} onRefresh={regenerateBriefing} />
          {events.length > 0 && <Agenda events={events} />}
          {loading ? (
            <p className="text-sm text-neutral-600">Loading…</p>
          ) : projects.length === 0 ? (
            <p className="text-sm text-neutral-600">
              No projects yet. Ingest some documents and sort them in Review.
            </p>
          ) : (
            <ul className="flex flex-col gap-2" data-help="focus-cards">
              {sorted.map((p) => (
                <ProjectCard
                  key={p.name}
                  project={p}
                  otherProjects={names.filter((n) => n !== p.name)}
                  proposal={proposals[p.name]}
                  editing={editing === p.name}
                  onEdit={() => setEditing(editing === p.name ? null : p.name)}
                  onOpen={() => onOpenProject(p.name)}
                  onSaved={async () => {
                    setEditing(null);
                    await refresh();
                  }}
                />
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}

function ProjectCard({
  project,
  otherProjects,
  proposal,
  editing,
  onEdit,
  onOpen,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  proposal?: ProjectProposal;
  editing: boolean;
  onEdit: () => void;
  onOpen: () => void;
  onSaved: () => void;
}) {
  const meta = STATUS_META[project.status];
  const label = project.status === "part_of" && project.parent ? `Part of ${project.parent}` : meta.label;

  return (
    <li className="rounded-xl border border-neutral-800 bg-neutral-900/40" data-help="focus-card">
      <div className="flex items-center justify-between gap-3 px-4 py-3">
        <button onClick={onOpen} className="min-w-0 flex-1 text-left">
          <div className="flex items-center gap-2">
            <span
              className={`shrink-0 rounded-md border px-2 py-0.5 text-xs font-medium ${meta.cls}`}
              data-help="focus-status-badge"
            >
              {label}
            </span>
            <span className="truncate text-sm font-medium text-neutral-100">{project.name}</span>
          </div>
          <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-neutral-500">
            <span>
              {project.doc_count} doc{project.doc_count === 1 ? "" : "s"}
            </span>
            {project.importance && <span className="capitalize">{project.importance} importance</span>}
            {project.size && <span>{project.size}</span>}
            {project.deadline && <span>due {project.deadline.slice(0, 10)}</span>}
            {project.blocked_by && <span>blocked by {project.blocked_by}</span>}
            {project.last_activity && <span>active {formatDate(project.last_activity)}</span>}
          </div>
          {project.calendar_event && (
            <div className="mt-1 truncate text-xs text-sky-300/80">
              📅 {project.calendar_event.summary} · {formatEventWhen(project.calendar_event.start)}
            </div>
          )}
        </button>
        <button
          onClick={onEdit}
          className="shrink-0 rounded-md px-2 py-1 text-xs text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200"
        >
          {editing ? "Close" : "Triage"}
        </button>
      </div>

      {editing && (
        <MetaEditor
          project={project}
          otherProjects={otherProjects}
          proposal={proposal}
          onSaved={onSaved}
        />
      )}
    </li>
  );
}

/** Inline triage editor — the "you-confirm" half. Pre-fills from the AI proposal
 *  if one was suggested, otherwise from the project's current values. */
function MetaEditor({
  project,
  otherProjects,
  proposal,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  proposal?: ProjectProposal;
  onSaved: () => void;
}) {
  const [deadline, setDeadline] = useState(project.deadline?.slice(0, 10) ?? "");
  const [size, setSize] = useState<ProjectSize>(project.size);
  const [blockedBy, setBlockedBy] = useState(project.blocked_by ?? "");
  const [parent, setParent] = useState(project.parent ?? "");
  const [saving, setSaving] = useState(false);

  function applyProposal() {
    if (!proposal) return;
    setSize(proposal.size);
    if (proposal.deadline) setDeadline(proposal.deadline.slice(0, 10));
    if (proposal.blocked_by) setBlockedBy(proposal.blocked_by);
    if (proposal.parent) setParent(proposal.parent);
  }

  async function save() {
    setSaving(true);
    try {
      await setProjectMetadata(project.name, {
        deadline: deadline || null,
        size,
        blockedBy: blockedBy || null,
        parent: parent || null,
      });
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="border-t border-neutral-800 px-4 py-3" data-help="focus-triage">
      {proposal && (
        <div className="mb-3 rounded-lg border border-sky-900/50 bg-sky-950/30 px-3 py-2 text-xs text-sky-200">
          <div className="mb-1 flex items-center justify-between">
            <span className="font-medium">AI suggestion</span>
            <button onClick={applyProposal} className="rounded px-2 py-0.5 text-sky-300 hover:bg-sky-900/40">
              Use it
            </button>
          </div>
          <p className="text-sky-300/80">
            {proposal.size ? `size ${proposal.size}` : "no size"}
            {proposal.parent ? ` · part of ${proposal.parent}` : ""}
            {proposal.blocked_by ? ` · blocked by ${proposal.blocked_by}` : ""}
            {proposal.deadline ? ` · due ${proposal.deadline.slice(0, 10)}` : ""}
          </p>
          {proposal.reasoning && <p className="mt-1 text-sky-300/60">{proposal.reasoning}</p>}
        </div>
      )}

      <div className="grid grid-cols-2 gap-3">
        <Field label="Size (Quick win = quick)">
          <select
            value={size ?? ""}
            onChange={(e) => setSize((e.target.value || null) as ProjectSize)}
            className="w-full rounded-md border border-neutral-700 bg-neutral-900 px-2 py-1.5 text-sm text-neutral-100"
          >
            <option value="">—</option>
            <option value="quick">quick</option>
            <option value="standard">standard</option>
            <option value="large">large</option>
          </select>
        </Field>
        <Field label="Deadline (Due soon)">
          <input
            type="date"
            value={deadline}
            onChange={(e) => setDeadline(e.target.value)}
            className="w-full rounded-md border border-neutral-700 bg-neutral-900 px-2 py-1.5 text-sm text-neutral-100"
          />
        </Field>
        <Field label="Blocked by">
          <ProjectSelect value={blockedBy} options={otherProjects} onChange={setBlockedBy} />
        </Field>
        <Field label="Part of (parent)">
          <ProjectSelect value={parent} options={otherProjects} onChange={setParent} />
        </Field>
      </div>

      <div className="mt-3 flex justify-end gap-2">
        <button
          onClick={save}
          disabled={saving}
          className="rounded-lg bg-neutral-100 px-3 py-1.5 text-sm font-medium text-neutral-900 hover:bg-white disabled:opacity-40"
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-neutral-500">{label}</span>
      {children}
    </label>
  );
}

function ProjectSelect({
  value,
  options,
  onChange,
}: {
  value: string;
  options: string[];
  onChange: (v: string) => void;
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="w-full rounded-md border border-neutral-700 bg-neutral-900 px-2 py-1.5 text-sm text-neutral-100"
    >
      <option value="">—</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
    </select>
  );
}

/** The daily briefing — a short "here's your picture today" synthesis (Step 7). Hidden
 *  until there's something to show; shows a generating state while the model writes it. */
function Briefing({
  briefing,
  busy,
  onRefresh,
}: {
  briefing: DailyBriefing | null;
  busy: boolean;
  onRefresh: () => void;
}) {
  const text = briefing?.briefing.trim() ?? "";
  // Nothing yet and not generating → don't take up space (e.g. an empty store).
  if (!text && !busy) return null;

  return (
    <section
      className="mb-5 rounded-xl border border-neutral-800 bg-neutral-900/40 px-4 py-3"
      data-help="focus-briefing"
    >
      <div className="mb-2 flex items-center justify-between">
        <h2 className="text-xs font-semibold uppercase tracking-wide text-neutral-500">Today</h2>
        <button
          onClick={onRefresh}
          disabled={busy}
          title="Regenerate today's briefing from your current projects and calendar"
          className="rounded-md px-2 py-0.5 text-xs text-neutral-500 hover:bg-neutral-800 hover:text-neutral-300 disabled:opacity-40"
        >
          {busy ? "Refreshing…" : "Refresh"}
        </button>
      </div>
      {text ? (
        <div className="whitespace-pre-wrap text-sm leading-relaxed text-neutral-200">{text}</div>
      ) : (
        <p className="text-sm text-neutral-600">Putting together your briefing…</p>
      )}
      {text && briefing?.updated_at && (
        <p className="mt-2 text-xs text-neutral-600">Updated {formatDate(briefing.updated_at)}</p>
      )}
    </section>
  );
}

/** The "Upcoming" agenda — the next handful of events from connected calendars. */
function Agenda({ events }: { events: CalendarEvent[] }) {
  const shown = events.slice(0, 8);
  return (
    <section className="mb-5 rounded-xl border border-neutral-800 bg-neutral-900/40 px-4 py-3" data-help="focus-agenda">
      <h2 className="mb-2 text-xs font-semibold uppercase tracking-wide text-neutral-500">Upcoming</h2>
      <ul className="flex flex-col gap-1.5">
        {shown.map((e) => (
          <li key={e.id} className="flex items-baseline gap-3 text-sm">
            <span className="w-32 shrink-0 text-xs text-neutral-500">{formatEventWhen(e.start, e.all_day)}</span>
            <span className="truncate text-neutral-200">{e.summary}</span>
            {e.location && <span className="truncate text-xs text-neutral-600">{e.location}</span>}
          </li>
        ))}
      </ul>
      {events.length > shown.length && (
        <p className="mt-2 text-xs text-neutral-600">+{events.length - shown.length} more</p>
      )}
    </section>
  );
}

/** A short "when" for a calendar event — date for all-day, date + time otherwise. */
function formatEventWhen(start: string, allDay?: boolean): string {
  const d = new Date(start);
  if (Number.isNaN(d.getTime())) return start.slice(0, 16);
  const date = d.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  // All-day events have a plain date with no time component.
  if (allDay || !start.includes("T")) return date;
  return `${date} ${d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`;
}
