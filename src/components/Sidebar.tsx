// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import type { Conversation, LocalLlmStatus } from "../lib/types";
import { listProjects } from "../lib/ipc";
import { shortModel } from "../lib/format";
import { localEndpointState, LOCAL_STATE_TOKEN } from "../lib/localStatus";
import { useDevMode } from "../lib/capabilities";
import { useDepth, useTheme } from "../theme";
import { Button, Collapsible, ConfirmDialog, Modal, NavItem, Select } from "./ui";
import { globalChats, projectChats } from "../lib/chatNav";
import { Briefing } from "./Briefing";
import { readBriefingInSidebar, subscribeBriefingPrefs } from "../lib/briefingPrefs";

export type View =
  | "focus"
  | "project"
  | "chat"
  | "calendar"
  | "documents"
  | "review"
  | "teach"
  | "graph"
  | "pinboard"
  | "dev";

/** The command-palette shortcut hint shown in the sidebar (⌘ on macOS). */
const SHORTCUT_HINT =
  typeof navigator !== "undefined" && /mac/i.test(navigator.platform) ? "⌘K" : "Ctrl K";

/** One foldable section of the Chats tab's sidebar, with a count and its own empty state. The
 *  heading keeps the mono/uppercase/faint treatment the single "Conversations" label had, so the
 *  two sections read as the same furniture rather than a new kind of thing. */
function ChatSection({
  title,
  count,
  empty,
  defaultOpen,
  children,
}: {
  title: string;
  count: number;
  empty: string;
  defaultOpen: boolean;
  children: ReactNode;
}) {
  return (
    <Collapsible
      className="pt-2"
      defaultOpen={defaultOpen}
      meta={count > 0 ? count : undefined}
      title={<span className="font-mono text-xs uppercase tracking-wide text-faint">{title}</span>}
    >
      {count === 0 ? <p className="px-2 py-2 text-xs text-faint">{empty}</p> : children}
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
  onNew: () => void;
  /** Open a project's scoped view from the Chats tab's Projects section. App owns the navigation
   *  (the same `openProject` the Focus cards and the command palette use). */
  onOpenProject: (project: string) => void;
  onOpenSettings: () => void;
  onOpenWhatsNew: () => void;
  onOpenPalette: () => void;
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
  onOpenProject,
  onOpenSettings,
  onOpenWhatsNew,
  onOpenPalette,
  chatModel,
  backgroundModel,
  chatFallbacks,
  backgroundFallbacks,
  localAi,
  width,
  onStartResize,
  resizing,
}: Props) {
  const { showMeta, minimal } = useDepth();
  // The Teach tab is a Depth-keyed feature reveal (hidden for the minimalist preset), overridable
  // in Settings. Hiding it hides only the editor — deterministic alias resolution keeps running.
  const { teachVisible, mapVisible } = useTheme();
  // The Dev tab is an orthogonal capability reveal (issue #78) — independent of Depth, shown only
  // when the user turns Developer mode on in Settings.
  const { devMode } = useDevMode();
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
  useEffect(() => {
    if (view !== "chat") return;
    let alive = true;
    listProjects()
      .then((p) => alive && setKnownProjects(p))
      .catch(() => {
        /* best-effort: the union still covers every project that has a chat */
      });
    return () => {
      alive = false;
    };
  }, [view, conversations]);
  const projectGroups = useMemo(
    () => projectChats(conversations, knownProjects),
    [conversations, knownProjects],
  );
  const unscoped = useMemo(() => globalChats(conversations), [conversations]);
  // Settings renders as an overlay over a still-mounted Sidebar, so a read-at-mount would leave the
  // toggle looking broken until the user navigated away and back. Subscribe instead.
  const [briefingInSidebar, setBriefingInSidebar] = useState(readBriefingInSidebar);
  useEffect(() => subscribeBriefingPrefs(() => setBriefingInSidebar(readBriefingInSidebar())), []);
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
      <div className="flex shrink-0 items-center justify-between px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="font-head text-sm font-semibold tracking-wide text-ink">PM</span>
          <span
            title="PM is in alpha — under active development; expect rough edges and changes between updates."
            className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[0.625rem] font-medium uppercase tracking-wide text-accent-text"
          >
            Alpha
          </span>
        </div>
        {(view === "chat" || view === "project") && (
          <button
            onClick={onNew}
            title="New conversation"
            className="rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink3 hover:bg-surface hover:text-ink"
          >
            + New
          </button>
        )}
      </div>

      <div className="shrink-0 px-2 pb-2">
        <button
          onClick={onOpenPalette}
          data-help="sidebar-search"
          title="Search and jump to any project, file, or conversation"
          className="flex w-full items-center justify-between rounded-[var(--radius-sm)] border border-border bg-surface px-3 py-1.5 text-left text-sm text-ink4 hover:text-ink2"
        >
          <span>Search…</span>
          <span className="font-mono text-xs text-faint">{SHORTCUT_HINT}</span>
        </button>
      </div>

      {/* Nav + conversations share one scroller: with every optional tab shown, a short window or a
          large text size used to push the footer (What's New / Settings) past the bottom edge, where
          the shell's `overflow-hidden` clipped it and left no way to reach it. `min-h-0` is what lets
          this shrink below its content so the scrollbar appears; the app-wide axis normaliser
          (installed once in App) covers wheel behaviour, so nothing is wired per-surface here. */}
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto overflow-x-hidden">
        <nav className="flex flex-col gap-1 px-2 pb-2">
          <NavItem
            active={view === "focus" || view === "project"}
            onClick={() => onNavigate("focus")}
            helpId="nav-focus"
          >
            Focus
          </NavItem>
          <NavItem active={view === "chat"} onClick={() => onNavigate("chat")} helpId="nav-chat">
            Chats
          </NavItem>
          <NavItem
            active={view === "calendar"}
            onClick={() => onNavigate("calendar")}
            helpId="nav-calendar"
          >
            Calendar
          </NavItem>
          <NavItem
            active={view === "documents"}
            onClick={() => onNavigate("documents")}
            helpId="nav-documents"
          >
            Documents
          </NavItem>
          {/* Review + Teach are the "learning tools" — shown/hidden together by `teachVisible`. */}
          {teachVisible && (
            <NavItem
              active={view === "review"}
              onClick={() => onNavigate("review")}
              helpId="nav-review"
              trailing={<CountBadge count={reviewCount} />}
            >
              Review
            </NavItem>
          )}
          {teachVisible && (
            <NavItem
              active={view === "teach"}
              onClick={() => onNavigate("teach")}
              helpId="nav-teach"
            >
              Teach
            </NavItem>
          )}
          {mapVisible && (
            <NavItem
              active={view === "graph"}
              onClick={() => onNavigate("graph")}
              helpId="nav-graph"
            >
              Map
            </NavItem>
          )}
          <NavItem
            active={view === "pinboard"}
            onClick={() => onNavigate("pinboard")}
            helpId="nav-pinboard"
          >
            Pinboard
          </NavItem>
          {devMode && (
            <NavItem active={view === "dev"} onClick={() => onNavigate("dev")} helpId="nav-dev">
              Dev
            </NavItem>
          )}
        </nav>

        <div className="px-2">
          {/* The Chats tab shows two sections — your projects, and the chats that belong to no
              project. An OPEN project keeps the old single flat list, fed from App's project chat
              session: in there the sidebar is that project's own history, and a Projects section
              listing every other project would just be a way to leave. */}
          {view === "chat" && (
            <div data-help="conversations-list">
              <ChatSection
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
              <p className="px-2 pb-1 pt-2 font-mono text-xs uppercase tracking-wide text-faint">
                Conversations
              </p>
              {conversations.length === 0 && (
                <p className="px-2 py-2 text-xs text-faint">No conversations yet.</p>
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
        {/* The model footer is an optional feature reveal — hidden whole in Minimal mode. Gate the
            entire button (not just the rows) so no empty, hover-highlighting ghost box is left
            behind and the divider sits directly above "What's New". */}
        {showMeta && (
          <button
            onClick={onOpenSettings}
            data-help="sidebar-models"
            title="Models in use — click to change"
            className="mb-1 w-full rounded-[var(--radius-sm)] px-3 py-1.5 text-left hover:bg-surface"
          >
            <ModelRow role="Chat" id={chatModel} fallbacks={chatFallbacks} />
            <ModelRow role="Tasks" id={backgroundModel} fallbacks={backgroundFallbacks} />
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
          className="w-full rounded-[var(--radius-sm)] px-3 py-2 text-left text-sm text-ink3 hover:bg-surface hover:text-ink"
        >
          Settings
        </button>
      </div>
    </aside>
  );
}

/** One line of the footer model tag: role label, active model, fallback count. */
function ModelRow({ role, id, fallbacks }: { role: string; id: string | null; fallbacks: number }) {
  return (
    <div className="flex items-center gap-1.5 text-xs leading-5">
      <span className="w-9 shrink-0 font-mono text-faint">{role}</span>
      <span className="min-w-0 flex-1 truncate text-ink3" title={id ?? "Using the default model"}>
        {id ? shortModel(id) : "default"}
      </span>
      {fallbacks > 0 && (
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

/** Drop the provider prefix for a compact label ("anthropic/claude-x" → "claude-x"). */
/** The local-endpoint status line in the model footer (#297). Renders NOTHING unless an endpoint is
 *  configured (the zero-pixel contract). "resting" is the dead-host cooldown, during which background
 *  work goes to cloud — the honest signal a user who chose local wants to see. */
function LocalRow({ status }: { status: LocalLlmStatus | null }) {
  const state = localEndpointState(status);
  if (state === null) return null;
  const label =
    state === "connected"
      ? "connected"
      : state === "resting"
        ? "resting · using cloud"
        : "unreachable";
  return (
    <div className="flex items-center gap-1.5 text-xs leading-5">
      <span className="w-9 shrink-0 font-mono text-faint">Local</span>
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
  const titleId = useId();
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
    <Modal open onClose={onClose} labelledBy={titleId} widthClassName="max-w-md">
      <div className="p-5">
        <h2 id={titleId} className="font-head text-base font-semibold text-ink">
          Move conversation
        </h2>
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
        <div className="mt-5 flex justify-end gap-2">
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
        </div>
      </div>
    </Modal>
  );
}

/** The pill count shown on a nav row (e.g. pending reviews); renders nothing at zero. */
function CountBadge({ count }: { count?: number }) {
  if (count == null || count <= 0) return null;
  return (
    <span className="ml-2 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 font-mono text-xs font-medium text-accent-text">
      {count}
    </span>
  );
}
