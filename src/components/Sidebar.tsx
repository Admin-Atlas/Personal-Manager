// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Conversation } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { useDepth, useTheme } from "../theme";
import { NavItem } from "./ui";

export type View =
  | "focus"
  | "project"
  | "chat"
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
}

export function Sidebar({
  view,
  onNavigate,
  conversations,
  activeId,
  reviewCount,
  onSelect,
  onNew,
  onOpenSettings,
  onOpenWhatsNew,
  onOpenPalette,
  chatModel,
  backgroundModel,
  chatFallbacks,
  backgroundFallbacks,
}: Props) {
  const { showMeta } = useDepth();
  // The Teach tab is a Depth-keyed feature reveal (hidden for the minimalist preset), overridable
  // in Settings. Hiding it hides only the editor — deterministic alias resolution keeps running.
  const { teachVisible } = useTheme();
  // The Dev tab is an orthogonal capability reveal (issue #78) — independent of Depth, shown only
  // when the user turns Developer mode on in Settings.
  const { devMode } = useDevMode();
  return (
    <aside className="flex h-full w-64 flex-col border-r border-border bg-panel">
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
        {view === "chat" && (
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
        {view === "chat" && (
          <div data-help="conversations-list">
            <p className="px-2 pb-1 pt-2 font-mono text-xs uppercase tracking-wide text-faint">
              Conversations
            </p>
            {conversations.length === 0 && (
              <p className="px-2 py-2 text-xs text-faint">No conversations yet.</p>
            )}
            {conversations.map((c) => (
              <NavItem
                key={c.id}
                active={c.id === activeId}
                onClick={() => onSelect(c.id)}
                className="mb-1"
              >
                <span title={c.title}>{c.title}</span>
              </NavItem>
            ))}
          </div>
        )}
      </div>

      <div className="border-t border-border p-2">
        <button
          onClick={onOpenSettings}
          data-help="sidebar-models"
          title="Models in use — click to change"
          className="mb-1 w-full rounded-[var(--radius-sm)] px-3 py-1.5 text-left hover:bg-surface"
        >
          {showMeta && <ModelRow role="Chat" id={chatModel} fallbacks={chatFallbacks} />}
          {showMeta && (
            <ModelRow role="Tasks" id={backgroundModel} fallbacks={backgroundFallbacks} />
          )}
        </button>
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

/** The pill count shown on a nav row (e.g. pending reviews); renders nothing at zero. */
function CountBadge({ count }: { count?: number }) {
  if (count == null || count <= 0) return null;
  return (
    <span className="ml-2 rounded-[var(--radius-sm)] bg-accent-soft px-2 py-0.5 font-mono text-xs font-medium text-accent-text">
      {count}
    </span>
  );
}
