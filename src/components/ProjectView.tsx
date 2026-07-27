// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { ChatView } from "./ChatView";
import { Composer } from "./Composer";
import { ContextMeter } from "./ContextMeter";
import { ProviderChip } from "./ProviderChip";
import { FallbackStrip } from "./FallbackStrip";
import { RetrievalExplainPanel } from "./RetrievalExplainPanel";
import { listDocuments, listMilestones, listProjects, setDocumentMetadata } from "../lib/ipc";
import { useResizable } from "../lib/useResizable";
import { useSidebarSplit } from "../lib/useSidebarSplit";
import { idleSince } from "../lib/chatSession";
import type { ProjectChat } from "../lib/useProjectChat";
import type { Document, LocalLlmStatus, Milestone } from "../lib/types";
import { Button } from "./ui";
import { ImportancePicker } from "./ImportancePicker";
import { LinkedBadge, ProjectPicker, projectsOf } from "./ProjectPicker";
import { MilestoneList } from "./MilestoneList";
import { TagEditor } from "./TagEditor";
import { ChatBadge } from "./ChatBadge";
import { DeleteDocumentButton, DeleteDocumentDialog } from "./DeleteDocumentDialog";
import { CollapseTab } from "./CollapseTab";
import { rankImportance } from "../lib/importance";
import { useReader } from "../lib/reader";
import {
  readMilestoneSort,
  readShowCompletedMilestones,
  writeMilestoneSort,
  writeShowCompletedMilestones,
  type MsSort,
  type MsSortKey,
} from "../lib/milestonePrefs";
import { useDepth, useTheme } from "../theme";

const PROJECT_LIST_ID = "focus-projects";

type FileSortKey = "name" | "importance";
interface FileSort {
  key: FileSortKey;
  dir: "asc" | "desc";
}

interface Props {
  project: string;
  /** The project's scoped chat session (owned by App so the left sidebar can list it too). */
  chat: ProjectChat;
  /** Live local-endpoint status (#297), for the chat's fallback strip / provider chip / provenance
   *  footer. Passed from App (the single subscription); `null`/unconfigured renders nothing. */
  localAi: LocalLlmStatus | null;
  /** A file to scroll to and briefly highlight (set by the command palette). */
  focusDocId?: number | null;
  /** Open a past chat a citation points to, at its cited turn — routes up to App's global chat view
   *  (the cited chat may not be this project's) (board card 7E PR3). */
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
  /** Switch chat to a larger-context model (the meter's Upgrade action). The same App handler the
   *  global chat uses — chat models are a single global setting, so both meters upgrade the same way. */
  onUpgrade: (modelId: string) => void | Promise<void>;
  onBack: () => void;
}

/** Per-project scoped view (spec §4): the project's files alongside a chat whose retrieval is
 *  confined to just this project — "everything narrows to just it". The scoped chat's session lives
 *  in App (so the left sidebar lists this project's conversations, like the global chat); this view
 *  renders the active thread plus the project's milestones and documents. */
export function ProjectView({
  project,
  chat,
  localAi,
  focusDocId,
  onOpenChatCitation,
  onUpgrade,
  onBack,
}: Props) {
  const [documents, setDocuments] = useState<Document[]>([]);
  const [milestones, setMilestones] = useState<Milestone[]>([]);
  /** The file the palette jumped to — flashed briefly, then cleared. */
  const [flashId, setFlashId] = useState<number | null>(null);
  const filesRef = useRef<HTMLUListElement>(null);
  const { atLeast, showMeta, showPower } = useDepth();
  // The focus-panel triage controls are the same learning tool as the Review/Teach tabs, so they
  // ride the same Settings switch: when the user trusts the AI's filing and hides those tabs, these
  // hide too (the panel falls back to the read-only display below).
  const { teachVisible } = useTheme();
  // Existing project names for the power-depth project datalist (re-file autocomplete).
  const [projectNames, setProjectNames] = useState<string[]>([]);
  // Clicking a file opens the shared document reader onto it (mounted at app scope), so a user can
  // read a project's document without hunting for it in the Documents tab.
  const { openReader, current: readerDoc } = useReader();
  // How the Files panel is ordered. Name A→Z by default; clicking a key again reverses it.
  const [sort, setSort] = useState<FileSort>({ key: "name", dir: "asc" });
  // The document whose delete is being confirmed, or null.
  const [deleting, setDeleting] = useState<Document | null>(null);
  // How the Milestones panel is ordered — deadline (soonest first) by default, remembered per device
  // FOR THIS PROJECT. Display-only: the backend sort_order is untouched (governing() reads it).
  //
  // App renders <ProjectView> without a `key`, so switching from project A to project B does NOT
  // remount this component — a lazy useState initialiser would never re-run and B would inherit A's
  // sort. Hence the explicit re-read on `project`. And the write lives in the toggle rather than in a
  // `[msSort]` effect: an effect would fire AFTER the project changed and stamp the previous
  // project's sort under the new project's name.
  const [msSort, setMsSort] = useState<MsSort>(() => readMilestoneSort(project));
  useEffect(() => setMsSort(readMilestoneSort(project)), [project]);
  // Whether completed ("met") milestones are shown; default true — the scroll-to-next below tucks
  // them above the fold rather than hiding history. Ignored under Manual sort, where the ↑/↓ reorder
  // needs every row.
  const [showCompleted, setShowCompleted] = useState(readShowCompletedMilestones);
  useEffect(() => writeShowCompletedMilestones(showCompleted), [showCompleted]);
  // The right panel's width is a fraction of the window (drag the left edge to resize, stays
  // proportional on window resize), clamped so it can't get so narrow the content scrolls.
  const {
    width: asideWidth,
    startResize,
    resizing,
    collapsed: asideCollapsed,
    expand: expandAside,
  } = useResizable({
    storageKey: "pm.project.sidebarFrac",
    defaultFrac: 0.24,
    minFrac: 0.16,
    maxFrac: 0.5,
    edge: "left",
    collapsible: true,
  });
  // The sidebar's Milestones (top) / Files (bottom) split. The ratio is a hard pref across the app
  // (card 7E) — see useSidebarSplit. Only in play when the Milestones panel is shown.
  const {
    topFrac,
    containerRef: splitRef,
    startResize: startSplit,
    resizing: splitting,
  } = useSidebarSplit();

  function toggleSort(key: FileSortKey) {
    setSort((cur) =>
      cur.key === key
        ? { key, dir: cur.dir === "asc" ? "desc" : "asc" }
        : { key, dir: key === "importance" ? "desc" : "asc" },
    );
  }

  function toggleMsSort(key: MsSortKey) {
    // Computed from `msSort` rather than via a functional updater, so the persist is a plain
    // side-effect that can't double-fire under StrictMode.
    const next: MsSort =
      msSort.key === key
        ? { key, dir: msSort.dir === "asc" ? "desc" : "asc" }
        : { key, dir: "asc" };
    setMsSort(next);
    writeMilestoneSort(project, next);
  }

  const sortedDocs = useMemo(() => {
    const factor = sort.dir === "asc" ? 1 : -1;
    return [...documents].sort((a, b) => {
      const c =
        sort.key === "importance"
          ? rankImportance(a.importance) - rankImportance(b.importance) ||
            a.title.localeCompare(b.title)
          : a.title.localeCompare(b.title);
      return c * factor;
    });
  }, [documents, sort]);

  // Milestones sorted for display only — the backend order (sort_order, id) is left untouched, so
  // governing()/status derivation is unaffected. Manual returns the array unchanged (already backend
  // order, so the ↑/↓ index math stays correct). Undated milestones sink to the bottom either way.
  const sortedMilestones = useMemo(() => {
    if (msSort.key === "manual") return milestones;
    const factor = msSort.dir === "asc" ? 1 : -1;
    return [...milestones].sort((a, b) => {
      if (msSort.key === "deadline") {
        const da = a.due_date?.slice(0, 10) ?? "";
        const db = b.due_date?.slice(0, 10) ?? "";
        if (da && db) {
          const c = da.localeCompare(db);
          if (c) return c * factor;
        } else if (!da && db) {
          return 1; // undated sinks, regardless of direction
        } else if (da && !db) {
          return -1;
        }
      } else {
        const c = a.label.localeCompare(b.label);
        if (c) return c * factor;
      }
      return a.sort_order - b.sort_order || a.id - b.id;
    });
  }, [milestones, msSort]);

  // Whether there's any completed milestone to hide — gates the "Completed" checkbox.
  const hasMetMilestones = useMemo(() => milestones.some((m) => m.state === "met"), [milestones]);
  // The list actually shown: completed ones dropped when hidden — except under Manual sort, where the
  // ↑/↓ reorder maps array indices to sort_order and must see every row.
  const displayMilestones = useMemo(
    () =>
      showCompleted || msSort.key === "manual"
        ? sortedMilestones
        : sortedMilestones.filter((m) => m.state !== "met"),
    [sortedMilestones, showCompleted, msSort.key],
  );

  const refreshMilestones = () => {
    listMilestones(project)
      .then(setMilestones)
      .catch(() => {});
  };

  // Reload just this project's documents — shared by the project-change effect and the delete
  // dialog, so a deleted file leaves the list immediately rather than lingering until a tab switch.
  // Membership, not just home (#275): a document linked into this project belongs in its file list
  // and in its chat's grounding, and this one predicate is what decides both for the panel.
  const belongsHere = (d: Document) => d.project === project || d.linked_projects.includes(project);

  const refreshDocuments = () => {
    listDocuments()
      .then((all) => setDocuments(all.filter(belongsHere)))
      .catch((e) => chat.setError(String(e)));
  };

  useEffect(() => {
    // Load this project's documents and milestones. The chat session (App-owned) re-inits itself on
    // the same project change.
    refreshDocuments();
    refreshMilestones();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [project]);

  // When the palette opens this project to land on a file, scroll it into view
  // and flash it. Keyed on the docs too, so it fires once the list has rendered.
  useEffect(() => {
    if (focusDocId == null) return;
    setFlashId(focusDocId);
    filesRef.current
      ?.querySelector(`[data-doc-id="${focusDocId}"]`)
      ?.scrollIntoView({ block: "center" });
    const clear = setTimeout(() => setFlashId(null), 2500);
    return () => clearTimeout(clear);
  }, [focusDocId, documents]);

  // Project names for the power-depth re-file datalist; only needed while the triage controls show.
  useEffect(() => {
    if (!teachVisible || !showPower) return;
    listProjects()
      .then(setProjectNames)
      .catch(() => {});
  }, [teachVisible, showPower]);

  /** Persist a metadata change for one document (importance / tags / project) and reflect it
   *  locally. A metadata edit only rewrites front-matter — no re-embed. If a power-user re-files a
   *  document to a different project, it leaves this project's panel. */
  async function saveMeta(
    doc: Document,
    patch: Partial<Pick<Document, "project" | "linked_projects" | "tags" | "importance">>,
  ) {
    // The memberships are replaced wholesale by the backend, so every edit — even one that only
    // touches the importance — must pass the current list back or it would unlink the document.
    const nextProjects = patch.linked_projects
      ? [patch.project ?? doc.project, ...patch.linked_projects]
      : patch.project
        ? [patch.project, ...doc.linked_projects.filter((p) => p !== patch.project)]
        : projectsOf(doc);
    const [nextProject, ...nextAlso] = nextProjects;
    const nextTags = patch.tags ?? doc.tags;
    const nextImportance = patch.importance !== undefined ? patch.importance : doc.importance;
    try {
      const updated = await setDocumentMetadata(
        doc.id,
        nextProject,
        nextAlso,
        nextTags,
        nextImportance,
      );
      // Leaves the panel only when it no longer belongs here AT ALL. Testing `project !== project`
      // would drop every linked document on any edit, since its home is elsewhere by definition.
      setDocuments((docs) =>
        !belongsHere(updated)
          ? docs.filter((d) => d.id !== updated.id)
          : docs.map((d) => (d.id === updated.id ? updated : d)),
      );
    } catch (e) {
      chat.setError(String(e));
    }
  }

  // Reopening a chat idle > 24h offers a clean start (card 7E) — purely UX framing; the turns were
  // already indexed under card B. Only for a loaded, settled thread the user hasn't dismissed.
  const idleDate =
    atLeast("standard") &&
    chat.convId != null &&
    chat.streaming === null &&
    chat.dismissedIdleFor !== chat.convId
      ? idleSince(chat.messages, Date.now())
      : null;

  // Split the sidebar 50-50 (draggable) only when there are actual milestones to balance against
  // Files. With none, Files fills the space — the add-a-milestone control (Depth-gated) just sits
  // above it at its natural height, no divider. So an empty project doesn't waste half the sidebar.
  const hasMilestones = milestones.length > 0;
  const showAddMilestone = showMeta && !hasMilestones;

  const filesPanel = (
    <>
      <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3">
        <span className="font-mono text-xs uppercase tracking-wide text-ink4">Files</span>
        {documents.length > 1 && (
          <div className="flex items-center gap-2 text-[0.625rem] text-ink4">
            <SortToggle label="Name" sortKey="name" sort={sort} onSort={toggleSort} />
            <SortToggle label="Importance" sortKey="importance" sort={sort} onSort={toggleSort} />
          </div>
        )}
      </div>
      {teachVisible && showPower && (
        <datalist id={PROJECT_LIST_ID}>
          {projectNames.map((name) => (
            <option key={name} value={name} />
          ))}
        </datalist>
      )}
      {documents.length === 0 ? (
        <p className="px-4 py-2 text-xs text-ink4">No documents in this project.</p>
      ) : (
        <ul ref={filesRef} className="flex flex-col gap-0.5 px-2 pb-4">
          {sortedDocs.map((d) => (
            <li
              key={d.id}
              data-doc-id={d.id}
              onClick={() => openReader(d)}
              // Keyboard access without role="button" — the item can contain its own controls (the
              // triage inputs below), so open on Enter/Space only when the item itself has focus.
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                  e.preventDefault();
                  openReader(d);
                }
              }}
              className={`group cursor-pointer rounded-[var(--radius-sm)] px-2 py-1.5 transition-colors hover:bg-surface ${
                flashId === d.id
                  ? "bg-surface ring-1 ring-[color-mix(in_oklab,var(--accent)_50%,transparent)]"
                  : readerDoc?.id === d.id
                    ? "bg-accent-soft"
                    : ""
              }`}
            >
              <div className="flex min-w-0 items-center gap-1.5">
                {d.source_type === "chat" && <ChatBadge compact />}
                <span
                  className="min-w-0 flex-1 truncate font-head text-sm text-ink2"
                  title={d.title}
                >
                  {d.title}
                </span>
                {/* Only when this project is NOT the document's home: otherwise every row in a
                    single-project store would carry a badge that told the user nothing. */}
                {d.project !== project && <LinkedBadge home={d.project} />}
                <DeleteDocumentButton onClick={() => setDeleting(d)} />
              </div>
              {teachVisible ? (
                // Manual triage, same controls as the Review tab. Everyone gets the importance
                // toggle; power depth also gets project (re-file) + tags, like a Review row.
                // Stop clicks here from bubbling to the row's open-reader handler.
                <div className="mt-1.5 flex flex-col gap-1.5" onClick={(e) => e.stopPropagation()}>
                  {showPower && (
                    <div className="flex items-start gap-1.5 text-xs text-ink4">
                      <span className="shrink-0 pt-1">Projects</span>
                      <ProjectPicker
                        // Inside this project, a "Primary <this project>" pill on every row just
                        // restates the heading. What's left is what you don't already know: the
                        // OTHER projects a file belongs to, and the input to add one.
                        hidePrimary={project}
                        value={projectsOf(d)}
                        onChange={(projects) => {
                          const [home, ...also] = projects;
                          void saveMeta(d, { project: home, linked_projects: also });
                        }}
                        suggestions={projectNames}
                        listId={PROJECT_LIST_ID}
                      />
                    </div>
                  )}
                  <ImportancePicker
                    value={d.importance}
                    onChange={(importance) => void saveMeta(d, { importance })}
                  />
                  {showPower && (
                    <TagEditor tags={d.tags} onChange={(tags) => void saveMeta(d, { tags })} />
                  )}
                </div>
              ) : (
                showMeta && (
                  <div className="flex gap-2 font-mono text-xs text-ink4">
                    {d.importance && <span className="capitalize">{d.importance}</span>}
                    <span>
                      {d.chunk_count} chunk{d.chunk_count === 1 ? "" : "s"}
                    </span>
                  </div>
                )
              )}
            </li>
          ))}
        </ul>
      )}
      {deleting && (
        <DeleteDocumentDialog
          doc={deleting}
          onClose={() => setDeleting(null)}
          onDeleted={refreshDocuments}
        />
      )}
    </>
  );

  const milestonesPanel = (
    <div className="px-4 pb-3 pt-3" data-help="project-milestones">
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-xs uppercase tracking-wide text-ink4">Milestones</span>
        {showMeta && milestones.length > 1 && (
          <div className="flex items-center gap-2 text-[0.625rem] text-ink4">
            {msSort.key !== "manual" && hasMetMilestones && (
              <label
                className="flex cursor-pointer items-center gap-1"
                title="Show completed milestones"
              >
                <input
                  type="checkbox"
                  checked={showCompleted}
                  onChange={(e) => setShowCompleted(e.target.checked)}
                  className="accent-[var(--accent)]"
                />
                <span>Completed</span>
              </label>
            )}
            <SortToggle
              label="Manual"
              sortKey="manual"
              sort={msSort}
              directional={false}
              onSort={toggleMsSort}
            />
            <SortToggle label="Deadline" sortKey="deadline" sort={msSort} onSort={toggleMsSort} />
            <SortToggle label="Name" sortKey="label" sort={msSort} onSort={toggleMsSort} />
          </div>
        )}
      </div>
      <div className="mt-2">
        <MilestoneList
          project={project}
          milestones={displayMilestones}
          onChanged={refreshMilestones}
          readOnly={!showMeta}
          manualOrder={msSort.key === "manual"}
        />
      </div>
    </div>
  );

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-border bg-panel px-6 py-3">
        <Button variant="tertiary" onClick={onBack} title="Back to Focus">
          ← Focus
        </Button>
        <div className="min-w-0">
          <h1 className="truncate font-head text-sm font-semibold text-ink">{project}</h1>
          <p className="font-mono text-xs text-ink4">
            {documents.length} document{documents.length === 1 ? "" : "s"} · chat scoped to this
            project
          </p>
        </div>
      </header>

      <div className={`flex flex-1 overflow-hidden ${resizing ? "select-none" : ""}`}>
        <main className="flex min-w-0 flex-1 flex-col" data-help="project-chat">
          {chat.error && (
            <div
              className="border-b px-4 py-2 text-sm"
              style={{
                color: "var(--st-due)",
                borderColor: "color-mix(in oklab, var(--st-due) 40%, transparent)",
                background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
              }}
            >
              {chat.error}
            </div>
          )}
          {chat.fallback && (
            <FallbackStrip fallback={chat.fallback} onDismiss={chat.dismissFallback} />
          )}
          <ChatView
            messages={chat.messages}
            prompts={chat.prompts}
            confidences={chat.confidences}
            providers={chat.providers}
            showProvenance={!!localAi?.configured}
            streaming={chat.streaming}
            onOpenChatCitation={onOpenChatCitation}
          />
          {idleDate && (
            <div
              className="flex items-center justify-between gap-3 border-t border-border px-4 py-2 text-xs text-ink3"
              data-help="chat-idle-prompt"
            >
              <span>This conversation has been idle since {idleDate}. Start a new one?</span>
              <div className="flex shrink-0 items-center gap-3">
                <Button variant="secondary" onClick={chat.newChat} className="px-2 py-1 text-xs">
                  New chat
                </Button>
                <button
                  type="button"
                  onClick={() => chat.setDismissedIdleFor(chat.convId)}
                  title="Dismiss"
                  className="text-ink4 hover:text-ink2"
                >
                  Dismiss
                </button>
              </div>
            </div>
          )}
          <Composer
            disabled={chat.sending}
            onSend={chat.handleSend}
            leftTools={
              <div className="flex items-center gap-2">
                <ContextMeter
                  conversationId={chat.convId}
                  refreshKey={chat.messages.length}
                  onUpgrade={onUpgrade}
                />
                <ProviderChip status={localAi} />
              </div>
            }
            rightTools={<RetrievalExplainPanel messages={chat.messages} project={project} />}
          />
        </main>

        {asideCollapsed ? (
          <CollapseTab side="right" onExpand={expandAside} />
        ) : (
          <aside
            style={{ width: asideWidth }}
            className="relative flex shrink-0 flex-col overflow-hidden border-l border-border bg-panel"
            data-help="project-sidebar"
          >
            <div
              onPointerDown={startResize}
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize panel"
              title="Drag to resize · drag to the edge to hide"
              data-help="project-resize"
              className={`absolute left-0 top-0 z-10 h-full w-1.5 cursor-col-resize touch-none transition-colors hover:bg-[color-mix(in_oklab,var(--accent)_45%,transparent)] ${
                resizing ? "bg-[color-mix(in_oklab,var(--accent)_60%,transparent)]" : ""
              }`}
            />
            {hasMilestones ? (
              // Milestones (top) + Files (bottom), split by a draggable divider (hard-pref ratio).
              <div ref={splitRef} className="flex min-h-0 flex-1 flex-col">
                <div
                  style={{ flexBasis: `${topFrac * 100}%` }}
                  className="min-h-0 shrink-0 grow-0 overflow-y-auto overflow-x-hidden"
                  data-help="project-milestones-panel"
                >
                  {milestonesPanel}
                </div>
                <div
                  onPointerDown={startSplit}
                  role="separator"
                  aria-orientation="horizontal"
                  aria-label="Resize milestones and files"
                  title="Drag to resize"
                  data-help="project-split"
                  className={`h-1.5 shrink-0 cursor-row-resize touch-none border-y border-border transition-colors hover:bg-[color-mix(in_oklab,var(--accent)_45%,transparent)] ${
                    splitting ? "bg-[color-mix(in_oklab,var(--accent)_60%,transparent)]" : ""
                  }`}
                />
                <div className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden">{filesPanel}</div>
              </div>
            ) : (
              // No milestones yet: the (Depth-gated) add control sits at its natural height, Files
              // fills the rest of the sidebar.
              <div className="flex min-h-0 flex-1 flex-col">
                {showAddMilestone && (
                  <div className="shrink-0 border-b border-border">{milestonesPanel}</div>
                )}
                <div
                  className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
                  data-help="project-files"
                >
                  {filesPanel}
                </div>
              </div>
            )}
          </aside>
        )}
      </div>
    </div>
  );
}

/** A compact sort toggle for a panel header: click to sort by this key, click again to reverse.
 *  Shows the active direction (▲/▼) or an idle hint (↕). A non-directional key (e.g. "Manual")
 *  shows no glyph — it just selects that order. Shared by the Files and Milestones panels. */
function SortToggle<K extends string>({
  label,
  sortKey,
  sort,
  directional = true,
  onSort,
}: {
  label: string;
  sortKey: K;
  sort: { key: K; dir: "asc" | "desc" };
  directional?: boolean;
  onSort: (key: K) => void;
}) {
  const active = sort.key === sortKey;
  return (
    <button
      type="button"
      onClick={() => onSort(sortKey)}
      aria-pressed={active}
      title={
        directional
          ? `Sort by ${label.toLowerCase()} (${active && sort.dir === "desc" ? "descending" : "ascending"})`
          : `Sort ${label.toLowerCase()}`
      }
      className={`inline-flex items-center gap-0.5 hover:text-ink2 ${active ? "text-ink2" : ""}`}
    >
      {label}
      {directional && (
        <span aria-hidden className="text-[0.5rem] leading-none">
          {active ? (sort.dir === "asc" ? "▲" : "▼") : "↕"}
        </span>
      )}
    </button>
  );
}
