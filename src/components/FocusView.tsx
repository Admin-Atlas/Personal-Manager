// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import {
  addMilestone,
  addPreference,
  calendarOverview,
  listCalendarEvents,
  listProjectOverviews,
  proposeProjectMetadata,
  resolveFlag,
  routeFocusInput,
  setProjectMetadata,
  syncCalendar,
} from "../lib/ipc";
import { MilestoneList } from "./MilestoneList";
import { MergeProjectDialog } from "./MergeProjectDialog";
import { DeleteProjectDialog } from "./DeleteProjectDialog";
import { FocusUpcoming } from "./FocusUpcoming";
import { Briefing } from "./Briefing";
import type {
  AgendaEvent,
  Calendar,
  FocusRoute,
  Importance,
  ProjectOverview,
  ProjectProposal,
  ProjectSize,
} from "../lib/types";
import { formatDate, formatDateOnly, formatEventWhen } from "../lib/format";
import { runMutation } from "../lib/runMutation";
import {
  DEFAULT_DIR,
  SORT_LABELS,
  effectiveImportance,
  readSort,
  sortProjects,
  writeSort,
  type Sort,
  type SortKey,
} from "../lib/focusSort";
import {
  Button,
  Card,
  Input,
  Popover,
  Skeleton,
  StatusBadge,
  Select,
  SegmentedControl,
} from "./ui";
import {
  FOCUS_PANELS,
  FOCUS_SPLIT_MAX,
  FOCUS_SPLIT_MIN,
  FOCUS_SPLIT_MIN_PX,
  clampFocusSplit,
  readFocusHiddenPanels,
  readFocusLayout,
  readFocusSplit,
  writeFocusHiddenPanels,
  writeFocusLayout,
  writeFocusSplit,
  type FocusLayout,
  type FocusPanel,
} from "../lib/focusPrefs";
import { useDepth } from "../theme";
import { useBriefing } from "../lib/briefing";

interface Props {
  /** Open the per-project scoped view. */
  onOpenProject: (project: string) => void;
  /** Route a question typed in the focus box to a fresh, flag-grounded general chat (board card 9). */
  onAsk: (text: string) => void;
}

// Switching tabs unmounts this view, so without a cache every return refetches and
// flashes the skeleton. Remember the last good load at module scope and seed state
// from it: revisits render instantly, then the mount effect revalidates in the
// background. Memory-only (cleared on app reload), so the first open still loads.
let cachedProjects: ProjectOverview[] | null = null;
let cachedEvents: AgendaEvent[] = [];
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
  /** The project whose suggestion is in flight, or null. */
  const [proposing, setProposing] = useState<string | null>(null);
  /** Focus-agenda events (empty when not connected). Includes events that ended earlier today, tagged
   *  `ended` — the Agenda greys those rather than hiding them. */
  const [events, setEvents] = useState<AgendaEvent[]>(() => cachedEvents);
  /** The connected calendars. Whole objects rather than bare ids: FocusUpcoming colours the grid from
   *  them, needs `quiet` to honour the same exclusion the agenda feed applies, and the event detail
   *  popover names the owning calendar. */
  const [calendars, setCalendars] = useState<Calendar[]>([]);
  // The briefing is owned by the app-scope provider (it has up to three surfaces mounted at once);
  // this view only needs to re-trigger it when a flag is resolved.
  const { refresh: regenerateBriefing } = useBriefing();
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

  // After a flag is asserted done in the focus box: regenerate the briefing (the resolved flag drops
  // out) AND reload the project overviews — a milestone-anchored flag also ticks its milestone `met`
  // on the backend, which changes that project's governing status, so the cards must re-derive.
  async function onFlagResolved() {
    await Promise.all([regenerateBriefing(), refresh()]);
  }

  // On landing: paint the project cards and today's briefing IMMEDIATELY (Steps 6-7). The old code
  // gated them behind a NETWORK calendar sync, so cold-start time-to-content was the sync's latency
  // (F-09). Now the calendar chain runs in parallel and re-derives the cards once it lands, so a
  // name-matched synced event can still flip a project to "Due soon" — exactly the immediate-then-
  // resync pattern onFlagResolved uses. refreshSeqRef makes the later refresh() win if the immediate
  // and post-sync loads resolve out of order; aliveRef still guards setEvents.
  useEffect(() => {
    void refresh();
    void (async () => {
      try {
        const overview = await calendarOverview();
        if (aliveRef.current) setCalendars(overview.calendars);
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
  // Which panels the user has switched off. Subscribed rather than read-once: Settings' "Reset
  // Focus" renders as an overlay over this still-mounted view, so a read-at-mount would leave a
  // reset looking like it had done nothing until the tab remounted.
  const [hiddenPanels, setHiddenPanels] = useState<Set<FocusPanel>>(readFocusHiddenPanels);
  useEffect(() => {
    const onChanged = () => setHiddenPanels(readFocusHiddenPanels());
    window.addEventListener("pm:settings-changed", onChanged);
    return () => window.removeEventListener("pm:settings-changed", onChanged);
  }, []);
  const shown = (id: FocusPanel) => !hiddenPanels.has(id);
  // The body must never be able to go completely blank, so the last visible panel can't be switched
  // off — its checkbox is disabled rather than silently ignoring the click.
  const visibleCount = FOCUS_PANELS.length - hiddenPanels.size;
  function togglePanel(id: FocusPanel) {
    const next = new Set(hiddenPanels);
    if (next.has(id)) next.delete(id);
    else if (visibleCount > 1) next.add(id);
    else return;
    setHiddenPanels(next);
    writeFocusHiddenPanels(next);
  }
  useEffect(() => {
    writeSort(sort);
  }, [sort]);
  const sorted = useMemo(() => sortProjects(projects, sort), [projects, sort]);

  // Split (side-by-side) vs stacked layout — per-device, set from this header.
  const [layout, setLayout] = useState<FocusLayout>(readFocusLayout);
  function changeLayout(next: FocusLayout) {
    setLayout(next);
    writeFocusLayout(next);
  }

  // How the split layout divides its width, as the LEFT column's share. Dragged from the divider
  // between the columns, persisted per-device (see focusPrefs), and reset by "Reset Focus".
  const [split, setSplit] = useState<number>(readFocusSplit);
  const [dragging, setDragging] = useState(false);
  const splitRowRef = useRef<HTMLDivElement>(null);
  // The two-column template is applied as an INLINE style (it carries a live fraction), and inline
  // styles beat Tailwind's `grid-cols-1` — so the lg breakpoint has to be evaluated here rather than
  // left to the class, or a narrow window would keep three tracks instead of collapsing to one.
  // Matches the `lg:` breakpoint the divider itself is gated on.
  const [wideScreen, setWideScreen] = useState(
    () => window.matchMedia("(min-width: 1024px)").matches,
  );
  useEffect(() => {
    const mql = window.matchMedia("(min-width: 1024px)");
    const onChange = () => setWideScreen(mql.matches);
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, []);

  // One pointer gesture, window-level so a fast drag that outruns the pointer keeps tracking (the
  // BriefingPanel pattern). The fraction is measured against the ROW, not the window, so it means the
  // same thing at any window size; the pixel floor is applied here because only the live row width
  // knows whether a legal fraction would still leave a usable column.
  function startSplitDrag(e: React.PointerEvent) {
    e.preventDefault();
    const row = splitRowRef.current;
    if (!row) return;
    setDragging(true);
    let latest = split;
    const onMove = (ev: PointerEvent) => {
      const rect = row.getBoundingClientRect();
      if (rect.width <= 0) return;
      const raw = (ev.clientX - rect.left) / rect.width;
      const floor = Math.max(FOCUS_SPLIT_MIN, FOCUS_SPLIT_MIN_PX / rect.width);
      const ceil = Math.min(FOCUS_SPLIT_MAX, 1 - FOCUS_SPLIT_MIN_PX / rect.width);
      // A row too narrow to honour both floors at once (a very small window) sits at dead centre
      // rather than snapping to a bound.
      latest = floor > ceil ? 0.5 : Math.min(ceil, Math.max(floor, raw));
      setSplit(latest);
    };
    const finish = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      window.removeEventListener("blur", finish);
      setDragging(false);
      writeFocusSplit(latest);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("blur", finish);
  }

  /** Keyboard resize + double-click reset, so the divider isn't pointer-only. */
  function nudgeSplit(delta: number) {
    const next = clampFocusSplit(split + delta);
    setSplit(next);
    writeFocusSplit(next);
  }

  /** Ask the AI to propose triage metadata for ONE project.
   *
   *  This used to be a header button covering the whole list, which meant one click fanned out to a
   *  model call per project — bounded only at 2000 — with no confirmation and no way to want it for
   *  just the project in front of you. Scoped to the project whose Triage panel is open, it is one
   *  click, one call, beside the fields it fills in. The command already took a `names` filter. */
  async function suggestOne(name: string) {
    if (proposing) return;
    setProposing(name);
    setError(null);
    try {
      await proposeProjectMetadata(
        (event) => {
          if (!aliveRef.current) return; // view gone — drop late proposals
          if (event.type === "proposed") {
            setProposals((prev) => ({ ...prev, [event.project]: event.proposal }));
          }
        },
        [name],
      );
    } catch (e) {
      if (aliveRef.current) setError(String(e));
    } finally {
      if (aliveRef.current) setProposing(null);
    }
  }

  // The two columns, split out so the side-by-side and stacked layouts arrange the same JSX without
  // duplicating it. In stacked mode they render one after another exactly as before.
  const briefingAndActions = (
    <>
      {shown("briefing") && <Briefing />}
      {/* Not gated on having projects. The focus box routes four ways and only one of them (edit)
          needs a project — asking a flag-grounded question, capturing a preference and marking a
          flag done all work on an empty install. Gating it on `projects.length > 0` made switching
          the "Focus box" panel on look like a dead toggle for anyone who hadn't sorted anything yet.
          Still gated on `loading` so it doesn't flash in beside the skeletons. */}
      {shown("actions") && !loading && (
        <FocusBox onAsk={onAsk} onOpenProject={onOpenProject} onResolved={onFlagResolved} />
      )}
      {shown("upcoming") && events.length > 0 && (
        <FocusUpcoming listEvents={events} calendars={calendars} onOpenProject={onOpenProject} />
      )}
    </>
  );
  // Whether the split layout's left column has anything in it. With all three off, its track would
  // otherwise render empty and hold the project list pinned to a dead offset.
  const leftColumnShown = shown("briefing") || shown("actions") || shown("upcoming");
  // Only with BOTH columns present is there anything to divide — otherwise the one that's left takes
  // the full width and the divider would be a handle onto nothing.
  const bothColumns = leftColumnShown && shown("projects");
  // The heading rides in the Sort row rather than above it, which is exactly what Briefing ("Today")
  // and FocusUpcoming ("Upcoming") do — this column was the only Focus panel without one, leaving the
  // corner left of the Sort control empty. It stays up in every branch (loading, empty, populated) so
  // the column is labelled consistently and matches the name the Panels popover uses; the Sort
  // cluster itself only appears when there is something to sort. `flex-wrap` for the narrow end of
  // the split (the column floors at 260px), mirroring FocusUpcoming's header row.
  const projectList = (
    <>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-mono text-xs font-semibold uppercase tracking-wide text-ink3">
          Projects
        </h2>
        {!loading && projects.length > 0 && (
          <div className="flex items-center gap-1.5 text-xs text-ink4" data-help="focus-sort">
            <span>Sort</span>
            <Select
              value={sort.key}
              onChange={(e) => {
                const key = e.target.value as SortKey;
                // Choosing a key applies its natural direction; the ↑/↓ button flips it.
                setSort((cur) => (cur.key === key ? cur : { key, dir: DEFAULT_DIR[key] }));
              }}
              compact
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
              className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] px-1.5 py-0.5 hover:bg-surface hover:text-ink2"
            >
              {sort.dir === "asc" ? "↑" : "↓"}
            </button>
          </div>
        )}
      </div>
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
        <ul className="flex flex-col gap-2" data-help="focus-cards">
          {sorted.map((p) => (
            <ProjectCard
              key={p.name}
              project={p}
              otherProjects={names.filter((n) => n !== p.name)}
              proposal={proposals[p.name]}
              suggesting={proposing === p.name}
              suggestDisabled={proposing !== null}
              onSuggest={() => void suggestOne(p.name)}
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
      )}
    </>
  );

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between gap-3 border-b border-border px-6 py-3">
        <div data-help="focus-header">
          <h1 className="font-head text-sm font-semibold text-ink">Focus</h1>
          <p className="text-xs text-ink3">
            Every project, one status — what to look at right now.
          </p>
        </div>
        <div className="flex items-center gap-2">
          {/* Split puts the briefing/actions/agenda beside the project list; stacked is one column.
              Hidden below lg, where the layout collapses to one column regardless. */}
          <SegmentedControl
            className="hidden lg:inline-flex"
            value={layout}
            onChange={changeLayout}
            options={[
              {
                value: "split",
                label: "Split",
                title: "Briefing and agenda beside the project list",
              },
              { value: "vertical", label: "Stacked", title: "Everything in one column" },
            ]}
          />
          {/* Which panels this tab shows. It lives in the header — the one part that is never
              hideable — because it is the way back for anything switched off. */}
          <Popover
            align="right"
            ariaLabel="Panels to show"
            trigger={({ open, toggle }) => (
              <Button
                variant="secondary"
                onClick={toggle}
                aria-expanded={open}
                data-help="focus-panels"
                title="Choose which panels this tab shows"
              >
                Panels
                <span className="font-mono text-xs text-ink4">
                  {visibleCount}/{FOCUS_PANELS.length}
                </span>
              </Button>
            )}
          >
            <ul>
              {FOCUS_PANELS.map((p) => {
                const on = shown(p.id);
                // The last one standing can't be switched off, or the tab body would be empty.
                const locked = on && visibleCount === 1;
                return (
                  <li key={p.id}>
                    <label
                      className={`flex items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink ${
                        locked ? "cursor-default opacity-60" : "cursor-pointer hover:bg-surface"
                      }`}
                      title={locked ? "At least one panel has to stay visible" : undefined}
                    >
                      <input
                        type="checkbox"
                        checked={on}
                        disabled={locked}
                        onChange={() => togglePanel(p.id)}
                        className="accent-[var(--accent)]"
                      />
                      <span className={on ? "" : "text-ink4"}>{p.label}</span>
                    </label>
                  </li>
                );
              })}
            </ul>
          </Popover>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto">
        {/* Split goes full-bleed, like Calendar / Documents / Pinboard. The old max-w-6xl cap left
            ~220px of dead gutter each side on a 1920 screen — #471 raised the cap to fix exactly
            that symptom but kept one. Stacked KEEPS max-w-3xl: a single column of prose is a reading
            measure, and removing it there would make the layout worse, not better.
            `flex min-h-full flex-col` so the grid below can claim the viewport height. */}
        <div
          className={`mx-auto flex min-h-full flex-col px-6 py-6 ${
            layout === "split" ? "" : "max-w-3xl"
          }`}
        >
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
          {layout === "split" ? (
            // Two columns on a wide screen (briefing/actions/agenda | project list) with a draggable
            // divider between them; the grid falls back to ONE column below lg, where the divider is
            // hidden and the row reads like the stacked layout. `bothColumns` matters because with
            // only one side showing there is nothing to divide — the single column takes the width.
            <div
              ref={splitRowRef}
              // `min-h-0 flex-1` so the row spans the remaining height: the divider rule then runs
              // full height instead of stopping at the tallest card, which was the most visible tell
              // that the panels weren't using the screen.
              className={`grid min-h-0 flex-1 grid-cols-1 gap-6 ${dragging ? "select-none" : ""}`}
              style={
                bothColumns
                  ? {
                      // Set only at lg+ via the media query below; the inline value is the wide-screen
                      // template and Tailwind's `grid-cols-1` governs narrow.
                      gridTemplateColumns: wideScreen
                        ? `minmax(0, ${split}fr) auto minmax(0, ${1 - split}fr)`
                        : undefined,
                    }
                  : undefined
              }
            >
              {leftColumnShown && <div className="min-w-0">{briefingAndActions}</div>}
              {bothColumns && (
                <div
                  role="separator"
                  aria-orientation="vertical"
                  aria-label="Resize the columns"
                  aria-valuenow={Math.round(split * 100)}
                  aria-valuemin={Math.round(FOCUS_SPLIT_MIN * 100)}
                  aria-valuemax={Math.round(FOCUS_SPLIT_MAX * 100)}
                  tabIndex={0}
                  title="Drag to resize · double-click for an even split"
                  onPointerDown={startSplitDrag}
                  onDoubleClick={() => {
                    setSplit(0.5);
                    writeFocusSplit(0.5);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "ArrowLeft") {
                      e.preventDefault();
                      nudgeSplit(-0.02);
                    } else if (e.key === "ArrowRight") {
                      e.preventDefault();
                      nudgeSplit(0.02);
                    }
                  }}
                  // `-mx-3` pulls the 24px hit area back into the gap so the grab zone is generous
                  // without the columns moving apart; the visible rule is the 1px child.
                  className="group -mx-3 hidden w-6 cursor-col-resize touch-none items-stretch justify-center focus:outline-none lg:flex"
                >
                  <div
                    className={`w-px transition-colors ${
                      dragging ? "bg-accent" : "bg-border group-hover:bg-ink4 group-focus:bg-accent"
                    }`}
                  />
                </div>
              )}
              {shown("projects") && <div className="min-w-0">{projectList}</div>}
            </div>
          ) : (
            <>
              {briefingAndActions}
              {shown("projects") && projectList}
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
  proposal,
  suggesting,
  suggestDisabled,
  onSuggest,
  editing,
  onEdit,
  onOpen,
  onChanged,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  proposal?: ProjectProposal;
  suggesting: boolean;
  /** True while ANY project's suggestion is in flight — they share one backend call. */
  suggestDisabled: boolean;
  onSuggest: () => void;
  editing: boolean;
  onEdit: () => void;
  onOpen: () => void;
  onChanged: () => void;
  onSaved: () => void;
}) {
  const { minimal, showMeta } = useDepth();
  const badge = <StatusBadge status={project.status} />;

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
                {/* No size chip: `quick` is already carried by the Quick win badge to the left, and
                    the other two values told you nothing the card didn't. The FIELD stays — it is
                    still editable in Triage and still a sort key, so no stored sort gets coerced.
                    The milestone chip below stays too: it is the only place the governing date
                    shows, and Smart now sorts on that date first. */}
                {project.governing_milestone?.due_date && (
                  <span>
                    {project.governing_milestone.label}{" "}
                    {formatDateOnly(project.governing_milestone.due_date.slice(0, 10))}
                    {project.milestones.length > 1 ? ` +${project.milestones.length - 1}` : ""}
                  </span>
                )}
                {project.blocked_by && <span>blocked by {project.blocked_by}</span>}
                {project.last_activity && <span>active {formatDate(project.last_activity)}</span>}
              </div>
            )}
            {showMeta && project.calendar_event && (
              <div className="mt-1 break-words text-xs text-accent-text">
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
            proposal={proposal}
            suggesting={suggesting}
            suggestDisabled={suggestDisabled}
            onSuggest={onSuggest}
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
  proposal,
  suggesting,
  suggestDisabled,
  onSuggest,
  onChanged,
  onSaved,
}: {
  project: ProjectOverview;
  otherProjects: string[];
  proposal?: ProjectProposal;
  suggesting: boolean;
  suggestDisabled: boolean;
  onSuggest: () => void;
  onChanged: () => void;
  onSaved: () => void;
}) {
  const [size, setSize] = useState<ProjectSize>(project.size);
  const [importance, setImportance] = useState<Importance>(project.importance);
  const [blockedBy, setBlockedBy] = useState(project.blocked_by ?? "");
  const [merging, setMerging] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [applying, setApplying] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function applyProposal() {
    if (!proposal || applying) return;
    setSize(proposal.size);
    if (proposal.blocked_by) setBlockedBy(proposal.blocked_by);
    // A proposed deadline becomes a milestone straight away (milestones persist live).
    if (!proposal.deadline) return;
    const due = proposal.deadline.slice(0, 10);
    // Skip if that deadline is already an unmet milestone: without this a second "Use it"
    // (or clicking again after re-opening triage — the proposal persists) files a duplicate
    // "deadline" that then competes for governing status (F3-9). The busy latch above closes
    // the same gap for a fast double-click.
    const alreadyFiled = project.milestones.some(
      (m) => m.state !== "met" && m.label === "deadline" && m.due_date?.slice(0, 10) === due,
    );
    if (alreadyFiled) return;
    setApplying(true);
    const ok = await runMutation(async () => {
      await addMilestone(project.name, "deadline", due, null);
      onChanged();
    }, setError);
    // Re-enable only to allow a retry after a failure. On success we stay latched: the deadline
    // is filed but the parent's milestone list is refetched asynchronously, so re-enabling now
    // would leave a window where a second click sees stale milestones and files a duplicate.
    if (!ok) setApplying(false);
  }

  async function save() {
    setSaving(true);
    await runMutation(async () => {
      await setProjectMetadata(project.name, {
        size,
        importance,
        blockedBy: blockedBy || null,
      });
      onSaved();
    }, setError);
    setSaving(false);
  }

  return (
    <div className="border-t border-border px-4 py-3" data-help="focus-triage">
      {/* One project, one model call, beside the fields it fills in — this used to be a header
          button that fanned out across every project at once. */}
      {!proposal && (
        <div className="mb-3">
          <Button
            variant="tertiary"
            onClick={onSuggest}
            disabled={suggestDisabled}
            data-help="focus-suggest"
            className="text-xs"
            title="Let the AI propose a size, parent, blocker and deadline for this project"
          >
            {suggesting ? "Suggesting…" : "Suggest attributes (AI)"}
          </Button>
        </div>
      )}
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
              disabled={applying}
              className="rounded-[var(--radius-sm)] px-2 py-0.5 text-accent-text transition hover:bg-accent-soft disabled:opacity-50"
            >
              Use it
            </button>
          </div>
          <p className="text-ink3">
            {proposal.size ? `size ${proposal.size}` : "no size"}
            {proposal.blocked_by ? ` · blocked by ${proposal.blocked_by}` : ""}
            {proposal.deadline ? ` · due ${formatDateOnly(proposal.deadline.slice(0, 10))}` : ""}
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
      </div>

      {/* Where the "Part of (parent)" picker used to sit (#278). That field pretended to be
          grouping while actually suppressing this project's status; the one real job it did —
          "this was never its own project" — is now this explicit, irreversible action (#279). */}
      <div className="mt-3 flex items-center justify-between gap-3">
        <span className="text-xs text-ink4">
          {otherProjects.length > 0
            ? "Turns out this was always part of another project?"
            : "Done with this project?"}
        </span>
        <div className="flex shrink-0 gap-2">
          {otherProjects.length > 0 && (
            <Button variant="tertiary" onClick={() => setMerging(true)}>
              Merge into…
            </Button>
          )}
          <Button variant="tertiary" onClick={() => setDeleting(true)}>
            Delete…
          </Button>
        </div>
      </div>
      {merging && (
        <MergeProjectDialog
          project={project.name}
          otherProjects={otherProjects}
          onClose={() => setMerging(false)}
          onMerged={onSaved}
        />
      )}
      {deleting && (
        <DeleteProjectDialog
          project={project.name}
          onClose={() => setDeleting(false)}
          onDeleted={onSaved}
        />
      )}

      <div className="mt-3">
        <span className="text-xs text-ink3">Milestones (the nearest unmet drives Due soon)</span>
        <div className="mt-1.5">
          <MilestoneList
            project={project.name}
            milestones={project.milestones}
            onChanged={onChanged}
          />
        </div>
      </div>

      {error && (
        <div
          role="alert"
          className="mt-3 rounded-[var(--radius-sm)] border px-3 py-2 text-xs text-st-due"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
          }}
        >
          {error}
        </div>
      )}

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
