// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import {
  listConversations,
  listDocuments,
  listProjectOverviews,
  searchCalendarEvents,
} from "../lib/ipc";
import { readHidden } from "../lib/calendarPrefs";
import { formatEventWhen } from "../lib/format";
import type { CalendarEvent, Conversation, Document, ProjectOverview } from "../lib/types";
import type { View } from "./Sidebar";
import { Modal, STATUS_LABEL } from "./ui";
import { useDepth, useTheme } from "../theme";

interface Props {
  onClose: () => void;
  /** Open a project's scoped view; `focusDocId` highlights a file within it. */
  onOpenProject: (project: string, focusDocId?: number) => void;
  onOpenConversation: (id: number) => void;
  /** Show a calendar event: the Calendar tab, moved to its day, with its details open. */
  onOpenEvent: (event: CalendarEvent) => void;
  onNavigate: (view: View) => void;
  onOpenSettings: () => void;
}

type ItemKind = "project" | "file" | "conversation" | "event" | "goto";

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
  event: "Events",
  goto: "Go to",
};

const KIND_BADGE: Record<ItemKind, string> = {
  project: "Project",
  file: "File",
  conversation: "Chat",
  event: "Event",
  goto: "Go to",
};

/** Display/grouping order — entities first (spec §4), navigation last. */
const KIND_ORDER: ItemKind[] = ["project", "file", "conversation", "event", "goto"];

/** The shortest query that searches the calendar: one letter would match most of it. */
const EVENT_QUERY_MIN = 2;
/** How long typing must pause before the calendar is searched (a query per keystroke otherwise). */
const EVENT_SEARCH_DELAY_MS = 150;

/**
 * The query as the palette matches it. The chat's pin syntax, typed here: there is no pinning in
 * the palette — this is a text search over titles, projects and tags, not a retrieval scope — but
 * every form the chat TEACHES has to come back with something, and two of them came back empty
 * (#276):
 *
 *   `@"Atlas, Inc."` — what the chat's own @ menu inserts for a name with a space in it. The
 *   quotes belong to the syntax, not to the name, so left on they searched for a string no
 *   title contains.
 *   `@marketing @sales` — only the FIRST @ was stripped, so the second one had to appear
 *   literally in a title to match.
 *
 * Both now normalise to the words themselves. The sigil is dropped rather than honoured because
 * pinning a tag as a filter here is a feature, not a parse — it is carded separately.
 */
function normalizeQuery(query: string): string {
  return query
    .trim()
    .replace(/@"([^"]*)"?/g, "$1")
    .replace(/@/g, "")
    .trim()
    .toLowerCase();
}

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
 * project, file, past conversation, calendar event, or navigation destination, Enter jumps there.
 * Composes the existing list commands (mirrors GraphView's reuse); the calendar is searched in Rust
 * as you type, since PM keeps every event (#1047) and they don't all belong in memory.
 */
export function CommandPalette({
  onClose,
  onOpenProject,
  onOpenConversation,
  onOpenEvent,
  onNavigate,
  onOpenSettings,
}: Props) {
  const { showPower } = useDepth();
  // The Teach destination is only offered when the tab is visible (same Depth/Settings gate).
  const { teachVisible, mapVisible } = useTheme();
  const [query, setQuery] = useState("");
  // The highlighted row, by its id: rows that land later (the lists, the calendar's hits) never
  // move the highlight off what was arrowed to, so Enter opens what's highlighted. `null` (a new
  // query, or nothing arrowed yet) is the top row.
  const [activeId, setActiveId] = useState<string | null>(null);
  const [projects, setProjects] = useState<ProjectOverview[]>([]);
  const [docs, setDocs] = useState<Document[]>([]);
  const [convs, setConvs] = useState<Conversation[]>([]);
  // The calendar's hits, and the (normalised) query they answer: a hit list for an older query is
  // never shown under a newer one.
  const [eventHits, setEventHits] = useState<{ q: string; events: CalendarEvent[] }>({
    q: "",
    events: [],
  });
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  // Keep the latest navigation callbacks in a ref so the searchable index isn't
  // rebuilt on every parent re-render (App passes fresh closures each render, e.g.
  // on every streamed chat token while the palette is open).
  const cbRef = useRef({
    onClose,
    onOpenProject,
    onOpenConversation,
    onOpenEvent,
    onNavigate,
    onOpenSettings,
  });
  cbRef.current = {
    onClose,
    onOpenProject,
    onOpenConversation,
    onOpenEvent,
    onNavigate,
    onOpenSettings,
  };
  const q = normalizeQuery(query);

  // Search the calendar once typing pauses. Only the latest query's answer lands (an older one is
  // cancelled), and a failed search just shows no events: the rest of the palette still works. A
  // calendar the Calendar tab hides has nothing to jump to, so it's left out, in Rust, before the
  // limit.
  useEffect(() => {
    if (q.length < EVENT_QUERY_MIN) return;
    let cancelled = false;
    const timer = window.setTimeout(() => {
      void searchCalendarEvents(q, [...readHidden()])
        .catch(() => [] as CalendarEvent[])
        .then((events) => {
          if (!cancelled) setEventHits({ q, events });
        });
    }, EVENT_SEARCH_DELAY_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [q]);

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

  // The calendar's hits for this query, in the order Rust ranked them (upcoming soonest first, then
  // past latest first): already matched word by word, so they skip the fuzzy scorer. Only with a
  // query: an empty palette browses everything else, but not every event ever kept. While this
  // query's search is on its way, the last answer's hits that still hold every word stay, so the
  // group doesn't blink out and back under the rows being read.
  const eventItems = useMemo<PaletteItem[]>(() => {
    if (q.length < EVENT_QUERY_MIN) return [];
    const words = q.split(/\s+/).filter(Boolean);
    const events =
      eventHits.q === q
        ? eventHits.events
        : eventHits.events.filter((ev) => {
            const text = `${ev.summary} ${ev.location ?? ""}`.toLowerCase();
            return words.every((w) => text.includes(w));
          });
    return events.map((ev) => ({
      id: `event:${ev.calendar_id}:${ev.id}`,
      kind: "event",
      label: ev.summary || "(No title)",
      sublabel: `${formatEventWhen(ev.start, ev.all_day)}${ev.recurring ? " · repeats" : ""}`,
      search: "",
      activate: () => {
        cbRef.current.onOpenEvent(ev);
        cbRef.current.onClose();
      },
    }));
  }, [eventHits, q]);

  // Filter + rank against the query, then regroup in display order. With a query,
  // each group is ordered by match score (best first); empty query keeps the
  // natural order so the palette doubles as a browse-everything list.
  const { groups, flat } = useMemo(() => {
    const scored = items
      .map((item) => ({ item, score: q ? fuzzyScore(q, item.search) : 0 }))
      .filter((s): s is { item: PaletteItem; score: number } => s.score !== null);
    if (q) scored.sort((a, b) => b.score - a.score);

    const groups = KIND_ORDER.map((kind) => ({
      kind,
      items:
        kind === "event"
          ? eventItems
          : scored.filter((s) => s.item.kind === kind).map((s) => s.item),
    })).filter((g) => g.items.length > 0);

    return { groups, flat: groups.flatMap((g) => g.items) };
  }, [items, eventItems, q]);

  // The highlight follows the row it's on, not a position: when the lists or the calendar's hits land
  // and rows move, the arrowed row stays highlighted (an index used to snap back to the top, under a
  // row the user had picked), so Enter can't fire a row that shifted under the cursor. Nothing
  // arrowed yet, or the row gone, is the top row.
  const found = activeId === null ? -1 : flat.findIndex((item) => item.id === activeId);
  const active = found < 0 ? 0 : found;

  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-palette-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveId(flat[Math.min(active + 1, flat.length - 1)]?.id ?? null);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveId(flat[Math.max(active - 1, 0)]?.id ?? null);
    } else if (e.key === "Enter") {
      e.preventDefault();
      flat[active]?.activate();
    }
    // Escape is deliberately NOT here. It used to be, bound to this input alone — so one Tab out of
    // the input (the options are `tabIndex={-1}`, so Tab leaves the palette entirely) made Escape
    // dead, because a keydown on a background control never bubbles back to it. `Modal` listens on
    // `window`, which works from any focus position.
  }

  let flatIndex = -1;

  return (
    // The dialog SHELL is `Modal` — scrim, `role="dialog"`, `aria-modal`, Escape from any focus
    // position, the focus trap and focus restore. This used to be a hand-rolled overlay that had
    // none of those except a half-Escape; `placement="top"` is the one thing a palette needs that a
    // centred dialog doesn't, and it exists so this could stop being a copy.
    <Modal
      open
      onClose={onClose}
      placement="top"
      widthClassName="max-w-xl"
      heightClassName="h-fit max-h-[70vh]"
      overflowClassName="overflow-hidden"
      className="flex flex-col"
      // The help anchor rides the dialog element itself — help mode outlines the hovered
      // `[data-help]`, and a wrapper inside the panel would either paint no outline (no box) or
      // have to re-derive the `h-fit` + `flex-1` sizing this dialog already owns.
      helpId="command-palette"
      // The name is on the dialog, not the input: `aria-label` on a combobox names the FIELD.
      label="Command palette"
    >
      <input
        ref={inputRef}
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setActiveId(null);
        }}
        onKeyDown={onKeyDown}
        role="combobox"
        aria-label="Search projects, files, conversations and events"
        aria-expanded={flat.length > 0}
        aria-controls="pm-cmdk-listbox"
        aria-autocomplete="list"
        aria-activedescendant={flat.length > 0 ? `pm-cmdk-opt-${active}` : undefined}
        placeholder="Jump to a project, file, conversation, event…"
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
                    onMouseMove={() => setActiveId(item.id)}
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
    </Modal>
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
