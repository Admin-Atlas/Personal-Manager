// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import {
  addMilestone,
  addPreference,
  calendarOverview,
  getDailyBriefing,
  listCalendarEvents,
  listProjectOverviews,
  proposeProjectMetadata,
  refreshDailyBriefing,
  resolveFlag,
  routeFocusInput,
  setProjectMetadata,
  syncCalendar,
} from "../lib/ipc";
import { MilestoneList } from "./MilestoneList";
import type {
  CalendarEvent,
  DailyBriefing,
  FocusRoute,
  Importance,
  ProjectOverview,
  ProjectProposal,
  ProjectSize,
  ProjectStatus,
} from "../lib/types";
import { formatDate } from "../lib/format";
import { rankImportance } from "../lib/importance";
import { Button, Card, Input, Skeleton, StatusBadge, Select } from "./ui";
import { useDepth } from "../theme";

interface Props {
  /** Open the per-project scoped view. */
  onOpenProject: (project: string) => void;
  /** Route a question typed in the focus box to a fresh, flag-grounded general chat (board card 9). */
  onAsk: (text: string) => void;
}

/** Surface the most action-worthy first, mirroring the backend status precedence. */
const STATUS_ORDER: ProjectStatus[] = [
  "due_soon",
  "blocked",
  "quick_win",
  "take_a_look",
  "part_of",
  "on_track",
];

// --- focus-view sorting (Step 5 follow-up) ---
// "Smart" is the default — the status precedence above, the same one the focus view always
// used. The other keys let the user re-rank by one explicit attribute, in either direction.
type SortKey = "smart" | "deadline" | "importance" | "size" | "recent";
interface Sort {
  key: SortKey;
  dir: "asc" | "desc";
}
const SORT_LABELS: Record<SortKey, string> = {
  smart: "Smart",
  deadline: "Deadline",
  importance: "Importance",
  size: "Size",
  recent: "Recent active",
};
/** The natural direction for each key when it's first chosen (the ↑/↓ toggle flips it). */
const DEFAULT_DIR: Record<SortKey, "asc" | "desc"> = {
  smart: "asc", // most pressing first
  deadline: "asc", // soonest first
  importance: "desc", // highest first
  size: "desc", // largest first
  recent: "desc", // most recently active first
};
const SIZE_RANK: Record<string, number> = { quick: 1, standard: 2, large: 3 };
const SORT_LS_KEY = "pm.focus.sort";

/** The date a deadline-sort ranks on: the governing milestone, else a name-matched calendar
 *  event, else a far-future sentinel so undated projects sort last (ascending). */
function deadlineKey(p: ProjectOverview): string {
  return (p.governing_milestone?.due_date ?? p.calendar_event?.start ?? "9999-12-31").slice(0, 10);
}

/** The priority tag a project shows: the manual override, falling back to the computed
 *  structural auto-importance (the "Auto" value) when no override is set. */
function effectiveImportance(p: ProjectOverview): Importance {
  return p.importance ?? p.auto_importance;
}

/** Ascending comparison for one sort key (the ↑/↓ toggle applies the direction outside). */
function ascCompare(a: ProjectOverview, b: ProjectOverview, key: SortKey): number {
  switch (key) {
    case "smart":
      return STATUS_ORDER.indexOf(a.status) - STATUS_ORDER.indexOf(b.status);
    case "deadline":
      return deadlineKey(a).localeCompare(deadlineKey(b));
    case "importance":
      return rankImportance(effectiveImportance(a)) - rankImportance(effectiveImportance(b));
    case "size":
      return (SIZE_RANK[a.size ?? ""] ?? 0) - (SIZE_RANK[b.size ?? ""] ?? 0);
    case "recent":
      return (a.last_activity ?? "").localeCompare(b.last_activity ?? "");
  }
}

function readSort(): Sort {
  try {
    const raw = localStorage.getItem(SORT_LS_KEY);
    if (raw) {
      const s = JSON.parse(raw);
      if (s && s.key in SORT_LABELS && (s.dir === "asc" || s.dir === "desc")) return s;
    }
  } catch {
    /* fall through to the default */
  }
  return { key: "smart", dir: "asc" };
}

// Switching tabs unmounts this view, so without a cache every return refetches and
// flashes the skeleton. Remember the last good load at module scope and seed state
// from it: revisits render instantly, then the mount effect revalidates in the
// background. Memory-only (cleared on app reload), so the first open still loads.
let cachedProjects: ProjectOverview[] | null = null;
let cachedEvents: CalendarEvent[] = [];
let cachedBriefing: DailyBriefing | null = null;
// The focus box's in-progress text and any staged suggestion (a pending confirm / a note) must outlive a
// tab switch too — the whole view unmounts, so component-local state would be dropped. Same lifetime as
// the loads above: remembered until the app reloads, so a suggestion you haven't acted on is still there
// when you come back.
let cachedFocusBox: { text: string; pending: FocusRoute | null; note: string | null } = {
  text: "",
  pending: null,
  note: null,
};

export function FocusView({ onOpenProject, onAsk }: Props) {
  const [projects, setProjects] = useState<ProjectOverview[]>(() => cachedProjects ?? []);
  // Skeleton only on the genuine first load of the session; revisits seed from cache.
  const [loading, setLoading] = useState(cachedProjects === null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  /** AI proposals keyed by project name, populated by "Suggest attributes". */
  const [proposals, setProposals] = useState<Record<string, ProjectProposal>>({});
  const [proposing, setProposing] = useState(false);
  /** Upcoming calendar events for the agenda (empty when not connected). */
  const [events, setEvents] = useState<CalendarEvent[]>(() => cachedEvents);
  /** The daily briefing (Step 7); null until loaded, then refreshed when stale. */
  const [briefing, setBriefing] = useState<DailyBriefing | null>(() => cachedBriefing);
  const [briefingBusy, setBriefingBusy] = useState(false);
  // Sequence guard so a stale in-flight refresh can't overwrite a newer one — the
  // initial pre-sync load and the post-sync reload can resolve out of order.
  const refreshSeqRef = useRef(0);
  // These views fire background model calls (suggest/briefing) that can resolve
  // after the user has left; don't write state once unmounted.
  const aliveRef = useRef(true);
  // StrictMode (dev) double-invokes effects mount → unmount → mount; without
  // re-arming on remount the flag stays false and every guarded write — including
  // refresh()'s setLoading(false) — is silently dropped, stranding the skeleton.
  useEffect(() => {
    aliveRef.current = true;
    return () => void (aliveRef.current = false);
  }, []);

  async function refresh() {
    const seq = ++refreshSeqRef.current;
    try {
      const overviews = await listProjectOverviews();
      if (aliveRef.current && seq === refreshSeqRef.current) {
        setProjects(overviews);
        cachedProjects = overviews;
      }
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
      if (aliveRef.current) {
        setBriefing(b);
        cachedBriefing = b;
      }
    } catch {
      /* keep whatever we have */
    } finally {
      if (aliveRef.current) setBriefingBusy(false);
    }
  }

  // After a flag is asserted done in the focus box: regenerate the briefing (the resolved flag drops
  // out) AND reload the project overviews — a milestone-anchored flag also ticks its milestone `met`
  // on the backend, which changes that project's governing status, so the cards must re-derive.
  async function onFlagResolved() {
    await Promise.all([regenerateBriefing(), refresh()]);
  }

  // Load the stored briefing, and silently regenerate it when stale (older than the
  // freshness window) so it refreshes ~once a day on open, not on every mount.
  async function loadBriefing() {
    try {
      const b = await getDailyBriefing();
      if (!aliveRef.current) return;
      setBriefing(b);
      cachedBriefing = b;
      if (b.stale) void regenerateBriefing();
    } catch {
      /* briefing is optional — focus view works without it */
    }
  }

  // On landing: paint the project cards and today's briefing IMMEDIATELY (Steps 6-7). The old code
  // gated them behind a NETWORK calendar sync, so cold-start time-to-content was the sync's latency
  // (F-09). Now the calendar chain runs in parallel and re-derives the cards once it lands, so a
  // name-matched synced event can still flip a project to "Due soon" — exactly the immediate-then-
  // resync pattern onFlagResolved uses. refreshSeqRef makes the later refresh() win if the immediate
  // and post-sync loads resolve out of order; aliveRef still guards setEvents.
  useEffect(() => {
    void refresh();
    void loadBriefing();
    void (async () => {
      try {
        const overview = await calendarOverview();
        if (overview.accounts.length > 0) {
          await syncCalendar().catch(() => {});
          const evts = await listCalendarEvents();
          if (aliveRef.current) {
            setEvents(evts);
            cachedEvents = evts;
          }
          await refresh();
        }
      } catch {
        /* connector optional — focus view works without it */
      }
    })();
  }, []);

  const names = useMemo(() => projects.map((p) => p.name), [projects]);
  // How the project list is ordered. Defaults to "Smart" (status precedence); remembered per-device.
  const [sort, setSort] = useState<Sort>(() => readSort());
  useEffect(() => {
    localStorage.setItem(SORT_LS_KEY, JSON.stringify(sort));
  }, [sort]);
  const sorted = useMemo(() => {
    const factor = sort.dir === "asc" ? 1 : -1;
    return [...projects].sort(
      (a, b) => ascCompare(a, b, sort.key) * factor || a.name.localeCompare(b.name),
    );
  }, [projects, sort]);

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
      <header className="flex items-center justify-between border-b border-border px-6 py-3">
        <div data-help="focus-header">
          <h1 className="font-head text-sm font-semibold text-ink">Focus</h1>
          <p className="text-xs text-ink3">
            Every project, one status — what to look at right now.
          </p>
        </div>
        <Button
          variant="secondary"
          onClick={suggestAll}
          disabled={proposing || projects.length === 0}
          data-help="focus-suggest"
          title="Let the AI propose a size, parent, blocker and deadline for each project"
        >
          {proposing ? "Suggesting…" : "Suggest attributes (AI)"}
        </Button>
      </header>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto max-w-3xl px-6 py-6">
          {error && (
            <div
              className="mb-4 rounded-[var(--radius-sm)] border px-3 py-2 text-sm text-st-due"
              style={{
                borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
                background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
              }}
            >
              {error}
            </div>
          )}
          <Briefing briefing={briefing} busy={briefingBusy} onRefresh={regenerateBriefing} />
          {!loading && projects.length > 0 && (
            <FocusBox onAsk={onAsk} onOpenProject={onOpenProject} onResolved={onFlagResolved} />
          )}
          {events.length > 0 && <Agenda events={events} />}
          {loading ? (
            <ul className="flex flex-col gap-2">
              {Array.from({ length: 4 }).map((_, i) => (
                <li key={i}>
                  <Skeleton className="h-16 w-full" />
                </li>
              ))}
            </ul>
          ) : projects.length === 0 ? (
            <p className="text-sm text-ink4">
              No projects yet. Ingest some documents and sort them in Review.
            </p>
          ) : (
            <>
              <div
                className="mb-2 flex items-center justify-end gap-1.5 text-xs text-ink4"
                data-help="focus-sort"
              >
                <span>Sort</span>
                <Select
                  value={sort.key}
                  onChange={(e) => {
                    const key = e.target.value as SortKey;
                    // Choosing a key applies its natural direction; the ↑/↓ button flips it.
                    setSort((cur) => (cur.key === key ? cur : { key, dir: DEFAULT_DIR[key] }));
                  }}
                  className="h-7 py-0 text-xs"
                  title="Reorder projects"
                >
                  {(Object.keys(SORT_LABELS) as SortKey[]).map((k) => (
                    <option key={k} value={k}>
                      {SORT_LABELS[k]}
                    </option>
                  ))}
                </Select>
                <button
                  type="button"
                  onClick={() => setSort((s) => ({ ...s, dir: s.dir === "asc" ? "desc" : "asc" }))}
                  title={
                    sort.dir === "asc"
                      ? "Ascending — click for descending"
                      : "Descending — click for ascending"
                  }
                  className="rounded-[var(--radius-sm)] px-1.5 py-0.5 hover:bg-surface hover:text-ink2"
                >
                  {sort.dir === "asc" ? "↑" : "↓"}
                </button>
              </div>
              <ul className="flex flex-col gap-2" data-help="focus-cards">
                {sorted.map((p) => (
                  <ProjectCard
                    key={p.name}
                    project={p}
                    otherProjects={names.filter((n) => n !== p.name)}
                    events={events}
                    proposal={proposals[p.name]}
                    editing={editing === p.name}
                    onEdit={() => setEditing(editing === p.name ? null : p.name)}
                    onOpen={() => onOpenProject(p.name)}
                    onChanged={refresh}
                    onSaved={async () => {
                      setEditing(null);
                      await refresh();
                    }}
                  />
                ))}
              </ul>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function ProjectCard({
  project,
  otherProjects,
  events,
  proposal,
  editing,
  onEdit,
  onOpen,
  onChanged,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  events: CalendarEvent[];
  proposal?: ProjectProposal;
  editing: boolean;
  onEdit: () => void;
  onOpen: () => void;
  onChanged: () => void;
  onSaved: () => void;
}) {
  const { minimal, showMeta } = useDepth();
  const badge = (
    <StatusBadge
      status={project.status}
      label={
        project.status === "part_of" && project.parent ? `Part of ${project.parent}` : undefined
      }
    />
  );

  return (
    <li data-help="focus-card">
      <Card className="overflow-hidden">
        <div className="flex items-center justify-between gap-3 px-4 py-3">
          <button onClick={onOpen} className="min-w-0 flex-1 text-left">
            <div className="flex items-center gap-2">
              <span className="shrink-0" data-help="focus-status-badge">
                {badge}
              </span>
              <span
                className={`truncate font-medium text-ink ${minimal ? "text-base" : "text-sm"}`}
              >
                {project.name}
              </span>
            </div>
            {showMeta && (
              <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-xs text-ink4">
                <span>
                  {project.doc_count} doc{project.doc_count === 1 ? "" : "s"}
                </span>
                {effectiveImportance(project) && (
                  <span className="capitalize">
                    {effectiveImportance(project)} priority
                    {!project.importance && project.auto_importance ? " (auto)" : ""}
                  </span>
                )}
                {project.size && <span>{project.size}</span>}
                {project.governing_milestone?.due_date && (
                  <span>
                    {project.governing_milestone.label}{" "}
                    {formatDate(project.governing_milestone.due_date.slice(0, 10))}
                    {project.milestones.length > 1 ? ` +${project.milestones.length - 1}` : ""}
                  </span>
                )}
                {project.blocked_by && <span>blocked by {project.blocked_by}</span>}
                {project.last_activity && <span>active {formatDate(project.last_activity)}</span>}
              </div>
            )}
            {showMeta && project.calendar_event && (
              <div className="mt-1 truncate text-xs text-accent-text">
                📅 {project.calendar_event.summary} ·{" "}
                {formatEventWhen(project.calendar_event.start)}
              </div>
            )}
          </button>
          <button
            onClick={onEdit}
            className="shrink-0 rounded-[var(--radius-sm)] px-2 py-1 text-xs text-ink3 transition hover:bg-surface hover:text-ink2"
          >
            {editing ? "Close" : "Triage"}
          </button>
        </div>

        {editing && (
          <MetaEditor
            project={project}
            otherProjects={otherProjects}
            events={events}
            proposal={proposal}
            onChanged={onChanged}
            onSaved={onSaved}
          />
        )}
      </Card>
    </li>
  );
}

/** Inline triage editor — the "you-confirm" half. Pre-fills from the AI proposal
 *  if one was suggested, otherwise from the project's current values. */
function MetaEditor({
  project,
  otherProjects,
  events,
  proposal,
  onChanged,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  events: CalendarEvent[];
  proposal?: ProjectProposal;
  onChanged: () => void;
  onSaved: () => void;
}) {
  const [size, setSize] = useState<ProjectSize>(project.size);
  const [importance, setImportance] = useState<Importance>(project.importance);
  const [blockedBy, setBlockedBy] = useState(project.blocked_by ?? "");
  const [parent, setParent] = useState(project.parent ?? "");
  const [saving, setSaving] = useState(false);

  async function applyProposal() {
    if (!proposal) return;
    setSize(proposal.size);
    if (proposal.blocked_by) setBlockedBy(proposal.blocked_by);
    if (proposal.parent) setParent(proposal.parent);
    // A proposed deadline becomes a milestone straight away (milestones persist live).
    if (proposal.deadline) {
      await addMilestone(project.name, "deadline", proposal.deadline.slice(0, 10), null);
      onChanged();
    }
  }

  async function save() {
    setSaving(true);
    try {
      await setProjectMetadata(project.name, {
        size,
        importance,
        blockedBy: blockedBy || null,
        parent: parent || null,
      });
      onSaved();
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="border-t border-border px-4 py-3" data-help="focus-triage">
      {proposal && (
        <div
          className="mb-3 rounded-[var(--radius-sm)] border px-3 py-2 text-xs text-accent-text"
          style={{
            borderColor: "color-mix(in oklab, var(--accent) 35%, transparent)",
            background: "color-mix(in oklab, var(--accent) 10%, transparent)",
          }}
        >
          <div className="mb-1 flex items-center justify-between">
            <span className="font-medium">AI suggestion</span>
            <button
              onClick={applyProposal}
              className="rounded-[var(--radius-sm)] px-2 py-0.5 text-accent-text transition hover:bg-accent-soft"
            >
              Use it
            </button>
          </div>
          <p className="text-ink3">
            {proposal.size ? `size ${proposal.size}` : "no size"}
            {proposal.parent ? ` · part of ${proposal.parent}` : ""}
            {proposal.blocked_by ? ` · blocked by ${proposal.blocked_by}` : ""}
            {proposal.deadline ? ` · due ${formatDate(proposal.deadline.slice(0, 10))}` : ""}
          </p>
          {proposal.reasoning && <p className="mt-1 text-ink4">{proposal.reasoning}</p>}
        </div>
      )}

      <div className="grid grid-cols-2 gap-3">
        <Field label="Size (Quick win = quick)">
          <Select
            value={size ?? ""}
            onChange={(e) => setSize((e.target.value || null) as ProjectSize)}
            className="w-full"
          >
            <option value="">—</option>
            <option value="quick">quick</option>
            <option value="standard">standard</option>
            <option value="large">large</option>
          </Select>
        </Field>
        <Field label="Priority">
          <Select
            value={importance ?? ""}
            onChange={(e) => setImportance((e.target.value || null) as Importance)}
            className="w-full"
            title="A manual priority for this project. Auto shows no tag."
          >
            <option value="">Auto (no tag)</option>
            <option value="high">high</option>
            <option value="medium">medium</option>
            <option value="low">low</option>
          </Select>
          {importance === null && project.auto_importance && (
            <p className="mt-1 text-xs text-ink4">
              Auto currently resolves to:{" "}
              <span className="capitalize">{project.auto_importance}</span>
            </p>
          )}
        </Field>
        <Field label="Blocked by">
          <ProjectSelect value={blockedBy} options={otherProjects} onChange={setBlockedBy} />
        </Field>
        <Field label="Part of (parent)">
          <ProjectSelect value={parent} options={otherProjects} onChange={setParent} />
        </Field>
      </div>

      <div className="mt-3">
        <span className="text-xs text-ink3">Milestones (the nearest unmet drives Due soon)</span>
        <div className="mt-1.5">
          <MilestoneList
            project={project.name}
            milestones={project.milestones}
            calendarEvents={events}
            onChanged={onChanged}
          />
        </div>
      </div>

      <div className="mt-3 flex justify-end gap-2">
        <Button variant="primary" onClick={save} disabled={saving}>
          {saving ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs text-ink3">{label}</span>
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
    <Select value={value} onChange={(e) => onChange(e.target.value)} className="w-full">
      <option value="">—</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
    </Select>
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
    <Card className="mb-5 px-4 py-3" data-help="focus-briefing">
      <div className="mb-2 flex items-center justify-between">
        <h2 className="font-mono text-xs font-semibold uppercase tracking-wide text-ink3">Today</h2>
        <Button
          variant="tertiary"
          onClick={onRefresh}
          disabled={busy}
          title="Regenerate today's briefing from your current projects and calendar"
          className="px-2 py-0.5 text-xs"
        >
          {busy ? "Refreshing…" : "Refresh"}
        </Button>
      </div>
      {text ? (
        <div className="whitespace-pre-wrap text-sm leading-relaxed text-ink2">{text}</div>
      ) : (
        <p className="text-sm text-ink4">Putting together your briefing…</p>
      )}
      {text && briefing?.updated_at && (
        <p className="mt-2 text-xs text-ink4">Updated {formatDate(briefing.updated_at)}</p>
      )}
    </Card>
  );
}

/** The polymorphic focus box (board card 9, decisions 6–7). One typed line is classified by a
 *  background router and placed: mark a visible flag done (a deliberate user assertion — confirmed here
 *  before it commits, so nothing is ever crossed off without a vouch), capture a durable preference,
 *  ask a flag-grounded question (routed to a fresh chat), or edit a project. No pre-scoping click —
 *  natural language picks the target from the visible flag set. */
function FocusBox({
  onAsk,
  onOpenProject,
  onResolved,
}: {
  onAsk: (text: string) => void;
  onOpenProject: (project: string) => void;
  onResolved: () => void;
}) {
  // Seed from the module cache so a suggestion survives leaving and returning to the tab (see
  // cachedFocusBox). `busy` is transient and deliberately not cached.
  const [text, setText] = useState(cachedFocusBox.text);
  const [busy, setBusy] = useState(false);
  // A proposed write awaiting the user's one-tap confirm. Resolve/prefer are the only writes, and both
  // pass through here — so a flag only leaves the set on an explicit vouch (HITL-confirm, decision 5).
  const [pending, setPending] = useState<FocusRoute | null>(cachedFocusBox.pending);
  const [note, setNote] = useState<string | null>(cachedFocusBox.note);

  // Mirror the staged suggestion into the module cache on every change, so it's restored on remount.
  useEffect(() => {
    cachedFocusBox = { text, pending, note };
  }, [text, pending, note]);

  async function submit() {
    const trimmed = text.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setNote(null);
    setPending(null);
    try {
      const route = await routeFocusInput(trimmed);
      switch (route.kind) {
        case "resolve":
        case "prefer":
          setPending(route); // stage it — the confirm strip below commits it
          break;
        case "ask":
          onAsk(route.text); // navigates to a fresh, flag-grounded chat
          setText("");
          break;
        case "edit":
          if (route.project) {
            onOpenProject(route.project); // navigates to the project's view
            setText("");
          } else {
            setNote("Open the project to edit its milestones.");
          }
          break;
        case "unclear":
          setNote("Not sure what to do with that — try rephrasing, or ask it as a question.");
          break;
      }
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function confirmPending() {
    if (!pending) return;
    setBusy(true);
    try {
      if (pending.kind === "resolve") {
        await resolveFlag(pending.flag_id);
        setNote(`Marked done: ${pending.label}`);
        onResolved(); // regenerate the briefing so the resolved flag drops out of it
      } else if (pending.kind === "prefer") {
        const { scope, entity_id, condition, value } = pending.draft;
        await addPreference(scope, entity_id, condition, value);
        setNote("Saved — PM will keep that in mind.");
      }
      setPending(null);
      setText("");
    } catch (e) {
      setNote(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card className="mb-5 px-4 py-3" data-help="focus-box">
      <div className="flex items-center gap-2">
        <Input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void submit();
            }
          }}
          disabled={busy}
          placeholder="Mark something done, set a reminder preference, or ask…"
          data-help="focus-box-input"
        />
        <Button variant="secondary" onClick={submit} disabled={busy || !text.trim()}>
          {busy ? "…" : "Go"}
        </Button>
      </div>

      {pending?.kind === "resolve" && (
        <ConfirmRow
          prompt={
            <>
              Mark <span className="font-medium text-ink2">{pending.label}</span> as done?
            </>
          }
          confirmLabel="Mark done"
          onConfirm={confirmPending}
          onCancel={() => setPending(null)}
          busy={busy}
        />
      )}
      {pending?.kind === "prefer" && (
        <ConfirmRow
          prompt={
            <>
              Remember: <span className="font-medium text-ink2">“{pending.draft.value}”</span>
              {pending.draft.project_name ? ` · for ${pending.draft.project_name}` : ""}
              {pending.draft.condition ? ` · when ${pending.draft.condition}` : ""}
            </>
          }
          confirmLabel="Save"
          onConfirm={confirmPending}
          onCancel={() => setPending(null)}
          busy={busy}
        />
      )}
      {note && !pending && <p className="mt-2 text-xs text-ink4">{note}</p>}
    </Card>
  );
}

/** A one-tap confirm strip for a proposed write (resolve/prefer) staged by the focus box. */
function ConfirmRow({
  prompt,
  confirmLabel,
  onConfirm,
  onCancel,
  busy,
}: {
  prompt: React.ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  busy: boolean;
}) {
  return (
    <div className="mt-2 flex items-center justify-between gap-3 text-sm text-ink3">
      <span className="min-w-0">{prompt}</span>
      <span className="flex shrink-0 gap-2">
        <Button
          variant="primary"
          onClick={onConfirm}
          disabled={busy}
          className="px-2 py-0.5 text-xs"
        >
          {confirmLabel}
        </Button>
        <Button
          variant="tertiary"
          onClick={onCancel}
          disabled={busy}
          className="px-2 py-0.5 text-xs"
        >
          Cancel
        </Button>
      </span>
    </div>
  );
}

/** The "Upcoming" agenda — the next handful of events from connected calendars. */
function Agenda({ events }: { events: CalendarEvent[] }) {
  const shown = events.slice(0, 8);
  return (
    <Card className="mb-5 px-4 py-3" data-help="focus-agenda">
      <h2 className="mb-2 font-mono text-xs font-semibold uppercase tracking-wide text-ink3">
        Upcoming
      </h2>
      <ul className="flex flex-col gap-1.5">
        {shown.map((e) => (
          <li key={e.id} className="flex items-baseline gap-3 text-sm">
            <span className="w-32 shrink-0 font-mono text-xs text-ink3">
              {formatEventWhen(e.start, e.all_day)}
            </span>
            <span className="truncate text-ink2">{e.summary}</span>
            {e.location && <span className="truncate text-xs text-ink4">{e.location}</span>}
          </li>
        ))}
      </ul>
      {events.length > shown.length && (
        <p className="mt-2 text-xs text-ink4">+{events.length - shown.length} more</p>
      )}
    </Card>
  );
}

/** A short "when" for a calendar event — date for all-day, date + time otherwise. */
function formatEventWhen(start: string, allDay?: boolean): string {
  const d = new Date(start);
  if (Number.isNaN(d.getTime())) return start.slice(0, 16);
  const date = formatDate(start);
  // All-day events have a plain date with no time component.
  if (allDay || !start.includes("T")) return date;
  return `${date} ${d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" })}`;
}
