// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  memo,
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
  type RefObject,
} from "react";
import type { Citation, Message } from "../lib/types";
import { useDepth } from "../theme";
import { useReader } from "../lib/reader";
import { formatDate } from "../lib/format";

interface Props {
  messages: Message[];
  /** Live assistant text while a reply streams in; null when idle. */
  streaming: string | null;
  /** Open a past chat a citation points to, at its cited turn (board card 7E PR3). Absent in surfaces
   *  where chat citations don't navigate. */
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
  /** A turn to scroll to and briefly flash — set when a chat citation clicked elsewhere navigated here
   *  to this exact turn. Carries a `nonce` that bumps on every click so clicking the *same* citation
   *  again re-fires the jump (a bare id wouldn't: React bails on a same-value state set). */
  focusTurn?: { id: number; nonce: number } | null;
}

/** A conversation reopened after this long reads as "resumed" — we mark it so the user knows they're
 *  continuing an older thread rather than mid-flow (board card 7E). 12h ≈ "not the same sitting". */
const RESUME_AFTER_MS = 12 * 60 * 60 * 1000;

/** The last-active date to show as a "resumed" marker, or null when the thread is fresh/current. Pure: the
 *  most recent message older than the resume threshold ⇒ this is a reopened conversation. Once the user
 *  sends again the newest message is current, so the marker naturally clears. */
function resumeMarkerDate(messages: Message[], now: number): string | null {
  const last = messages[messages.length - 1];
  if (!last) return null;
  const t = new Date(last.created_at).getTime();
  if (Number.isNaN(t) || now - t < RESUME_AFTER_MS) return null;
  return formatDate(last.created_at);
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
  onOpenChatCitation,
}: {
  citations: Citation[];
  itemRefs: RefObject<(HTMLLIElement | null)[]>;
  flash: number | null;
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
}) {
  // A document citation opens the shared reader onto that document (mounted at app scope), so the user
  // can see what the answer drew from without leaving the conversation.
  const { openReaderById } = useReader();
  return (
    <div className="flex justify-start" data-help="chat-sources">
      <div className="max-w-[80%] text-xs text-ink4">
        <span className="mr-2 font-medium text-ink3">Sources</span>
        <ol className="mt-1 flex flex-col gap-0.5">
          {citations.map((c, i) => {
            // A chat citation reads "from [chat], DATE" and opens the archived conversation at the
            // cited turn; a document citation opens that document in the reader (card 7E PR3).
            const chatLink = c.is_chat && c.conversation_id != null && onOpenChatCitation;
            return (
              <li
                key={`${c.document_id}-${i}`}
                ref={(el) => {
                  itemRefs.current[i] = el;
                }}
                title={
                  chatLink ? "Open this chat at the cited turn" : (c.source_path ?? c.vault_path)
                }
                className="rounded-[var(--radius-sm)] px-1 transition-colors duration-500 motion-reduce:transition-none"
                style={flash === i + 1 ? { background: "var(--accent-soft)" } : undefined}
              >
                <span className="text-faint">[{i + 1}]</span>{" "}
                {chatLink ? (
                  <button
                    type="button"
                    onClick={() => onOpenChatCitation(c.conversation_id!, c.turn_id ?? null)}
                    className="border-0 bg-transparent p-0 text-left align-baseline text-accent-text underline decoration-dotted underline-offset-2 transition hover:brightness-110 motion-reduce:transition-none"
                  >
                    from {c.title}
                    {c.dated ? `, ${formatDate(c.dated)}` : ""}
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => openReaderById(c.document_id)}
                    className="border-0 bg-transparent p-0 text-left align-baseline text-accent-text underline decoration-dotted underline-offset-2 transition hover:brightness-110 motion-reduce:transition-none"
                  >
                    {c.title}
                  </button>
                )}
              </li>
            );
          })}
        </ol>
      </div>
    </div>
  );
}

/** One message and, for a grounded assistant turn, its sources — wired so an inline
 *  `[n]` marker scrolls to and flashes source n. `highlight` flashes the whole turn when a chat
 *  citation navigated here to it (card 7E PR3); `registerBlock` lets the parent scroll it into view. */
// Memoised (F-50): while a reply streams, ChatView re-renders on every token — without this every
// prior turn's MessageBlock would re-render too. `registerBlock` and `onOpenChatCitation` are passed as
// STABLE callbacks and `highlight` is a plain bool, so a settled turn's props don't change token-to-token
// and React skips it. `message` is referentially stable (it comes from the unchanged `messages` array).
const MessageBlock = memo(function MessageBlock({
  message,
  onOpenChatCitation,
  highlight,
  registerBlock,
}: {
  message: Message;
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
  highlight?: boolean;
  registerBlock: (id: number, el: HTMLDivElement | null) => void;
}) {
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
    <div
      ref={(el) => registerBlock(message.id, el)}
      className={`flex flex-col gap-1.5 rounded-[var(--radius)] transition-shadow duration-500 motion-reduce:transition-none ${
        highlight ? "ring-1 ring-[color-mix(in_oklab,var(--accent)_50%,transparent)]" : ""
      }`}
    >
      <Bubble
        role={message.role}
        content={message.content}
        citationCount={showSources ? citations.length : 0}
        onCite={showSources ? jumpToSource : undefined}
      />
      {showSources && (
        <Sources
          citations={citations}
          itemRefs={itemRefs}
          flash={flash}
          onOpenChatCitation={onOpenChatCitation}
        />
      )}
    </div>
  );
});

export function ChatView({ messages, streaming, onOpenChatCitation, focusTurn }: Props) {
  const endRef = useRef<HTMLDivElement>(null);
  const { atLeast } = useDepth();
  // Per-turn refs so a chat citation that navigated here can scroll straight to its turn.
  const blockRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  const [flashMsg, setFlashMsg] = useState<number | null>(null);
  // The nonce of the focus request we've already handled, so a later message arriving (a reply
  // streaming in) doesn't yank the scroll back up to the old cited turn. Keyed on the nonce, not the
  // turn id, so re-clicking the *same* citation (fresh nonce) re-fires while streaming replies don't.
  const lastNonceRef = useRef<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Stable callbacks so memoised MessageBlocks don't re-render every streaming token (F-50). The
  // citation handler reads the latest prop through a ref, so it stays referentially stable even if the
  // parent passes a fresh function each render.
  const onCiteRef = useRef(onOpenChatCitation);
  onCiteRef.current = onOpenChatCitation;
  const openCitation = useCallback(
    (conversationId: number, turnId: number | null) => onCiteRef.current?.(conversationId, turnId),
    [],
  );
  const registerBlock = useCallback((id: number, el: HTMLDivElement | null) => {
    if (el) blockRefs.current.set(id, el);
    else blockRefs.current.delete(id);
  }, []);

  // Snap to the newest turn when the message set changes (a new turn, or a conversation just opened).
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);
  // While a reply streams, only stay pinned to the bottom if the user is ALREADY near it — so they can
  // scroll up to read earlier turns mid-stream without being dragged back down every token (F-50).
  useEffect(() => {
    if (streaming === null) return;
    const el = scrollRef.current;
    if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 120) {
      endRef.current?.scrollIntoView({ behavior: "auto" });
    }
  }, [streaming]);

  // Arrive on a cited turn: scroll it into view and flash it once (mirrors ProjectView's file focus).
  // Depends on `messages` too, so it still fires when the target conversation's turns load a tick after
  // the request is set. Declared after the scroll-to-bottom effect so it wins the final scroll position.
  useEffect(() => {
    if (focusTurn == null || focusTurn.nonce === lastNonceRef.current) return;
    const el = blockRefs.current.get(focusTurn.id);
    if (!el) return; // turns not loaded yet — the messages-dep re-run will catch it
    lastNonceRef.current = focusTurn.nonce;
    el.scrollIntoView({ behavior: "smooth", block: "center" });
    setFlashMsg(focusTurn.id);
    const clear = window.setTimeout(
      () => setFlashMsg((cur) => (cur === focusTurn.id ? null : cur)),
      2000,
    );
    return () => window.clearTimeout(clear);
  }, [focusTurn, messages]);

  const empty = messages.length === 0 && streaming === null;
  // Only meaningful while idle (a streaming reply means we're mid-flow, not resuming).
  const resumedOn =
    atLeast("standard") && streaming === null ? resumeMarkerDate(messages, Date.now()) : null;

  return (
    <div ref={scrollRef} className="flex-1 overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-4 px-4 py-6">
        {empty && (
          <div className="mt-24 text-center text-ink3">
            <p className="text-sm">Start a conversation.</p>
          </div>
        )}
        {resumedOn && (
          <div className="flex justify-center" data-help="chat-resumed">
            <span className="rounded-full bg-surface px-3 py-1 text-xs text-ink4">
              Resumed · last active {resumedOn}
            </span>
          </div>
        )}
        {messages.map((m) => (
          <MessageBlock
            key={m.id}
            message={m}
            onOpenChatCitation={openCitation}
            highlight={flashMsg === m.id}
            registerBlock={registerBlock}
          />
        ))}
        {streaming !== null && <Bubble role="assistant" content={streaming} />}
        <div ref={endRef} />
      </div>
    </div>
  );
}
