// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { CalendarOverview, Conversation, LocalLlmStatus } from "../lib/types";
import { calendarOverview, listArchivedProjects, listProjects, onCalendarSynced } from "../lib/ipc";
import { archivedSet, isArchived } from "../lib/archivedProjects";
import { shortModel } from "../lib/format";
import { readHidden, readRosterOpen, writeHidden, writeRosterOpen } from "../lib/calendarPrefs";
import { localEndpointState, LOCAL_STATE_LABEL, LOCAL_STATE_TOKEN } from "../lib/localStatus";
import {
  ACTIVITY_DETAIL,
  ACTIVITY_LABEL,
  awaitsLocalModel,
  localModelActivity,
  type LocalModelActivity,
} from "../lib/localModelState";
import {
  keptLocalTitle,
  onBatteryRowTitle,
  POWER_DETAIL,
  powerOf,
  powerTag,
  type PowerTag,
} from "../lib/powerRoute";
import type { View } from "../lib/nav";
import {
  useDepth,
  useTheme,
  milestoneColor,
  pinboardColor,
  sourceColors,
  sourceShapeIndex,
  taskColor,
} from "../theme";
import {
  MILESTONE_CALENDAR_ID,
  PINBOARD_CALENDAR_ID,
  TASK_CALENDAR_ID,
} from "../lib/calendar-layout";
import { CalendarSourceList } from "./calendar/CalendarSourceList";
import { Button, Collapsible, ConfirmDialog, Dialog, NavItem, Select } from "./ui";
import { globalChats, projectChats } from "../lib/chatNav";
import { Briefing } from "./Briefing";
import { readBriefingInSidebar, subscribeBriefingPrefs } from "../lib/briefingPrefs";
import { readChatSectionOpen, writeChatSectionOpen, type ChatSectionId } from "../lib/chatPrefs";
import { SidebarNav } from "./SidebarNav";

/** The command-palette shortcut hint shown in the sidebar (⌘ on macOS). */
const SHORTCUT_HINT =
  typeof navigator !== "undefined" && /mac/i.test(navigator.platform) ? "⌘K" : "Ctrl K";

/** One foldable section of the Chats tab's sidebar, with a count and its own empty state. The
 *  heading keeps the mono/uppercase/ink4 treatment the single "Conversations" label had, so the
 *  two sections read as the same furniture rather than a new kind of thing.
 *
 *  The fold is remembered per device (lib/chatPrefs). It has to be: this whole block is gated on
 *  `view === "chat"`, so the section unmounts on every tab switch — before, that reseeded from
 *  `defaultOpen` and the user's choice was lost on navigation as well as on restart. Reading in a
 *  lazy initialiser therefore also repairs the tab-switch case for free, and the
 *  `pm:settings-changed` subscription covers the case the initialiser can't: Settings renders as an
 *  overlay over a STILL-MOUNTED Sidebar, so a reset from there has to reach us live. */
function ChatSection({
  id,
  title,
  count,
  empty,
  defaultOpen,
  children,
}: {
  id: ChatSectionId;
  title: string;
  count: number;
  empty: string;
  defaultOpen: boolean;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(() => readChatSectionOpen(id) ?? defaultOpen);
  useEffect(() => {
    const sync = () => setOpen(readChatSectionOpen(id) ?? defaultOpen);
    window.addEventListener("pm:settings-changed", sync);
    return () => window.removeEventListener("pm:settings-changed", sync);
  }, [id, defaultOpen]);
  return (
    <Collapsible
      className="pt-2"
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        writeChatSectionOpen(id, next);
      }}
      meta={count > 0 ? count : undefined}
      title={<span className="font-mono text-xs uppercase tracking-wide text-ink4">{title}</span>}
    >
      {count === 0 ? <p className="px-2 py-2 text-xs text-ink4">{empty}</p> : children}
    </Collapsible>
  );
}

/** One conversation row: the title, plus hover-revealed move/delete controls.
 *
 *  The controls sit OUTSIDE the NavItem (itself a <button>) — a button can't nest a button. They're
 *  hover-revealed siblings overlaid on the row's right edge; `pr-14` on the NavItem keeps a long
 *  title from sliding under them. */
function ConversationRow({
  conversation: c,
  active,
  onSelect,
  onMove,
  onDelete,
}: {
  conversation: Conversation;
  active: boolean;
  onSelect: (id: number) => void;
  onMove: (c: Conversation) => void;
  onDelete: (c: Conversation) => void;
}) {
  return (
    <div className="group relative">
      <NavItem active={active} onClick={() => onSelect(c.id)} className="mb-1 pr-14">
        <span title={c.title}>{c.title}</span>
      </NavItem>
      <div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
        <button
          type="button"
          onClick={() => onMove(c)}
          title="Move to a project"
          aria-label={`Move conversation “${c.title}” to a project`}
          className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 opacity-0 transition hover:bg-surface hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
        >
          <span aria-hidden="true">📁</span>
        </button>
        <button
          type="button"
          onClick={() => onDelete(c)}
          title="Delete chat"
          aria-label={`Delete conversation “${c.title}”`}
          className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 opacity-0 transition hover:bg-surface hover:text-st-due focus-visible:opacity-100 group-hover:opacity-100"
        >
          <span aria-hidden="true">🗑</span>
        </button>
      </div>
    </div>
  );
}

interface Props {
  view: View;
  onNavigate: (view: View) => void;
  conversations: Conversation[];
  activeId: number | null;
  reviewCount: number;
  onSelect: (id: number) => void;
  /** Delete a conversation for good (card 7G) — the parent owns the mutation + list refresh. */
  onDelete: (id: number) => void;
  /** Move a conversation into a project, or back to global with `null` (card B). The parent owns the
   *  mutation + list refresh (and, in a project pane, resetting if the open chat leaves the project). */
  onMove: (id: number, project: string | null) => void;
  /** Start a new GLOBAL conversation — always, from any tab. Deliberately not context-sensitive:
   *  this hangs off the Chats row, and a button there can only honestly mean "a new chat in Chats".
   *  Making it quietly project-scoped whenever a project happened to be open is what made its
   *  predecessor unreadable — you could not tell from looking at it what it would make. */
  onNew: () => void;
  /** Start a new conversation IN THE OPEN PROJECT. Separate from `onNew` because they make
   *  different things, and each sits above the list it adds to. */
  onNewProjectChat: () => void;
  /** Open a project's scoped view from the Chats tab's Projects section. App owns the navigation
   *  (the same `openProject` the Focus cards and the command palette use). */
  onOpenProject: (project: string) => void;
  onOpenSettings: () => void;
  onOpenWhatsNew: () => void;
  onOpenPalette: () => void;
  /** A better-fitting local model is available (#437) — marks the Settings row with a quiet dot. */
  betterFit: boolean;
  /** A connected Google account predates the Sheets permission — same quiet dot, same row. */
  sheetsNudge: boolean;
  /** Active (primary) models, shown in the footer tag; null → using the default. */
  chatModel: string | null;
  backgroundModel: string | null;
  /** How many auto-switch fallbacks are configured behind each primary (0 = none). */
  chatFallbacks: number;
  backgroundFallbacks: number;
  /** Live local-endpoint status (#297). Adds a coloured "Local" line to the model footer — but ONLY
   *  when an endpoint is configured; `null`/unconfigured renders nothing (zero-pixel for cloud-only). */
  localAi: LocalLlmStatus | null;
  /** Live width (px) from the resize hook, and its drag handle + feedback (owned by App so the
   *  collapsed reopen tab can render in the sidebar's place). */
  width: number;
  onStartResize: (e: React.PointerEvent) => void;
  resizing: boolean;
}

export function Sidebar({
  view,
  onNavigate,
  conversations,
  activeId,
  reviewCount,
  onSelect,
  onDelete,
  onMove,
  onNew,
  onNewProjectChat,
  onOpenProject,
  onOpenSettings,
  onOpenWhatsNew,
  onOpenPalette,
  betterFit,
  sheetsNudge,
  chatModel,
  backgroundModel,
  chatFallbacks,
  backgroundFallbacks,
  localAi,
  width,
  onStartResize,
  resizing,
}: Props) {
  const { minimal } = useDepth();
  // Which tabs show, and in what order, is lib/nav's (drawn by SidebarNav): Depth's reveals, the
  // Dev tab's developer mode, and what the user pinned or hid.
  const { modelsVisible, system, accent, colorblind } = useTheme();
  // The conversation awaiting a delete confirmation (null = no dialog open). Held here so the row's
  // hover trash and the confirm modal stay a local concern; the actual purge is `onDelete` (App owns
  // the mutation + list refresh + reselecting after the active chat is deleted).
  const [pendingDelete, setPendingDelete] = useState<Conversation | null>(null);
  // The conversation whose "move to project" picker is open (null = none). Like the delete flow, the
  // row's hover control and the picker modal stay local; the actual reassignment is `onMove` (the parent
  // owns the mutation + list refresh).
  const [pendingMove, setPendingMove] = useState<Conversation | null>(null);
  // The Projects section needs every project, not just those with chats: `list_projects` is
  // DISTINCT over documents, so the two inputs each cover a gap in the other (see chatNav.ts).
  // Loaded once and refreshed whenever the roster changes, which is when a new project can appear.
  const [knownProjects, setKnownProjects] = useState<string[]>([]);
  // Archived projects (#1018) leave this section, chats and all; their chats stay reachable from
  // the project itself, which Focus's Archive folder opens.
  const [archivedNames, setArchivedNames] = useState<string[]>([]);
  useEffect(() => {
    if (view !== "chat") return;
    let alive = true;
    listProjects()
      .then((p) => alive && setKnownProjects(p))
      .catch(() => {
        /* best-effort: the union still covers every project that has a chat */
      });
    listArchivedProjects()
      .then((a) => alive && setArchivedNames(a))
      .catch(() => {
        /* best-effort: nothing hidden */
      });
    return () => {
      alive = false;
    };
  }, [view, conversations]);
  const projectGroups = useMemo(() => {
    const archived = archivedSet(archivedNames);
    return projectChats(conversations, knownProjects).filter(
      (g) => !isArchived(archived, g.project),
    );
  }, [conversations, knownProjects, archivedNames]);
  const unscoped = useMemo(() => globalChats(conversations), [conversations]);
  // Settings renders as an overlay over a still-mounted Sidebar, so a read-at-mount would leave the
  // toggle looking broken until the user navigated away and back. Subscribe instead.
  const [briefingInSidebar, setBriefingInSidebar] = useState(readBriefingInSidebar);
  useEffect(() => subscribeBriefingPrefs(() => setBriefingInSidebar(readBriefingInSidebar())), []);

  // --- the calendar roster (was a "Calendars x/x" dropdown in the calendar header) --------------
  //
  // The sidebar reads the roster itself rather than having CalendarView lift `overview`/`hidden` up
  // through App and back down — structurally the same shape as the `listProjects()` effect above, and
  // it keeps the calendar's state where it already lives. Only while the Calendar tab is open: the
  // sidebar never unmounts, so rendering it always would make it a permanent app-wide widget.
  const [calOverview, setCalOverview] = useState<CalendarOverview | null>(null);
  const [calHidden, setCalHidden] = useState<Set<string>>(readHidden);
  const [rosterOpen, setRosterOpen] = useState(readRosterOpen);
  useEffect(() => {
    if (view !== "calendar") return;
    let alive = true;
    let unlisten: (() => void) | null = null;
    const load = () =>
      calendarOverview()
        .then((o) => alive && setCalOverview(o))
        .catch(() => {
          /* best-effort: no roster simply means no block */
        });
    void load();
    // A calendar ticked or unticked in Settings → Connectors, or a sync that found a new one, says so
    // here. Settings opens over this still-mounted sidebar, so reading only on the way into the tab
    // left an unticked calendar listed until PM restarted.
    void onCalendarSynced(() => void load())
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [view]);
  // The grid writes the same set, and this sidebar is mounted alongside it, so follow the app-wide
  // signal writeHidden emits — otherwise the two lists would disagree until a remount.
  useEffect(() => {
    const sync = () => setCalHidden(readHidden());
    window.addEventListener("pm:settings-changed", sync);
    return () => window.removeEventListener("pm:settings-changed", sync);
  }, []);
  function toggleCalendar(id: string) {
    const next = new Set(calHidden);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setCalHidden(next);
    writeHidden(next); // announces, so the grid follows
  }
  // Pure functions of the id SET (sorted internally), so this map provably matches the grid's without
  // any plumbing between them.
  const calColorOf = useMemo(() => {
    const ids = calOverview?.calendars.map((c) => c.id) ?? [];
    const map = sourceColors(ids, system, accent, colorblind);
    // The Personal Manager rows take the colours their entries have on the calendar (as
    // CalendarView's `colorOf` does), not the grey fallback for an unknown calendar.
    return (id: string) => {
      if (id === MILESTONE_CALENDAR_ID) return milestoneColor(system);
      if (id === PINBOARD_CALENDAR_ID) return pinboardColor(system);
      if (id === TASK_CALENDAR_ID) return taskColor(system);
      return map.get(id) ?? "var(--ink4)";
    };
  }, [calOverview, system, accent, colorblind]);
  const calShapeOf = useMemo(() => {
    const slots = sourceShapeIndex(calOverview?.calendars.map((c) => c.id) ?? []);
    return (id: string) => slots.get(id);
  }, [calOverview]);
  return (
    <aside
      style={{ width }}
      className="relative flex h-full flex-col overflow-hidden border-r border-border bg-panel"
    >
      {/* Right-edge grip: drag to resize; drag all the way to the window edge to snap it shut. */}
      <div
        onPointerDown={onStartResize}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize sidebar"
        title="Drag to resize · drag to the edge to hide"
        className={`absolute right-0 top-0 z-10 h-full w-1.5 cursor-col-resize touch-none transition-colors hover:bg-[color-mix(in_oklab,var(--accent)_45%,transparent)] ${
          resizing ? "bg-[color-mix(in_oklab,var(--accent)_60%,transparent)]" : ""
        }`}
      />
      {/* Search sits directly under the window chrome. The name-and-Alpha badge that used to be
          above it is gone — the chrome carries both a few pixels higher, now with the version
          number attached, and two of them side by side was the same fact stated twice. What that
          left behind was a row rendered UNCONDITIONALLY to hold the New button, which only appears
          on two tabs: every other tab paid its full height as blank space between the chrome and
          Search, and on the two tabs that did show it, one control pushed the whole sidebar down to
          claim a line of its own. New now lives on the Chats row below, where it belongs, and the
          padding here is even with the nav (`p-2` here, `px-2` there). */}
      <div className="shrink-0 p-2">
        <button
          onClick={onOpenPalette}
          data-help="sidebar-search"
          title="Search and jump to any project, file, conversation, or calendar event"
          className="flex w-full items-center justify-between rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-1.5 text-left text-sm text-ink4 hover:text-ink2"
        >
          <span>Search…</span>
          <span className="ml-2 shrink-0 font-mono text-xs text-ink4">{SHORTCUT_HINT}</span>
        </button>
      </div>

      {/* Nav + conversations share one scroller: with every optional tab shown, a short window or a
          large text size used to push the footer (What's New / Settings) past the bottom edge, where
          the shell's `overflow-hidden` clipped it and left no way to reach it. `min-h-0` is what lets
          this shrink below its content so the scrollbar appears; the app-wide axis normaliser
          (installed once in App) covers wheel behaviour, so nothing is wired per-surface here. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto overflow-x-hidden">
        <SidebarNav view={view} onNavigate={onNavigate} reviewCount={reviewCount} onNew={onNew} />

        <div className="px-2">
          {/* The Chats tab shows two sections — your projects, and the chats that belong to no
              project. An OPEN project keeps the old single flat list, fed from App's project chat
              session: in there the sidebar is that project's own history, and a Projects section
              listing every other project would just be a way to leave. */}
          {view === "chat" && (
            <div data-help="conversations-list">
              <ChatSection
                id="projects"
                title="Projects"
                count={projectGroups.length}
                empty="No projects yet."
                defaultOpen={!minimal}
              >
                {projectGroups.map((g) => (
                  <NavItem
                    key={g.project}
                    active={false}
                    onClick={() => onOpenProject(g.project)}
                    className="mb-1"
                  >
                    <span className="flex items-center justify-between gap-2">
                      <span className="truncate" title={g.project}>
                        {g.project}
                      </span>
                      {g.chats.length > 0 && (
                        <span
                          className="shrink-0 font-mono text-xs text-ink4"
                          title={`${g.chats.length} chat${g.chats.length === 1 ? "" : "s"} in this project`}
                        >
                          {g.chats.length}
                        </span>
                      )}
                    </span>
                  </NavItem>
                ))}
              </ChatSection>

              <ChatSection
                id="global"
                title="Global chats"
                count={unscoped.length}
                empty="No global chats yet."
                defaultOpen
              >
                {unscoped.map((c) => (
                  <ConversationRow
                    key={c.id}
                    conversation={c}
                    active={c.id === activeId}
                    onSelect={onSelect}
                    onMove={setPendingMove}
                    onDelete={setPendingDelete}
                  />
                ))}
              </ChatSection>
            </div>
          )}
          {view === "project" && (
            <div data-help="conversations-list">
              {/* The project's own New, over the project's own list. Until now the ONLY unconditional
                  way to start a project chat was the sidebar's top New button, which read as a global
                  control; the only other route was the "this conversation has been idle — start a new
                  one?" prompt in ProjectView, which by definition isn't there when you want it. */}
              <div className="flex items-center justify-between gap-1 px-2 pb-1 pt-2">
                <p className="min-w-0 truncate font-mono text-xs uppercase tracking-wide text-ink4">
                  Conversations
                </p>
                <button
                  onClick={onNewProjectChat}
                  title="New conversation in this project"
                  aria-label="New conversation in this project"
                  className="shrink-0 rounded-[var(--radius-sm)] px-1 py-0.5 text-xs text-ink4 transition hover:bg-surface hover:text-ink"
                >
                  + New
                </button>
              </div>
              {conversations.length === 0 && (
                <p className="px-2 py-2 text-xs text-ink4">No conversations yet.</p>
              )}
              {conversations.map((c) => (
                <ConversationRow
                  key={c.id}
                  conversation={c}
                  active={c.id === activeId}
                  onSelect={onSelect}
                  onMove={setPendingMove}
                  onDelete={setPendingDelete}
                />
              ))}
            </div>
          )}
        </div>
      </div>

      <ConfirmDialog
        open={pendingDelete != null}
        title="Delete this conversation?"
        danger
        confirmLabel="Delete"
        onConfirm={() => {
          const id = pendingDelete?.id;
          setPendingDelete(null);
          if (id != null) onDelete(id);
        }}
        onClose={() => setPendingDelete(null)}
      >
        {pendingDelete && (
          <p className="text-ink2">
            “{pendingDelete.title}” and its messages will be permanently deleted, along with
            anything it added to search. This can’t be undone.
          </p>
        )}
      </ConfirmDialog>

      {pendingMove && (
        <MoveConversationDialog
          conversation={pendingMove}
          onClose={() => setPendingMove(null)}
          onMove={(project) => {
            const id = pendingMove.id;
            setPendingMove(null);
            onMove(id, project);
          }}
        />
      )}

      <div className="shrink-0 border-t border-border p-2">
        {/* The calendar roster, above the briefing. Collapsible and capped with its own scroller:
            `calendar_overview` returns EVERY registered calendar for every connected source with no
            `selected` filter, so a couple of Google accounts plus a feed is comfortably 15-25 rows —
            enough to push the model rows, What's New and Settings off the bottom of a fixed-height
            footer. Unselected calendars are filtered out: they mirror no events and can never show
            one, which is far more conspicuous inline than it was inside a popover. */}
        {view === "calendar" && calOverview && (
          <div className="mb-1 border-b border-border pb-2" data-help="calendar-filter">
            {/* Controlled, not `defaultOpen`: this block unmounts whenever you leave the Calendar
                tab, so an uncontrolled fold reseeded itself open on the way back. The state lives in
                Sidebar (which never unmounts) AND in localStorage, so the choice survives both a tab
                change and a restart. */}
            <Collapsible
              title="Calendars"
              meta={`${calOverview.calendars.filter((c) => c.selected && !calHidden.has(c.id)).length}/${
                calOverview.calendars.filter((c) => c.selected).length
              }`}
              open={rosterOpen}
              onOpenChange={(o) => {
                setRosterOpen(o);
                writeRosterOpen(o);
              }}
            >
              <div className="max-h-64 overflow-y-auto">
                <CalendarSourceList
                  accounts={calOverview.accounts}
                  calendars={calOverview.calendars}
                  hidden={calHidden}
                  onToggle={toggleCalendar}
                  colorOf={calColorOf}
                  shapeOf={calShapeOf}
                  hideUnselected
                />
              </div>
            </Collapsible>
          </div>
        )}
        {/* Today's briefing, off by default and switched on in Settings. It sits at the top of the
            footer — above the model rows at Standard/Power, above What's New at Minimal — so it is
            pinned outside the scroller above and stays put whichever tab is open. `Briefing` renders
            nothing at all when there's no briefing yet, so an enabled-but-empty store leaves no
            stray box or divider behind. */}
        {briefingInSidebar && (
          <div className="mb-1 border-b border-border px-1 pb-2">
            <Briefing variant="panel" />
          </div>
        )}
        {/* The model footer is an optional feature reveal. It follows the Depth preset by default,
            but is now a switch of its own in Settings → General → Appearance — so you can see which
            model is answering on Minimal, or hide it on Power. Gate the entire button (not just the
            rows) so no empty, hover-highlighting ghost box is left behind and the divider sits
            directly above "What's New". */}
        {modelsVisible && (
          <button
            onClick={onOpenSettings}
            data-help="sidebar-models"
            title="Models in use — click to change"
            className="mb-1 w-full rounded-[var(--radius-sm)] px-3 py-1.5 text-left hover:bg-surface"
          >
            {/* `id` is the OpenRouter list; `local` is what routing will really reach for. The
                rows used to render `id` unconditionally with no access to routing at all, so a
                machine answering every turn from its own GPU displayed a cloud model's name — and
                read together with "Local connected" below, the footer stated the exact inverse of
                what was happening. */}
            <ModelRow
              role="Chat"
              id={chatModel}
              local={localAi?.chat_local_model ?? null}
              awaitingModel={awaitsLocalModel(localAi, "chat")}
              activity={localModelActivity(localAi, "chat")}
              power={powerTag(localAi, "chat")}
              parked={localAi ? powerOf(localAi).chat.local_model : null}
              fallbacks={chatFallbacks}
            />
            <ModelRow
              role="Tasks"
              id={backgroundModel}
              local={localAi?.background_local_model ?? null}
              awaitingModel={awaitsLocalModel(localAi, "background")}
              activity={localModelActivity(localAi, "background")}
              power={powerTag(localAi, "background")}
              parked={localAi ? powerOf(localAi).background.local_model : null}
              fallbacks={backgroundFallbacks}
            />
            <LocalRow status={localAi} />
          </button>
        )}
        <button
          onClick={onOpenWhatsNew}
          className="w-full rounded-[var(--radius-sm)] px-3 py-2 text-left text-sm text-ink3 hover:bg-surface hover:text-ink"
        >
          What's New
        </button>
        <button
          onClick={onOpenSettings}
          className="flex w-full items-center gap-2 rounded-[var(--radius-sm)] px-3 py-2 text-left text-sm text-ink3 hover:bg-surface hover:text-ink"
        >
          <span className="min-w-0 flex-1 truncate">Settings</span>
          {/* A better-fitting local model is available (#437). A quiet dot, never a count or a
              badge — it points at somewhere worth looking, it doesn't demand anything. It clears
              the moment the suggestion is dismissed. */}
          {betterFit && <AttentionDot label="A better-fitting local model is available" />}
          {!betterFit && sheetsNudge && (
            <AttentionDot label="A Google account needs reconnecting to index Sheets" />
          )}
        </button>
      </div>
    </aside>
  );
}

/** One line of the footer model tag: role label, active model, what it is doing, fallback count. */
function ModelRow({
  role,
  id,
  local,
  awaitingModel,
  activity,
  power,
  parked,
  fallbacks,
}: {
  role: string;
  id: string | null;
  /** The local model this role will really reach for, when routing sends it there. */
  local: string | null;
  /** The role is routed to a local model and nothing answers it until one is picked (D7). */
  awaitingModel: boolean;
  /** What that model is doing right now, or `null` when there is nothing PM can honestly say. */
  activity: LocalModelActivity | null;
  /** What the On battery policy is doing to this role, or `null` when nothing (#432). */
  power: PowerTag | null;
  /** The role's bound local model, which the policy has parked while `power` is "on_battery". */
  parked: string | null;
  fallbacks: number;
}) {
  // On battery the backend already reports no local model for a moved role, so `shown` is the cloud
  // model answering and `activity` is null — only the title and the chip need to say why.
  const shown = local ?? id;
  const title = awaitingModel
    ? "Nothing answers this until you pick a model. Click to choose one."
    : power === "on_battery"
      ? onBatteryRowTitle(id, parked)
      : local
        ? `${local} — running on this machine${id ? `; ${id} is the cloud fallback` : ""}`
        : (id ?? "Using the default model");
  return (
    <div className="flex items-center gap-1.5 text-xs leading-5">
      <span className="w-9 shrink-0 font-mono text-ink4">{role}</span>
      {/* A role with nothing to answer it never shows the cloud model's name: that would read as
          the cloud answering, which is the one thing the Local AI switch promises it won't do. */}
      <span
        className={`min-w-0 flex-1 truncate ${awaitingModel ? "text-ink4" : "text-ink3"}`}
        title={title}
      >
        {awaitingModel ? "pick a model" : shown ? shortModel(shown) : "default"}
      </span>
      {/* The live half. It renders NOTHING at all when PM cannot honestly answer — a role using
          cloud, an endpoint with no `/api/ps`, or nothing observed recently enough to repeat — so a
          row that used to say only a model name still says only a model name. Answering gets the
          same soft accent chip the fallback count wears; the three resting states stay quiet text,
          because "not loaded" is ordinary and a permanently highlighted footer is noise. */}
      {activity && (
        <span
          className={
            activity === "answering"
              ? "shrink-0 rounded-[var(--radius-sm)] bg-accent-soft px-1 text-[0.625rem] text-accent-text"
              : "shrink-0 text-[0.625rem] text-ink4"
          }
          title={ACTIVITY_DETAIL[activity]}
        >
          {ACTIVITY_LABEL[activity]}
        </span>
      )}
      {/* The On battery marks wear the RESTING style: a deliberate policy, never the accent
          "answering" chip and never `--st-look`, which is the failure family's colour. */}
      {power === "on_battery" && (
        <span className="shrink-0 text-[0.625rem] text-ink4" title={POWER_DETAIL}>
          on battery
        </span>
      )}
      {power === "kept_local" && (
        <span className="shrink-0 text-[0.625rem] text-ink4" title={keptLocalTitle(local)}>
          kept local
        </span>
      )}
      {/* The fallbacks are cloud models, and none of them answers a role waiting for a pick. */}
      {!awaitingModel && fallbacks > 0 && (
        <span
          className="shrink-0 rounded-[var(--radius-sm)] bg-accent-soft px-1 font-mono text-[0.625rem] text-accent-text"
          title={`${fallbacks} auto-switch fallback${fallbacks === 1 ? "" : "s"}`}
        >
          +{fallbacks}
        </span>
      )}
    </div>
  );
}

/** The local-endpoint status line in the model footer (#297). Renders NOTHING unless an endpoint is
 *  configured (the zero-pixel contract). "resting" is the dead-host cooldown, during which background
 *  work goes to cloud — the honest signal a user who chose local wants to see. */
function LocalRow({ status }: { status: LocalLlmStatus | null }) {
  const state = localEndpointState(status);
  if (state === null) return null;
  const label = LOCAL_STATE_LABEL[state];
  return (
    <div className="flex items-center gap-1.5 text-xs leading-5">
      <span className="w-9 shrink-0 font-mono text-ink4">Local</span>
      <span
        className="min-w-0 flex-1 truncate"
        style={{ color: `var(${LOCAL_STATE_TOKEN[state]})` }}
        title="Local model endpoint status — click to manage in Settings"
      >
        {label}
      </span>
    </div>
  );
}

/** The "move a chat into a project (or back to global)" picker (card B, chat transfer). Fetches the
 *  project list lazily on open, defaults the selection to the chat's current home, and only enables
 *  Move when the target actually changes. The reassignment itself is the parent's `onMove`. */
function MoveConversationDialog({
  conversation,
  onClose,
  onMove,
}: {
  conversation: Conversation;
  onClose: () => void;
  onMove: (project: string | null) => void;
}) {
  // null = still loading the project list; [] = loaded, none exist yet.
  const [projects, setProjects] = useState<string[] | null>(null);
  const current = conversation.project ?? "";
  const [target, setTarget] = useState(current);

  useEffect(() => {
    let alive = true;
    listProjects()
      .then((p) => alive && setProjects(p))
      .catch(() => alive && setProjects([]));
    return () => {
      alive = false;
    };
  }, []);

  // Always offer the chat's own project even if `list_projects` doesn't surface it (e.g. a project with
  // no ingested docs yet), so the current home is representable and "unchanged" is detectable.
  const options =
    projects == null
      ? []
      : current !== "" && !projects.includes(current)
        ? [current, ...projects]
        : projects;

  return (
    <Dialog
      open
      onClose={onClose}
      widthClassName="max-w-md"
      title="Move conversation"
      footer={
        <>
          <Button variant="tertiary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => onMove(target === "" ? null : target)}
            disabled={projects == null || target === current}
          >
            Move
          </Button>
        </>
      }
    >
      <p className="mt-2 text-sm leading-relaxed text-ink3">
        Choose where “{conversation.title}” lives. A project chat searches only that project’s
        files; a global chat searches everything. Past messages stay put — only where it’s filed
        changes.
      </p>
      <Select
        className="mt-4 w-full"
        value={target}
        onChange={(e) => setTarget(e.target.value)}
        disabled={projects == null}
        aria-label="Destination project"
      >
        <option value="">No project (global)</option>
        {options.map((p) => (
          <option key={p} value={p}>
            {p}
          </option>
        ))}
      </Select>
    </Dialog>
  );
}

/** A small accent dot marking a row worth visiting — quieter than a count, and carrying no number
 *  because there is nothing to count. Exported so Settings' own nav can mark the same thing. */
export function AttentionDot({ label }: { label: string }) {
  return (
    <span
      role="status"
      aria-label={label}
      title={label}
      className="ml-2 inline-block h-1.5 w-1.5 shrink-0 rounded-full bg-accent"
    />
  );
}
