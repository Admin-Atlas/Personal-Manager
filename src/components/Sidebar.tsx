// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useId, useState } from "react";
import type { Conversation } from "../lib/types";
import { listProjects } from "../lib/ipc";
import { useDevMode } from "../lib/capabilities";
import { useDepth, useTheme } from "../theme";
import { Button, ConfirmDialog, Modal, NavItem, Select } from "./ui";

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
  onOpenSettings: () => void;
  onOpenWhatsNew: () => void;
  onOpenPalette: () => void;
  /** Active (primary) models, shown in the footer tag; null → using the default. */
  chatModel: string | null;
  backgroundModel: string | null;
  /** How many auto-switch fallbacks are configured behind each primary (0 = none). */
  chatFallbacks: number;
  backgroundFallbacks: number;
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
  onOpenSettings,
  onOpenWhatsNew,
  onOpenPalette,
  chatModel,
  backgroundModel,
  chatFallbacks,
  backgroundFallbacks,
  width,
  onStartResize,
  resizing,
}: Props) {
  const { showMeta } = useDepth();
  // The Teach tab is a Depth-keyed feature reveal (hidden for the minimalist preset), overridable
  // in Settings. Hiding it hides only the editor — deterministic alias resolution keeps running.
  const { teachVisible } = useTheme();
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
  return (
    <aside
      style={{ width }}
      className="relative flex h-full flex-col border-r border-border bg-panel"
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
      <div className="flex items-center justify-between px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="font-head text-sm font-semibold tracking-wide text-ink">PM</span>
          <span
            title="PM is in alpha — under active development; expect rough edges and changes between updates."
            className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-wide text-accent-text"
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

      <div className="px-2 pb-2">
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

      <nav className="flex flex-col gap-1 px-2 pb-2">
        <NavItem
          active={view === "focus" || view === "project"}
          onClick={() => onNavigate("focus")}
          helpId="nav-focus"
        >
          Focus
        </NavItem>
        <NavItem active={view === "chat"} onClick={() => onNavigate("chat")} helpId="nav-chat">
          Chat
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
          <NavItem active={view === "teach"} onClick={() => onNavigate("teach")} helpId="nav-teach">
            Teach
          </NavItem>
        )}
        <NavItem active={view === "graph"} onClick={() => onNavigate("graph")} helpId="nav-graph">
          Map
        </NavItem>
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

      <div className="flex-1 overflow-y-auto px-2">
        {/* The global chat lists all conversations; an open project lists just its own (fed from
            App's project chat session), so a project's history sits here like the global chat's. */}
        {(view === "chat" || view === "project") && (
          <div data-help="conversations-list">
            <p className="px-2 pb-1 pt-2 font-mono text-xs uppercase tracking-wide text-faint">
              Conversations
            </p>
            {conversations.length === 0 && (
              <p className="px-2 py-2 text-xs text-faint">No conversations yet.</p>
            )}
            {conversations.map((c) => (
              // The row controls sit OUTSIDE the NavItem (itself a <button>) — a button can't nest a
              // button. They're hover-revealed siblings overlaid on the row's right edge; `pr-14` on
              // the NavItem keeps a long title from sliding under them.
              <div key={c.id} className="group relative">
                <NavItem
                  active={c.id === activeId}
                  onClick={() => onSelect(c.id)}
                  className="mb-1 pr-14"
                >
                  <span title={c.title}>{c.title}</span>
                </NavItem>
                <div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5">
                  <button
                    type="button"
                    onClick={() => setPendingMove(c)}
                    title="Move to a project"
                    aria-label={`Move conversation “${c.title}” to a project`}
                    className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 opacity-0 transition hover:bg-surface hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"
                  >
                    <span aria-hidden="true">📁</span>
                  </button>
                  <button
                    type="button"
                    onClick={() => setPendingDelete(c)}
                    title="Delete chat"
                    aria-label={`Delete conversation “${c.title}”`}
                    className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 opacity-0 transition hover:bg-surface hover:text-st-due focus-visible:opacity-100 group-hover:opacity-100"
                  >
                    <span aria-hidden="true">🗑</span>
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
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

      <div className="border-t border-border p-2">
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
          className="shrink-0 rounded-[var(--radius-sm)] bg-accent-soft px-1 font-mono text-[10px] text-accent-text"
          title={`${fallbacks} auto-switch fallback${fallbacks === 1 ? "" : "s"}`}
        >
          +{fallbacks}
        </span>
      )}
    </div>
  );
}

/** Drop the provider prefix for a compact label ("anthropic/claude-x" → "claude-x"). */
function shortModel(id: string): string {
  const slash = id.indexOf("/");
  return slash >= 0 ? id.slice(slash + 1) : id;
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
