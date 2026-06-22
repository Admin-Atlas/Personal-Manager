// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import type { Citation, Message } from "../lib/types";
import { useDepth } from "../theme";

interface Props {
  messages: Message[];
  /** Live assistant text while a reply streams in; null when idle. */
  streaming: string | null;
}

/** Render assistant text with inline `[n]` citation markers turned into buttons that
 *  jump to the matching source (the grounding prompt asks the model to cite as
 *  [1], [2], …). A marker outside the citation range stays plain text. */
function renderWithCitations(
  content: string,
  count: number,
  onCite: (n: number) => void,
): ReactNode {
  return content.split(/(\[\d+\])/g).map((part, i) => {
    const m = /^\[(\d+)\]$/.exec(part);
    const n = m ? Number(m[1]) : 0;
    if (n >= 1 && n <= count) {
      return (
        <button
          key={i}
          type="button"
          onClick={() => onCite(n)}
          title={`Jump to source ${n}`}
          className="border-0 bg-transparent p-0 align-baseline font-medium text-accent-text underline decoration-dotted underline-offset-2 transition hover:brightness-110 motion-reduce:transition-none"
        >
          {part}
        </button>
      );
    }
    return <span key={i}>{part}</span>;
  });
}

function Bubble({
  role,
  content,
  citationCount = 0,
  onCite,
}: {
  role: string;
  content: string;
  citationCount?: number;
  onCite?: (n: number) => void;
}) {
  const isUser = role === "user";
  const body =
    onCite && citationCount > 0
      ? renderWithCitations(content, citationCount, onCite)
      : content || <span className="text-ink4">…</span>;
  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        className={`max-w-[80%] whitespace-pre-wrap rounded-[var(--radius)] px-4 py-2.5 text-sm leading-relaxed ${
          isUser ? "bg-accent text-accent-ink" : "bg-surface text-ink"
        }`}
      >
        {body}
      </div>
    </div>
  );
}

/** The documents an answer drew from, under the assistant bubble. Each item carries a
 *  ref so a clicked `[n]` marker can scroll to + briefly highlight its source. */
function Sources({
  citations,
  itemRefs,
  flash,
}: {
  citations: Citation[];
  itemRefs: RefObject<(HTMLLIElement | null)[]>;
  flash: number | null;
}) {
  return (
    <div className="flex justify-start" data-help="chat-sources">
      <div className="max-w-[80%] text-xs text-ink4">
        <span className="mr-2 font-medium text-ink3">Sources</span>
        <ol className="mt-1 flex flex-col gap-0.5">
          {citations.map((c, i) => (
            <li
              key={`${c.document_id}-${i}`}
              ref={(el) => {
                itemRefs.current[i] = el;
              }}
              title={c.source_path ?? c.vault_path}
              className="rounded-[var(--radius-sm)] px-1 transition-colors duration-500 motion-reduce:transition-none"
              style={flash === i + 1 ? { background: "var(--accent-soft)" } : undefined}
            >
              <span className="text-faint">[{i + 1}]</span> {c.title}
            </li>
          ))}
        </ol>
      </div>
    </div>
  );
}

/** One message and, for a grounded assistant turn, its sources — wired so an inline
 *  `[n]` marker scrolls to and flashes source n. */
function MessageBlock({ message }: { message: Message }) {
  const { atLeast } = useDepth();
  const itemRefs = useRef<(HTMLLIElement | null)[]>([]);
  const [flash, setFlash] = useState<number | null>(null);

  const citations = message.role === "assistant" ? (message.citations ?? []) : [];
  const showSources = atLeast("standard") && citations.length > 0;

  const jumpToSource = (n: number) => {
    const el = itemRefs.current[n - 1];
    if (!el) return;
    el.scrollIntoView({ behavior: "smooth", block: "nearest" });
    setFlash(n);
    window.setTimeout(() => setFlash((cur) => (cur === n ? null : cur)), 1500);
  };

  return (
    <div className="flex flex-col gap-1.5">
      <Bubble
        role={message.role}
        content={message.content}
        citationCount={showSources ? citations.length : 0}
        onCite={showSources ? jumpToSource : undefined}
      />
      {showSources && <Sources citations={citations} itemRefs={itemRefs} flash={flash} />}
    </div>
  );
}

export function ChatView({ messages, streaming }: Props) {
  const endRef = useRef<HTMLDivElement>(null);

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
          <MessageBlock key={m.id} message={m} />
        ))}
        {streaming !== null && <Bubble role="assistant" content={streaming} />}
        <div ref={endRef} />
      </div>
    </div>
  );
}
