// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  memo,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent,
  type RefObject,
} from "react";
import type {
  AnswerRating,
  Citation,
  GroundingConfidence,
  Message,
  PromptMessage,
} from "../lib/types";
import { useDepth } from "../theme";
import { useDevMode } from "../lib/capabilities";
import { useReader } from "../lib/reader";
import { Markdown } from "../lib/markdown";
import { citationTarget, linkCitations } from "../lib/chatMarkdown";
import { splitMentions } from "../lib/mentions";
import { listTags } from "../lib/ipc";
import { formatDate, formatDateLocal, shortModel } from "../lib/format";
import {
  answerFeedback,
  getSettings,
  ingestNote,
  rateAnswer,
  recordCitationClick,
  setRetrievalConfidenceThreshold,
} from "../lib/ipc";
import { IconButton, VisuallyHidden } from "./ui";

interface Props {
  messages: Message[];
  /** Live assistant text while a reply streams in; null when idle. */
  streaming: string | null;
  /** Developer mode only: the exact request PM sent for a turn, keyed by assistant message id, shown
   *  in a collapsed "prompt sent to the API" dropdown under that turn (card #395). Only turns sent this
   *  session carry one; reloaded history turns don't. */
  prompts?: Record<number, PromptMessage[]>;
  /** Developer mode only: the grounding-confidence readout for a turn (top rerank score / threshold /
   *  gated), keyed by assistant message id, shown under the answer for calibrating the gate (card #402). */
  confidences?: Record<number, GroundingConfidence>;
  /** Which provider answered each turn ("local"/"cloud"), keyed by assistant message id (#297). Live-
   *  session only (from the `done` event) — a reloaded-from-history turn has no entry and shows the
   *  model name alone (the provider is not persisted). */
  providers?: Record<number, "local" | "cloud">;
  /** Show the per-message "via <model> · local/cloud" provenance footer. True only when a local
   *  endpoint is configured, so a cloud-only user sees no change (#297). */
  showProvenance?: boolean;
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

/**
 * A user's message with the `@mentions` that RESOLVED marked.
 *
 * Only real tags are marked, and that is the whole point of showing it: it is how someone sees that
 * `@marketing` reached their Marketing files and that `@markting` did not — a pin that silently
 * matched nothing is otherwise indistinguishable from one that worked. A message with no resolved
 * mentions renders as one plain string, exactly as before.
 */
function MentionText({ text, known }: { text: string; known?: readonly string[] }) {
  const segments = useMemo(() => splitMentions(text, known ?? []), [text, known]);
  if (segments.length === 1 && !segments[0].tag) return <>{text}</>;
  return (
    <>
      {segments.map((seg, i) =>
        seg.tag ? (
          <span
            key={i}
            className="rounded-[var(--radius-sm)] bg-[color-mix(in_oklab,currentColor_18%,transparent)] px-1 font-medium"
            title={`Pinned ${seg.tag} for this message — it also searched that tag's documents`}
          >
            {seg.text}
          </span>
        ) : (
          <span key={i}>{seg.text}</span>
        ),
      )}
    </>
  );
}

function Bubble({
  role,
  content,
  citationCount = 0,
  onCite,
  markdown = false,
  knownTags,
}: {
  role: string;
  content: string;
  citationCount?: number;
  onCite?: (n: number) => void;
  /** Render the body as Markdown through the sanitizing boundary. Set for the model's answers, not
   *  for the user's own message — what someone typed should never be reinterpreted. */
  markdown?: boolean;
  /** Tag names that exist, so a `@mention` that actually resolved can be marked (#276). Only the
   *  user's own turns are checked; the model does not pin tags. */
  knownTags?: readonly string[];
}) {
  const isUser = role === "user";
  // A streaming reply sets `content` to the whole accumulated answer on EVERY token, and a Markdown
  // render of a ~2 KB answer measures ~8 ms — landing on each token that would be ~1 s of main-thread
  // work across one reply, on top of the parse growing as the answer does. Deferring lets React run
  // it at low priority and drop superseded intermediates, so the token feed and scrolling stay
  // smooth and the formatting still updates live. Settled turns are unaffected: their content never
  // changes, so the deferred value equals it immediately.
  const deferred = useDeferredValue(content);
  const text = markdown ? deferred : content;

  // One delegated listener instead of a handler per marker: the citation links are produced by the
  // Markdown renderer, so there is nothing to attach to individually. A real `<a href>` also fires
  // click on Enter, which keeps the citations keyboard-operable exactly as the old buttons were.
  const onCiteClick = (e: MouseEvent<HTMLDivElement>) => {
    if (!onCite) return;
    const href = (e.target as HTMLElement).closest?.("a[href]")?.getAttribute("href");
    const n = citationTarget(href, citationCount);
    if (n === null) return;
    e.preventDefault();
    onCite(n);
  };

  const body = !text ? (
    <span className="text-ink4">…</span>
  ) : markdown ? (
    <Markdown>{linkCitations(text, citationCount)}</Markdown>
  ) : (
    // Plain React nodes, NOT a trip through the Markdown boundary. The user's own text is
    // deliberately never reinterpreted as Markdown (see `markdown` above), and it does not need to
    // be: splitting a string into spans introduces no HTML, so the sanitizer's allowlist — which
    // permits neither `mark` nor a `class` on `span` — stays exactly as strict as it is.
    <MentionText text={text} known={knownTags} />
  );

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"}`}>
      <div
        onClick={markdown && onCite ? onCiteClick : undefined}
        className={`max-w-[80%] rounded-[var(--radius)] px-4 py-2.5 text-sm leading-relaxed ${
          markdown ? "pm-inline-md" : "whitespace-pre-wrap"
        } ${isUser ? "bg-accent text-accent-ink" : "bg-surface text-ink"}`}
      >
        {/* Author, for screen readers only: sighted users read it from left/right alignment + colour. */}
        <VisuallyHidden>{isUser ? "You said: " : "Assistant said: "}</VisuallyHidden>
        {body}
      </div>
    </div>
  );
}

/** A polite, screen-reader-only announcement of the assistant's reply once it finishes streaming. The
 *  visible bubble updates on every token, which would flood assistive tech, so we announce the whole
 *  answer once when the stream ends instead. */
function StreamAnnouncer({ streaming }: { streaming: string | null }) {
  const [reply, setReply] = useState("");
  const lastRef = useRef("");
  useEffect(() => {
    if (streaming !== null) {
      lastRef.current = streaming;
    } else if (lastRef.current) {
      setReply(lastRef.current);
      lastRef.current = "";
    }
  }, [streaming]);
  return (
    <VisuallyHidden role="status" aria-live="polite">
      {reply}
    </VisuallyHidden>
  );
}

/** The documents an answer drew from, under the assistant bubble. Each item carries a
 *  ref so a clicked `[n]` marker can scroll to + briefly highlight its source. */
function Sources({
  citations,
  itemRefs,
  flash,
  onOpenChatCitation,
  onCitationOpened,
}: {
  citations: Citation[];
  itemRefs: RefObject<(HTMLLIElement | null)[]>;
  flash: number | null;
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
  /** Opening a source is an implicit relevance signal (card 10) — logged, never blocking the open. */
  onCitationOpened?: (documentId: number) => void;
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
                    onClick={() => {
                      onCitationOpened?.(c.document_id);
                      openReaderById(c.document_id);
                    }}
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

/** Was this answer any good? (Stage-4 card 10.)
 *
 *  Two quiet buttons under a grounded answer. Nothing reads the signal yet — it accrues locally so a
 *  learned reranker has something to train on when that work lands, which it otherwise never would,
 *  because PM records no query-time relevance judgements anywhere. Clicking an already-set rating
 *  clears it, so the control is never a one-way door.
 *
 *  Deliberately unobtrusive: no prompt, no nag, no reward for answering. A rating that has to be
 *  coaxed is worse than no rating, because it is a judgement about being asked rather than about the
 *  answer. Only shown on answers that actually retrieved something — there is nothing to judge the
 *  relevance of otherwise. */
function AnswerRatingControls({ messageId }: { messageId: number }) {
  const [rating, setRating] = useState<AnswerRating | null>(null);

  // Reflect any rating already stored, so it survives reopening the conversation.
  useEffect(() => {
    let live = true;
    void answerFeedback(messageId)
      .then((f) => {
        if (live) setRating(f.rating);
      })
      // A feedback readout is never worth surfacing an error over; the controls just render unset.
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [messageId]);

  function choose(next: AnswerRating) {
    const value = rating === next ? null : next;
    setRating(value); // optimistic: the control must feel instant, and nothing depends on the write
    void rateAnswer(messageId, value).catch(() => {});
  }

  return (
    <div className="flex justify-start" data-help="chat-answer-rating">
      <div className="flex items-center gap-0.5">
        {(["up", "down"] as const).map((v) => {
          const on = rating === v;
          return (
            <IconButton
              key={v}
              // Chosen state is carried by the VARIANT, not by a colour in `className` — see the
              // `pressed` entry in IconButton for why the ad-hoc version silently did nothing. The
              // old control paired that dead class with a 👍 emoji, a full-colour glyph that ignores
              // `color` anyway, so a rating wrote to the database and showed no sign of it.
              variant={on ? "pressed" : "ghost"}
              aria-pressed={on}
              label={v === "up" ? "This answer was helpful" : "This answer missed"}
              title={
                on
                  ? "Clear this rating"
                  : v === "up"
                    ? "This answer was helpful"
                    : "This answer missed"
              }
              onClick={() => choose(v)}
              className="px-1.5 py-0.5"
            >
              <ThumbIcon down={v === "down"} />
            </IconButton>
          );
        })}
      </div>
    </div>
  );
}

/** A thumb, drawn rather than typed. Inline SVG inherits `currentColor`, which is the whole point:
 *  it lets the selected state actually show, and it renders the same on all three webview engines
 *  instead of leaving each platform's emoji font to decide what a thumb looks like. */
function ThumbIcon({ down }: { down: boolean }) {
  return (
    <svg
      viewBox="0 0 16 16"
      className="h-3.5 w-3.5"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinejoin="round"
      aria-hidden="true"
      // One path, flipped for the thumbs-down, so the two can never drift out of proportion.
      style={down ? { transform: "rotate(180deg)" } : undefined}
    >
      <path d="M4.5 14V6.8L8.2 1.6a1.6 1.6 0 0 1 2.6 1.8L9.6 6.2h3.2a1.6 1.6 0 0 1 1.55 2l-1.35 4.6A2 2 0 0 1 11.05 14H4.5Z" />
      <path d="M4.5 6.8H2.6A1.1 1.1 0 0 0 1.5 7.9v5A1.1 1.1 0 0 0 2.6 14h1.9" />
    </svg>
  );
}

/** Developer mode only (card #395): a default-collapsed dropdown revealing the exact request PM sent
 *  to the API for this turn — the system instructions and the single bundled user/context message,
 *  verbatim. Rendered as raw text (a `<pre>`, not Markdown): this is the literal payload the model
 *  received, so it must not be reformatted. React escapes the text, so no untrusted-content concern. */
function PromptPanel({ messages }: { messages: PromptMessage[] }) {
  return (
    <details
      data-help="chat-prompt-inspect"
      className="max-w-[80%] rounded-[var(--radius-sm)] border border-border2 bg-surface text-xs"
    >
      <summary className="cursor-pointer select-none px-2 py-1 text-ink4 hover:text-ink2">
        Prompt sent to the API · {messages.length} message{messages.length === 1 ? "" : "s"}
      </summary>
      <div className="flex flex-col gap-2 border-t border-rule px-2 py-2">
        {messages.map((m, i) => (
          <div key={i}>
            <div className="font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
              {m.role}
            </div>
            <pre className="mt-0.5 whitespace-pre-wrap break-words font-mono text-[0.6875rem] leading-snug text-ink2">
              {m.content}
            </pre>
          </div>
        ))}
      </div>
    </details>
  );
}

/** A "Save as note" affordance under an assistant answer (standard + power depth). PM's own answers
 *  are no longer indexed as retrievable grounding (they'd otherwise let the model re-ground on its own
 *  earlier output), so a genuinely useful one is kept by promoting it to a real, searchable vault note.
 *  Reuses the pinboard note-ingest path, keyed on the message id so it is idempotent (saving twice
 *  re-embeds in place, never duplicates), with a soft "saved from a chat" date breadcrumb for
 *  provenance. The note is standalone — deleting the chat never removes it. */
function SaveAsNoteButton({ message }: { message: Message }) {
  const [state, setState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const save = async () => {
    if (state === "saving" || state === "saved") return;
    setState("saving");
    try {
      const stamp = `_Saved from a chat, ${formatDateLocal(new Date())}_`;
      await ingestNote(`chat:${message.id}`, "", `${message.content}\n\n${stamp}`);
      setState("saved");
    } catch {
      setState("error");
    }
  };
  const label =
    state === "saving"
      ? "Saving…"
      : state === "saved"
        ? "Saved to notes ✓"
        : state === "error"
          ? "Couldn't save — retry"
          : "Save as note";
  return (
    <div className="flex justify-end">
      <button
        type="button"
        onClick={save}
        disabled={state === "saving" || state === "saved"}
        className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-xs text-ink4 transition-colors hover:text-ink2 disabled:cursor-default disabled:hover:text-ink4 motion-reduce:transition-none"
      >
        {label}
      </button>
    </div>
  );
}

/** Developer-mode grounding-confidence readout under an assistant answer (card #402): a compact,
 *  selectable/copy-pastable line showing the top rerank score, the active gate threshold, and whether
 *  the gate fired. The score is the calibration signal — read a few good vs. junk answers to find the
 *  threshold where they separate. */
function ConfidenceReadout({ confidence }: { confidence: GroundingConfidence }) {
  const top = confidence.top_score === null ? "— (ungrounded)" : confidence.top_score.toFixed(2);
  const thr = confidence.threshold === null ? "off" : confidence.threshold.toFixed(2);
  return (
    <div className="select-text font-mono text-[0.625rem] leading-snug text-ink4">
      confidence · top {top} · threshold {thr} · gated {confidence.gated ? "yes" : "no"}
    </div>
  );
}

/** The gate's default threshold, mirrored from the backend (`db::DEFAULT_CONFIDENCE_THRESHOLD`) only to
 *  seed the control when a dev flips the gate back on. The backend is authoritative for the value
 *  actually applied — it uses this default whenever the setting is absent, so the gate is on for
 *  everyone without the frontend writing anything. */
const DEFAULT_CONFIDENCE_THRESHOLD = -8.5;

/** Developer-mode control (card #402) to tune the confidence gate live for calibration: the minimum top
 *  rerank score for PM to trust its grounding. ON by default at the calibrated threshold; a dev can
 *  change the number or switch the gate off. Off is an EXPLICIT toggle and the number box is disabled
 *  when off, so a stray click/blank can't silently arm a "gate everything" 0 (the old footgun). Reads
 *  the current value once; writes on change (stateless backend). */
function ConfidenceThresholdControl() {
  // `threshold`: null = gate off; a number = gate on at that score. `draft` mirrors the number box so a
  // dev can clear and retype without the committed value snapping around mid-edit.
  const [threshold, setThreshold] = useState<number | null>(null);
  const [draft, setDraft] = useState(String(DEFAULT_CONFIDENCE_THRESHOLD));
  const [loaded, setLoaded] = useState(false);
  useEffect(() => {
    getSettings()
      .then((s) => {
        setThreshold(s.retrieval_confidence_threshold);
        if (s.retrieval_confidence_threshold !== null)
          setDraft(String(s.retrieval_confidence_threshold));
      })
      .catch(() => {})
      .finally(() => setLoaded(true));
  }, []);
  if (!loaded) return null;
  const on = threshold !== null;
  const commit = (v: number | null) => {
    setThreshold(v);
    setRetrievalConfidenceThreshold(v).catch(() => {});
  };
  const toggle = () => {
    if (on) {
      commit(null); // switch the gate off
    } else {
      const n = Number(draft);
      commit(draft.trim() !== "" && Number.isFinite(n) ? n : DEFAULT_CONFIDENCE_THRESHOLD);
    }
  };
  return (
    <div className="flex items-center gap-2 font-mono text-[0.625rem] text-ink4">
      <span className="uppercase tracking-wide">confidence gate</span>
      <button
        type="button"
        onClick={toggle}
        aria-pressed={on}
        className="rounded-[var(--radius-sm)] border border-border2 bg-surface px-1.5 py-0.5 text-ink2 transition-colors hover:text-ink motion-reduce:transition-none"
      >
        {on ? "on" : "off"}
      </button>
      <input
        type="number"
        step="0.5"
        value={on ? draft : ""}
        disabled={!on}
        placeholder={String(DEFAULT_CONFIDENCE_THRESHOLD)}
        aria-label="Confidence-gate threshold"
        onChange={(e) => {
          const raw = e.target.value;
          setDraft(raw);
          const n = Number(raw.trim());
          if (raw.trim() !== "" && Number.isFinite(n)) commit(n);
        }}
        className="w-14 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 text-ink2 disabled:opacity-40"
      />
      <span className="normal-case">
        {on
          ? `below ${threshold}, PM treats sources as weak and hedges`
          : "off — sources always trusted"}
      </span>
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
// `prompt` is stable too — a functional `prompts` update keeps existing turns' arrays by reference — and
// `showPrompt` is a plain bool, so the dev dropdown doesn't defeat the memo.
const MessageBlock = memo(function MessageBlock({
  message,
  prompt,
  confidence,
  provider,
  showProvenance,
  showPrompt,
  onOpenChatCitation,
  highlight,
  registerBlock,
  knownTags,
}: {
  message: Message;
  prompt?: PromptMessage[];
  confidence?: GroundingConfidence;
  provider?: "local" | "cloud";
  showProvenance?: boolean;
  showPrompt?: boolean;
  onOpenChatCitation?: (conversationId: number, turnId: number | null) => void;
  highlight?: boolean;
  registerBlock: (id: number, el: HTMLDivElement | null) => void;
  knownTags?: readonly string[];
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
        markdown={message.role === "assistant"}
        citationCount={showSources ? citations.length : 0}
        onCite={showSources ? jumpToSource : undefined}
        knownTags={message.role === "user" ? knownTags : undefined}
      />
      {showSources && (
        <Sources
          citations={citations}
          itemRefs={itemRefs}
          flash={flash}
          onOpenChatCitation={onOpenChatCitation}
          onCitationOpened={(documentId) => {
            void recordCitationClick(message.id, documentId).catch(() => {});
          }}
        />
      )}
      {showSources && <AnswerRatingControls messageId={message.id} />}
      {showPrompt && message.role === "assistant" && prompt && prompt.length > 0 && (
        <PromptPanel messages={prompt} />
      )}
      {showPrompt && message.role === "assistant" && confidence && (
        <ConfidenceReadout confidence={confidence} />
      )}
      {message.role === "assistant" && atLeast("standard") && message.content.trim() !== "" && (
        <SaveAsNoteButton message={message} />
      )}
      {showProvenance && message.role === "assistant" && atLeast("standard") && message.model && (
        <p className="px-1 text-[0.625rem] text-faint" data-help="chat-provenance">
          via {shortModel(message.model)}
          {provider ? ` · ${provider}` : ""}
        </p>
      )}
    </div>
  );
});

export function ChatView({
  messages,
  streaming,
  prompts,
  confidences,
  providers,
  showProvenance,
  onOpenChatCitation,
  focusTurn,
}: Props) {
  const endRef = useRef<HTMLDivElement>(null);
  const { atLeast } = useDepth();
  const { devMode } = useDevMode();
  // Per-turn refs so a chat citation that navigated here can scroll straight to its turn.
  const blockRefs = useRef<Map<number, HTMLDivElement>>(new Map());
  const [flashMsg, setFlashMsg] = useState<number | null>(null);
  // The nonce of the focus request we've already handled, so a later message arriving (a reply
  // streaming in) doesn't yank the scroll back up to the old cited turn. Keyed on the nonce, not the
  // turn id, so re-clicking the *same* citation (fresh nonce) re-fires while streaming replies don't.
  const lastNonceRef = useRef<number | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Tag names that exist, so a `@mention` in a past turn can be shown as having resolved (#276).
  // Fetched once per mount and only ever used for display — a chat whose tags fail to load renders
  // its messages exactly as it did before the feature.
  const [knownTags, setKnownTags] = useState<string[]>([]);
  useEffect(() => {
    let live = true;
    listTags()
      .then((t) => {
        if (live) setKnownTags(t.map((x) => x.name));
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

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
        {devMode && <ConfidenceThresholdControl />}
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
            prompt={prompts?.[m.id]}
            confidence={confidences?.[m.id]}
            provider={providers?.[m.id]}
            showProvenance={showProvenance}
            showPrompt={devMode}
            onOpenChatCitation={openCitation}
            highlight={flashMsg === m.id}
            registerBlock={registerBlock}
            knownTags={knownTags}
          />
        ))}
        {streaming !== null && <Bubble role="assistant" content={streaming} markdown />}
        <StreamAnnouncer streaming={streaming} />
        <div ref={endRef} />
      </div>
    </div>
  );
}
