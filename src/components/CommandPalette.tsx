// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { listConversations, listDocuments, listProjectOverviews } from "../lib/ipc";
import type { Conversation, Document, ProjectOverview } from "../lib/types";
import type { View } from "./Sidebar";
import { STATUS_LABEL } from "./ui";
import { useDepth, useTheme } from "../theme";

interface Props {
  onClose: () => void;
  /** Open a project's scoped view; `focusDocId` highlights a file within it. */
  onOpenProject: (project: string, focusDocId?: number) => void;
  onOpenConversation: (id: number) => void;
  onNavigate: (view: View) => void;
  onOpenSettings: () => void;
}

type ItemKind = "project" | "file" | "conversation" | "goto";

interface PaletteItem {
  id: string;
  kind: ItemKind;
  label: string;
  sublabel?: string;
  /** Lowercased haystack the query is matched against. */
  search: string;
  activate: () => void;
}

const KIND_HEADING: Record<ItemKind, string> = {
  project: "Projects",
  file: "Files",
  conversation: "Conversations",
  goto: "Go to",
};

const KIND_BADGE: Record<ItemKind, string> = {
  project: "Project",
  file: "File",
  conversation: "Chat",
  goto: "Go to",
};

/** Display/grouping order — entities first (spec §4), navigation last. */
const KIND_ORDER: ItemKind[] = ["project", "file", "conversation", "goto"];

const NAV_DESTS: { label: string; view: View }[] = [
  { label: "Focus", view: "focus" },
  { label: "Chats", view: "chat" },
  { label: "Calendar", view: "calendar" },
  { label: "Documents", view: "documents" },
  { label: "Review", view: "review" },
  { label: "Teach", view: "teach" },
  { label: "Map", view: "graph" },
  { label: "Pinboard", view: "pinboard" },
];

/**
 * Global quick-jump (spec §4): one keystroke opens it, type to filter to any
 * project, file, past conversation, or navigation destination, Enter jumps there.
 * Frontend-only — composes the existing list commands (mirrors GraphView's reuse).
 */
export function CommandPalette({
  onClose,
  onOpenProject,
  onOpenConversation,
  onNavigate,
  onOpenSettings,
}: Props) {
  const { showPower } = useDepth();
  // The Teach destination is only offered when the tab is visible (same Depth/Settings gate).
  const { teachVisible, mapVisible } = useTheme();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [projects, setProjects] = useState<ProjectOverview[]>([]);
  const [docs, setDocs] = useState<Document[]>([]);
  const [convs, setConvs] = useState<Conversation[]>([]);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  // Keep the latest navigation callbacks in a ref so the searchable index isn't
  // rebuilt on every parent re-render (App passes fresh closures each render, e.g.
  // on every streamed chat token while the palette is open).
  const cbRef = useRef({ onClose, onOpenProject, onOpenConversation, onNavigate, onOpenSettings });
  cbRef.current = { onClose, onOpenProject, onOpenConversation, onNavigate, onOpenSettings };

  // Fetch the three lists once on open — fast local reads. If any fails, the
  // "Go to" destinations below are still built, so navigation always works.
  useEffect(() => {
    inputRef.current?.focus();
    let cancelled = false;
    void (async () => {
      const [p, d, c] = await Promise.all([
        listProjectOverviews().catch(() => [] as ProjectOverview[]),
        listDocuments().catch(() => [] as Document[]),
        listConversations().catch(() => [] as Conversation[]),
      ]);
      if (cancelled) return;
      setProjects(p);
      setDocs(d);
      setConvs(c);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // The full searchable index. Rebuilt only when the source data or the
  // navigation callbacks change identity — never per keystroke.
  const items = useMemo<PaletteItem[]>(() => {
    const projectItems: PaletteItem[] = [...projects]
      .sort((a, b) => a.name.localeCompare(b.name))
      .map((pr) => ({
        id: `project:${pr.name}`,
        kind: "project",
        label: pr.name,
        sublabel: `${STATUS_LABEL[pr.status]} · ${pr.doc_count} doc${pr.doc_count === 1 ? "" : "s"}`,
        search: pr.name.toLowerCase(),
        activate: () => {
          cbRef.current.onOpenProject(pr.name);
          cbRef.current.onClose();
        },
      }));

    const fileItems: PaletteItem[] = docs.map((d) => ({
      id: `file:${d.id}`,
      kind: "file",
      label: d.title,
      sublabel: d.project,
      // Every project it belongs to and every tag it carries (#276), so the same `@tag` a chat
      // scopes by also finds files here.
      search:
        `${d.title} ${d.project} ${d.linked_projects.join(" ")} ${d.tags.join(" ")}`.toLowerCase(),
      activate: () => {
        cbRef.current.onOpenProject(d.project, d.id);
        cbRef.current.onClose();
      },
    }));

    const convItems: PaletteItem[] = convs.map((c) => ({
      id: `conv:${c.id}`,
      kind: "conversation",
      label: c.title,
      sublabel: c.project ? `scoped to ${c.project}` : undefined,
      search: `${c.title} ${c.project ?? ""}`.toLowerCase(),
      activate: () => {
        cbRef.current.onOpenConversation(c.id);
        cbRef.current.onClose();
      },
    }));

    const gotoItems: PaletteItem[] = [
      ...NAV_DESTS.filter(
        (dest) =>
          ((dest.view !== "teach" && dest.view !== "review") || teachVisible) &&
          (dest.view !== "graph" || mapVisible),
      ).map((dest) => ({
        id: `goto:${dest.view}`,
        kind: "goto" as const,
        label: dest.label,
        search: `${dest.label} go to`.toLowerCase(),
        activate: () => {
          cbRef.current.onNavigate(dest.view);
          cbRef.current.onClose();
        },
      })),
      {
        id: "goto:settings",
        kind: "goto",
        label: "Settings",
        search: "settings go to",
        activate: () => {
          cbRef.current.onOpenSettings();
          cbRef.current.onClose();
        },
      },
    ];

    return [...projectItems, ...fileItems, ...convItems, ...gotoItems];
  }, [projects, docs, convs, teachVisible, mapVisible]);

  // Filter + rank against the query, then regroup in display order. With a query,
  // each group is ordered by match score (best first); empty query keeps the
  // natural order so the palette doubles as a browse-everything list.
  const { groups, flat } = useMemo(() => {
    // The chat's pin syntax, typed here. There is no pinning in the palette — this is a text search
    // over titles, projects and tags, not a retrieval scope — but every form the chat TEACHES has to
    // come back with something, and two of them came back empty (#276):
    //
    //   `@"Atlas, Inc."` — what the chat's own @ menu inserts for a name with a space in it. The
    //   quotes belong to the syntax, not to the name, so left on they searched for a string no
    //   title contains.
    //   `@marketing @sales` — only the FIRST @ was stripped, so the second one had to appear
    //   literally in a title to match.
    //
    // Both now normalise to the words themselves. The sigil is dropped rather than honoured because
    // pinning a tag as a filter here is a feature, not a parse — it is carded separately.
    const q = query
      .trim()
      .replace(/@"([^"]*)"?/g, "$1")
      .replace(/@/g, "")
      .trim()
      .toLowerCase();
    const scored = items
      .map((item) => ({ item, score: q ? fuzzyScore(q, item.search) : 0 }))
      .filter((s): s is { item: PaletteItem; score: number } => s.score !== null);
    if (q) scored.sort((a, b) => b.score - a.score);

    const groups = KIND_ORDER.map((kind) => ({
      kind,
      items: scored.filter((s) => s.item.kind === kind).map((s) => s.item),
    })).filter((g) => g.items.length > 0);

    return { groups, flat: groups.flatMap((g) => g.items) };
  }, [items, query]);

  // Reset the highlight to the top whenever the result set changes — a new query,
  // or the async project/file/conversation lists landing — so Enter can't fire a
  // row that silently shifted under the cursor. Arrowing changes `active`, not
  // `flat`, so an explicit selection is preserved.
  useEffect(() => {
    setActive(0);
  }, [flat]);

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-palette-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((a) => Math.min(a + 1, flat.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((a) => Math.max(a - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      flat[active]?.activate();
    } else if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    }
  }

  let flatIndex = -1;

  return (
    <div
      className="absolute inset-0 z-50 flex justify-center pt-[12vh]"
      style={{ background: "var(--scrim)" }}
      onMouseDown={onClose}
    >
      <div
        className="flex h-fit max-h-[70vh] w-full max-w-xl flex-col overflow-hidden rounded-[var(--radius)] border border-border bg-surface shadow-2xl"
        data-help="command-palette"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          role="combobox"
          aria-label="Search projects, files and conversations"
          aria-expanded={flat.length > 0}
          aria-controls="pm-cmdk-listbox"
          aria-autocomplete="list"
          aria-activedescendant={flat.length > 0 ? `pm-cmdk-opt-${active}` : undefined}
          placeholder="Jump to a project, file, conversation…"
          className="w-full border-b border-border bg-transparent px-4 py-3 text-sm text-ink placeholder:text-ink4 focus:outline-none"
        />

        <div
          ref={listRef}
          role="listbox"
          id="pm-cmdk-listbox"
          aria-label="Results"
          className="flex-1 overflow-y-auto py-1"
        >
          {flat.length === 0 ? (
            <p className="px-4 py-6 text-center text-sm text-ink4">No matches.</p>
          ) : (
            groups.map((group) => (
              <div key={group.kind} role="group" aria-label={KIND_HEADING[group.kind]}>
                <p className="px-3 pb-1 pt-2 font-mono text-xs uppercase tracking-wide text-ink4">
                  {KIND_HEADING[group.kind]}
                </p>
                {group.items.map((item) => {
                  flatIndex++;
                  const idx = flatIndex;
                  return (
                    <button
                      key={item.id}
                      data-palette-index={idx}
                      id={`pm-cmdk-opt-${idx}`}
                      role="option"
                      aria-selected={idx === active}
                      // Arrow keys drive selection from the input via aria-activedescendant, so the
                      // options aren't separate tab stops; mouse click still activates.
                      tabIndex={-1}
                      onMouseMove={() => setActive(idx)}
                      onClick={item.activate}
                      className={`flex w-full items-center gap-3 px-3 py-2 text-left ${
                        idx === active ? "bg-accent-soft" : "hover:bg-surface"
                      }`}
                    >
                      <span className="w-12 shrink-0 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
                        {KIND_BADGE[item.kind]}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-sm text-ink" title={item.label}>
                        {item.label}
                      </span>
                      {item.sublabel && (
                        <span className="shrink-0 truncate text-xs text-ink3" title={item.sublabel}>
                          {item.sublabel}
                        </span>
                      )}
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        {showPower && (
          <div className="flex items-center gap-3 border-t border-border px-3 py-1.5 text-[0.6875rem] text-ink4">
            <span>↑↓ navigate</span>
            <span>↵ open</span>
            <span>esc close</span>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * Case-insensitive subsequence match with a light relevance score (higher is
 * better); `null` when the query's characters don't all appear in order. A
 * contiguous run, a substring hit, and an earlier first match all rank higher.
 * `query` is expected pre-lowercased.
 */
function fuzzyScore(query: string, haystack: string): number | null {
  let qi = 0;
  let score = 0;
  let firstIdx = -1;
  let prevIdx = -1;
  for (let hi = 0; hi < haystack.length && qi < query.length; hi++) {
    if (haystack[hi] === query[qi]) {
      if (firstIdx < 0) firstIdx = hi;
      if (prevIdx >= 0 && hi === prevIdx + 1) score += 2;
      prevIdx = hi;
      qi++;
    }
  }
  if (qi < query.length) return null;
  score += Math.max(0, 20 - firstIdx);
  if (haystack.includes(query)) score += 30;
  return score;
}
