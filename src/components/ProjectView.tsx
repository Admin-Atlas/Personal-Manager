// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import { ChatView } from "./ChatView";
import { Composer } from "./Composer";
import { createConversation, getMessages, listDocuments } from "../lib/ipc";
import { useChatStream } from "../lib/useChatStream";
import type { Document } from "../lib/types";
import { Button } from "./ui";
import { useDepth } from "../theme";

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
  const [convId, setConvId] = useState<number | null>(null);
  // Mirror convId for the stream guard so switching projects (which nulls convId)
  // abandons an in-flight reply instead of letting it land in the new project.
  const convIdRef = useRef(convId);
  convIdRef.current = convId;
  const chat = useChatStream(() => convIdRef.current);
  /** The file the palette jumped to — flashed briefly, then cleared. */
  const [flashId, setFlashId] = useState<number | null>(null);
  const filesRef = useRef<HTMLUListElement>(null);
  const { showMeta } = useDepth();

  useEffect(() => {
    // Reset chat when switching projects (also abandons any in-flight reply).
    setConvId(null);
    chat.clearTransient();
    chat.setMessages([]);
    listDocuments()
      .then((all) => setDocuments(all.filter((d) => d.project === project)))
      .catch((e) => chat.setError(String(e)));
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

  async function handleSend(text: string) {
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

    // Adopt the persisted messages only if we're still on this project's chat.
    try {
      if (convIdRef.current === id) chat.setMessages(await getMessages(id));
    } catch {
      /* keep optimistic state on reload failure */
    }
  }

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-border bg-panel px-6 py-3">
        <Button variant="tertiary" onClick={onBack} title="Back to Focus">
          ← Focus
        </Button>
        <div className="min-w-0">
          <h1 className="truncate font-head text-sm font-semibold text-ink">{project}</h1>
          <p className="font-mono text-xs text-ink4">
            {documents.length} document{documents.length === 1 ? "" : "s"} · chat scoped to this project
          </p>
        </div>
      </header>

      <div className="flex flex-1 overflow-hidden">
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
          <Composer disabled={chat.sending} onSend={handleSend} />
        </main>

        <aside
          className="w-80 shrink-0 overflow-y-auto border-l border-border bg-panel"
          data-help="project-files"
        >
          <p className="px-4 pb-1 pt-3 font-mono text-xs uppercase tracking-wide text-ink4">Files</p>
          {documents.length === 0 ? (
            <p className="px-4 py-2 text-xs text-ink4">No documents in this project.</p>
          ) : (
            <ul ref={filesRef} className="flex flex-col gap-0.5 px-2 pb-4">
              {documents.map((d) => (
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
                  {showMeta && (
                    <div className="flex gap-2 font-mono text-xs text-ink4">
                      {d.importance && <span className="capitalize">{d.importance}</span>}
                      <span>
                        {d.chunk_count} chunk{d.chunk_count === 1 ? "" : "s"}
                      </span>
                    </div>
                  )}
                </li>
              ))}
            </ul>
          )}
        </aside>
      </div>
    </div>
  );
}
