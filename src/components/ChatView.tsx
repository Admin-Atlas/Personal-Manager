// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef } from "react";
import type { Citation, Message } from "../lib/types";
import { useDepth } from "../theme";

interface Props {
  messages: Message[];
  /** Live assistant text while a reply streams in; null when idle. */
  streaming: string | null;
}

function Bubble({ role, content }: { role: string; content: string }) {
  const isUser = role === "user";
  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`max-w-[80%] whitespace-pre-wrap rounded-[var(--radius)] px-4 py-2.5 text-sm leading-relaxed ${
          isUser
            ? "bg-accent text-accent-ink"
            : "bg-surface text-ink"
        }`}
      >
        {content || <span className="text-ink4">…</span>}
      </div>
    </div>
  );
}

/** The documents an answer drew from, listed under the assistant bubble. */
function Sources({ citations }: { citations: Citation[] }) {
  return (
    <div className="flex justify-start" data-help="chat-sources">
      <div className="max-w-[80%] text-xs text-ink4">
        <span className="mr-2 font-medium text-ink3">Sources</span>
        <ol className="mt-1 flex flex-col gap-0.5">
          {citations.map((c, i) => (
            <li key={`${c.document_id}-${i}`} title={c.source_path ?? c.vault_path}>
              <span className="text-faint">[{i + 1}]</span> {c.title}
            </li>
          ))}
        </ol>
      </div>
    </div>
  );
}

export function ChatView({ messages, streaming }: Props) {
  const endRef = useRef<HTMLDivElement>(null);
  const { atLeast } = useDepth();

  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, streaming]);

  const empty = messages.length === 0 && streaming === null;

  return (
    <div className="flex-1 overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-4 px-4 py-6">
        {empty && (
          <div className="mt-24 text-center text-ink3">
            <p className="text-sm">Start a conversation.</p>
          </div>
        )}
        {messages.map((m) => (
          <div key={m.id} className="flex flex-col gap-1.5">
            <Bubble role={m.role} content={m.content} />
            {atLeast("standard") &&
              m.role === "assistant" &&
              m.citations &&
              m.citations.length > 0 && <Sources citations={m.citations} />}
          </div>
        ))}
        {streaming !== null && <Bubble role="assistant" content={streaming} />}
        <div ref={endRef} />
      </div>
    </div>
  );
}
