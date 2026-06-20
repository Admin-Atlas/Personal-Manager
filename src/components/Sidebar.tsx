// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Conversation } from "../lib/types";

export type View = "focus" | "project" | "chat" | "documents" | "review" | "graph";

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
  return (
    <aside className="flex h-full w-64 flex-col border-r border-neutral-800 bg-neutral-950">
      <div className="flex items-center justify-between px-4 py-3">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold tracking-wide text-neutral-200">PM</span>
          <span
            title="PM is in alpha — under active development; expect rough edges and changes between updates."
            className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-amber-300"
          >
            Alpha
          </span>
        </div>
        {view === "chat" && (
          <button
            onClick={onNew}
            title="New conversation"
            className="rounded-md px-2 py-1 text-sm text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
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
          className="flex w-full items-center justify-between rounded-md border border-neutral-800 bg-neutral-900/60 px-3 py-1.5 text-left text-sm text-neutral-500 hover:bg-neutral-900 hover:text-neutral-300"
        >
          <span>Search…</span>
          <span className="text-xs text-neutral-600">{SHORTCUT_HINT}</span>
        </button>
      </div>

      <nav className="flex flex-col gap-1 px-2 pb-2">
        <NavItem
          label="Focus"
          active={view === "focus" || view === "project"}
          onClick={() => onNavigate("focus")}
          helpId="nav-focus"
        />
        <NavItem label="Chat" active={view === "chat"} onClick={() => onNavigate("chat")} helpId="nav-chat" />
        <NavItem
          label="Documents"
          active={view === "documents"}
          onClick={() => onNavigate("documents")}
          helpId="nav-documents"
        />
        <NavItem
          label="Review"
          active={view === "review"}
          badge={reviewCount}
          onClick={() => onNavigate("review")}
          helpId="nav-review"
        />
        <NavItem label="Map" active={view === "graph"} onClick={() => onNavigate("graph")} helpId="nav-graph" />
      </nav>

      <div className="flex-1 overflow-y-auto px-2">
        {view === "chat" && (
          <div data-help="conversations-list">
            <p className="px-2 pb-1 pt-2 text-xs uppercase tracking-wide text-neutral-600">
              Conversations
            </p>
            {conversations.length === 0 && (
              <p className="px-2 py-2 text-xs text-neutral-600">No conversations yet.</p>
            )}
            {conversations.map((c) => (
              <button
                key={c.id}
                onClick={() => onSelect(c.id)}
                className={`mb-1 w-full truncate rounded-md px-3 py-2 text-left text-sm ${
                  c.id === activeId
                    ? "bg-neutral-800 text-neutral-100"
                    : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
                }`}
                title={c.title}
              >
                {c.title}
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="border-t border-neutral-800 p-2">
        <button
          onClick={onOpenSettings}
          data-help="sidebar-models"
          title="Models in use — click to change"
          className="mb-1 w-full rounded-md px-3 py-1.5 text-left hover:bg-neutral-800"
        >
          <ModelRow role="Chat" id={chatModel} fallbacks={chatFallbacks} />
          <ModelRow role="Tasks" id={backgroundModel} fallbacks={backgroundFallbacks} />
        </button>
        <button
          onClick={onOpenWhatsNew}
          className="w-full rounded-md px-3 py-2 text-left text-sm text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
        >
          What's New
        </button>
        <button
          onClick={onOpenSettings}
          className="w-full rounded-md px-3 py-2 text-left text-sm text-neutral-400 hover:bg-neutral-800 hover:text-neutral-100"
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
      <span className="w-9 shrink-0 text-neutral-600">{role}</span>
      <span className="min-w-0 flex-1 truncate text-neutral-400" title={id ?? "Using the default model"}>
        {id ? shortModel(id) : "default"}
      </span>
      {fallbacks > 0 && (
        <span
          className="shrink-0 rounded bg-amber-500/15 px-1 text-[10px] text-amber-300"
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

function NavItem({
  label,
  active,
  badge,
  onClick,
  helpId,
}: {
  label: string;
  active: boolean;
  badge?: number;
  onClick: () => void;
  helpId?: string;
}) {
  return (
    <button
      onClick={onClick}
      data-help={helpId}
      className={`flex w-full items-center justify-between rounded-md px-3 py-2 text-left text-sm ${
        active
          ? "bg-neutral-800 text-neutral-100"
          : "text-neutral-400 hover:bg-neutral-900 hover:text-neutral-200"
      }`}
    >
      <span>{label}</span>
      {badge != null && badge > 0 && (
        <span className="ml-2 rounded-full bg-amber-500/20 px-2 py-0.5 text-xs font-medium text-amber-300">
          {badge}
        </span>
      )}
    </button>
  );
}
