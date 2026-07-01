// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useMemo, useRef, useState } from "react";
import { ChatView } from "./ChatView";
import { ChatHistoryList } from "./ChatHistoryList";
import { Composer } from "./Composer";
import {
  createConversation,
  getMessages,
  listCalendarEvents,
  listConversations,
  listDocuments,
  listMilestones,
  listProjects,
  setDocumentMetadata,
} from "../lib/ipc";
import { useChatStream } from "../lib/useChatStream";
import { useResizable } from "../lib/useResizable";
import { useSidebarSplit } from "../lib/useSidebarSplit";
import { idleSince, isNewChatTrigger } from "../lib/chatSession";
import type { CalendarEvent, Conversation, Document, Milestone } from "../lib/types";
import { Button, Input } from "./ui";
import { ImportancePicker } from "./ImportancePicker";
import { MilestoneList } from "./MilestoneList";
import { TagEditor } from "./TagEditor";
import { rankImportance } from "../lib/importance";
import { useDepth, useTheme } from "../theme";

const PROJECT_LIST_ID = "focus-projects";

type FileSortKey = "name" | "importance";
interface FileSort {
  key: FileSortKey;
  dir: "asc" | "desc";
}

interface Props {
  project: string;
  /** A file to scroll to and briefly highlight (set by the command palette). */
  focusDocId?: number | null;
  onBack: () => void;
}

/** Per-project scoped view (spec §4): the project's files alongside a chat whose
 *  retrieval is confined to just this project — "everything narrows to just it".
 *  The scoped chat keeps its own conversation (created lazily on first message
 *  with this project set, so the backend scopes grounding to it). */
export function ProjectView({ project, focusDocId, onBack }: Props) {
  const [documents, setDocuments] = useState<Document[]>([]);
  const [milestones, setMilestones] = useState<Milestone[]>([]);
  const [calendarEvents, setCalendarEvents] = useState<CalendarEvent[]>([]);
  // This project's past chats (for the sidebar history panel), newest-first from the backend.
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [convId, setConvId] = useState<number | null>(null);
  // The conversation whose idle-prompt the user dismissed, so it doesn't nag again this session.
  const [dismissedIdleFor, setDismissedIdleFor] = useState<number | null>(null);
  // Mirror convId for the stream guard so switching projects (which nulls convId)
  // abandons an in-flight reply instead of letting it land in the new project.
  const convIdRef = useRef(convId);
  convIdRef.current = convId;
  const chat = useChatStream(() => convIdRef.current);
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
  // How the Files panel is ordered. Name A→Z by default; clicking a key again reverses it.
  const [sort, setSort] = useState<FileSort>({ key: "name", dir: "asc" });
  // The right panel's width is a fraction of the window (drag the left edge to resize, stays
  // proportional on window resize), clamped so it can't get so narrow the content scrolls.
  const {
    width: asideWidth,
    startResize,
    resizing,
  } = useResizable({
    storageKey: "pm.project.sidebarFrac",
    defaultFrac: 0.24,
    minFrac: 0.16,
    maxFrac: 0.5,
    edge: "left",
  });
  // The sidebar splits vertically: chat history (top) + documents (bottom). The ratio is a hard
  // pref across the app (card 7E) — see useSidebarSplit.
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

  const refreshMilestones = () => {
    listMilestones(project)
      .then(setMilestones)
      .catch(() => {});
  };

  const refreshConversations = () => {
    listConversations()
      .then((all) => setConversations(all.filter((c) => c.project === project)))
      .catch(() => {});
  };

  useEffect(() => {
    // Reset chat when switching projects (also abandons any in-flight reply). Opening a project
    // lands on a fresh chat pane; its past chats live in the sidebar history to resume from.
    setConvId(null);
    setDismissedIdleFor(null);
    chat.clearTransient();
    chat.setMessages([]);
    refreshConversations();
    listDocuments()
      .then((all) => setDocuments(all.filter((d) => d.project === project)))
      .catch((e) => chat.setError(String(e)));
    refreshMilestones();
    // Upcoming events feed the milestone link picker (empty when no calendar connected).
    listCalendarEvents()
      .then(setCalendarEvents)
      .catch(() => setCalendarEvents([]));
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
    patch: Partial<Pick<Document, "project" | "tags" | "importance">>,
  ) {
    const nextProject = patch.project ?? doc.project;
    const nextTags = patch.tags ?? doc.tags;
    const nextImportance = patch.importance !== undefined ? patch.importance : doc.importance;
    try {
      const updated = await setDocumentMetadata(doc.id, nextProject, nextTags, nextImportance);
      setDocuments((docs) =>
        updated.project !== project
          ? docs.filter((d) => d.id !== updated.id)
          : docs.map((d) => (d.id === updated.id ? updated : d)),
      );
    } catch (e) {
      chat.setError(String(e));
    }
  }

  /** Resume a past chat from the history panel: swap its turns into the pane. Guarded so a fast
   *  project switch mid-load can't drop stale messages into the new project. */
  async function openConversation(id: number) {
    if (id === convId) return;
    setConvId(id);
    setDismissedIdleFor(null);
    chat.clearTransient();
    try {
      const msgs = await getMessages(id);
      if (convIdRef.current === id) chat.setMessages(msgs);
    } catch (e) {
      chat.setError(String(e));
    }
  }

  /** Start a fresh chat in the pane (the "+ New chat" button / the /new trigger). The just-left chat
   *  is already persisted and shows in history, so this is a clean swap — nothing to archive. */
  function newChat() {
    setConvId(null);
    chat.clearTransient();
    chat.setMessages([]);
  }

  async function handleSend(text: string) {
    // /new · /done starts a fresh chat instead of sending — never reaches the model or the vault.
    if (isNewChatTrigger(text)) {
      newChat();
      return;
    }
    let id = convId;
    if (id == null) {
      try {
        const created = await createConversation(project);
        id = created.id;
        setConvId(id);
      } catch (e) {
        chat.setError(String(e));
        return;
      }
    }

    await chat.send(id, text);

    // Adopt the persisted messages only if we're still on this project's chat, then refresh history
    // so a just-created chat (and any background-generated title/order) shows in the panel.
    try {
      if (convIdRef.current === id) chat.setMessages(await getMessages(id));
    } catch {
      /* keep optimistic state on reload failure */
    }
    refreshConversations();
  }

  // Reopening a chat idle > 24h offers a clean start (card 7E) — purely UX framing; the turns were
  // already indexed under card B. Only for a loaded, settled thread the user hasn't dismissed.
  const idleDate =
    atLeast("standard") && convId != null && chat.streaming === null && dismissedIdleFor !== convId
      ? idleSince(chat.messages, Date.now())
      : null;

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
          <ChatView messages={chat.messages} streaming={chat.streaming} />
          {idleDate && (
            <div
              className="flex items-center justify-between gap-3 border-t border-border px-4 py-2 text-xs text-ink3"
              data-help="chat-idle-prompt"
            >
              <span>This conversation has been idle since {idleDate}. Start a new one?</span>
              <div className="flex shrink-0 items-center gap-3">
                <Button variant="secondary" onClick={newChat} className="px-2 py-1 text-xs">
                  New chat
                </Button>
                <button
                  type="button"
                  onClick={() => setDismissedIdleFor(convId)}
                  title="Dismiss"
                  className="text-ink4 hover:text-ink2"
                >
                  Dismiss
                </button>
              </div>
            </div>
          )}
          <Composer disabled={chat.sending} onSend={handleSend} />
        </main>

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
            title="Drag to resize"
            data-help="project-resize"
            className={`absolute left-0 top-0 z-10 h-full w-1.5 cursor-col-resize touch-none transition-colors hover:bg-[color-mix(in_oklab,var(--accent)_45%,transparent)] ${
              resizing ? "bg-[color-mix(in_oklab,var(--accent)_60%,transparent)]" : ""
            }`}
          />
          <div ref={splitRef} className="flex min-h-0 flex-1 flex-col">
            {/* Chat history (top). The history/documents ratio is a hard pref — useSidebarSplit. */}
            <div
              style={{ flexBasis: `${topFrac * 100}%` }}
              className="min-h-0 shrink-0 grow-0 overflow-hidden"
            >
              <ChatHistoryList
                conversations={conversations}
                activeId={convId}
                onSelect={openConversation}
                onNew={newChat}
              />
            </div>
            <div
              onPointerDown={startSplit}
              role="separator"
              aria-orientation="horizontal"
              aria-label="Resize chat history and documents"
              title="Drag to resize"
              data-help="project-split"
              className={`h-1.5 shrink-0 cursor-row-resize touch-none border-y border-border transition-colors hover:bg-[color-mix(in_oklab,var(--accent)_45%,transparent)] ${
                splitting ? "bg-[color-mix(in_oklab,var(--accent)_60%,transparent)]" : ""
              }`}
            />
            {/* Project documents (bottom): milestones + files. */}
            <div
              className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden"
              data-help="project-files"
            >
              {(showMeta || milestones.length > 0) && (
                <div
                  className="border-b border-border px-4 pb-3 pt-3"
                  data-help="project-milestones"
                >
                  <span className="font-mono text-xs uppercase tracking-wide text-ink4">
                    Milestones
                  </span>
                  <div className="mt-2">
                    <MilestoneList
                      project={project}
                      milestones={milestones}
                      calendarEvents={calendarEvents}
                      onChanged={refreshMilestones}
                      readOnly={!showMeta}
                    />
                  </div>
                </div>
              )}
              <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3">
                <span className="font-mono text-xs uppercase tracking-wide text-ink4">Files</span>
                {documents.length > 1 && (
                  <div className="flex items-center gap-2 text-[10px] text-ink4">
                    <FileSortButton label="Name" sortKey="name" sort={sort} onSort={toggleSort} />
                    <FileSortButton
                      label="Importance"
                      sortKey="importance"
                      sort={sort}
                      onSort={toggleSort}
                    />
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
                      className={`rounded-[var(--radius-sm)] px-2 py-1.5 transition-colors hover:bg-surface ${
                        flashId === d.id
                          ? "bg-surface ring-1 ring-[color-mix(in_oklab,var(--accent)_50%,transparent)]"
                          : ""
                      }`}
                    >
                      <div className="truncate font-head text-sm text-ink2" title={d.title}>
                        {d.title}
                      </div>
                      {teachVisible ? (
                        // Manual triage, same controls as the Review tab. Everyone gets the importance
                        // toggle; power depth also gets project (re-file) + tags, like a Review row.
                        <div className="mt-1.5 flex flex-col gap-1.5">
                          {showPower && (
                            <label className="flex items-center gap-1.5 text-xs text-ink4">
                              <span className="shrink-0">Project</span>
                              <Input
                                key={d.project}
                                list={PROJECT_LIST_ID}
                                defaultValue={d.project}
                                onBlur={(e) => {
                                  const v = e.target.value.trim();
                                  if (v && v !== d.project) void saveMeta(d, { project: v });
                                }}
                                className="h-6 w-full text-xs"
                              />
                            </label>
                          )}
                          <ImportancePicker
                            value={d.importance}
                            onChange={(importance) => void saveMeta(d, { importance })}
                          />
                          {showPower && (
                            <TagEditor
                              tags={d.tags}
                              onChange={(tags) => void saveMeta(d, { tags })}
                            />
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
            </div>
          </div>
        </aside>
      </div>
    </div>
  );
}

/** A compact sort toggle for the Files panel header: click to sort by this key, click again to
 *  reverse. Shows the active direction (▲/▼) or an idle hint (↕). */
function FileSortButton({
  label,
  sortKey,
  sort,
  onSort,
}: {
  label: string;
  sortKey: FileSortKey;
  sort: FileSort;
  onSort: (key: FileSortKey) => void;
}) {
  const active = sort.key === sortKey;
  return (
    <button
      type="button"
      onClick={() => onSort(sortKey)}
      title={`Sort by ${label.toLowerCase()}`}
      className={`inline-flex items-center gap-0.5 hover:text-ink2 ${active ? "text-ink2" : ""}`}
    >
      {label}
      <span aria-hidden className="text-[8px] leading-none">
        {active ? (sort.dir === "asc" ? "▲" : "▼") : "↕"}
      </span>
    </button>
  );
}
