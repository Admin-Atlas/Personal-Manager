// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The model's thinking, in a fold above its reply (the chat Thinking toggle). Two faces: the LIVE
// block while a reply streams — open while the model thinks, folding by itself once the answer
// starts — and the settled block under a finished turn, folded until opened. The two are separate
// mounts, so the user's own click and scroll on the live fold travel with the thought (`ThoughtFold`,
// kept by `useChatStream`) and the settled fold carries on from them. The header carries the timing
// at every depth ("Thinking… 12 s", then "Thought for 48 s"), because how long the model took is the
// point of showing it.
//
// The body is model output, so it is UNTRUSTED and goes in as a plain React text node: never
// `<Markdown>`, never `dangerouslySetInnerHTML`, never `linkCitations`. React's escaping is stricter
// than the sanitiser's allowlist, and thinking has no formatting worth the risk.
//
// Nothing here is ever stored: the text lives in `useChatStream`'s session map, keyed by message id.

import { useDeferredValue, useEffect, useLayoutEffect, useRef, useState } from "react";
import type { ChatThought, LiveThought, ThoughtFold } from "../lib/types";
import { useNowTick } from "../lib/useNowTick";
import { thinkingTitle } from "./thinkingWords";
import { Collapsible } from "./ui";

export const NO_ROOM_COPY =
  "Answered without thinking: this conversation is too long to leave your model room to think. Compress it, or give your model a longer context in Settings › Local AI.";

/** How close to its own bottom the body has to be to count as following the thought. */
const PIN_SLACK_PX = 48;

function ThinkingFold({
  text,
  title,
  autoOpen,
  live,
  fold,
  onFold,
}: {
  text: string;
  title: string;
  autoOpen: boolean;
  live: boolean;
  /** Where the user left this thought's fold on an earlier mount. Read once, on mount. */
  fold?: ThoughtFold;
  /** Reports where the user leaves this fold: a click on its header, a scroll in its box. */
  onFold?: (fold: ThoughtFold) => void;
}) {
  const [seed] = useState(fold);
  // `null` until the user clicks the header; from then on their choice wins over the automatic fold,
  // so opening it to read mid-answer never snaps shut under them — not even as the turn settles and
  // the thought moves to a new mount, which starts from the choice carried in `fold`.
  const [chosen, setChosen] = useState<boolean | null>(seed?.open ?? null);
  const open = chosen ?? autoOpen;
  // Where the box starts: back where the user left it when this mount opens on a fold they were in
  // (`null` = at its end). `undefined` = the top, so a fold opened fresh reads from the start.
  const [startAt] = useState(() => (seed && open ? seed.scrollTop : undefined));
  // A streaming thought sets `text` on every chunk; deferring keeps the token feed and scrolling
  // smooth, the same reason the reply bubble defers its Markdown.
  const deferredText = useDeferredValue(text);
  const boxRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(startAt == null);
  // The box's place as `ThoughtFold.scrollTop` words it, for the report a header click makes.
  const placeRef = useRef<number | null>(startAt ?? null);

  // Before paint, so a fold that carries on from an earlier mount never flashes its top first.
  useLayoutEffect(() => {
    const el = boxRef.current;
    if (el && startAt !== undefined) el.scrollTop = startAt ?? el.scrollHeight;
  }, [startAt]);

  // While live, keep the box at its own bottom unless the user has scrolled up in it. `scrollTop` on
  // the box itself, never `scrollIntoView`: that scrolls every ancestor, the document included.
  useEffect(() => {
    if (!live || !pinnedRef.current) return;
    const el = boxRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [deferredText, live]);

  return (
    <div className="flex justify-start" data-help="chat-thinking">
      <div className="w-full max-w-[80%]">
        <Collapsible
          open={open}
          onOpenChange={(next) => {
            setChosen(next);
            onFold?.({ open: next, scrollTop: placeRef.current });
          }}
          title={<span className="text-xs text-ink3">{title}</span>}
        >
          <div
            ref={boxRef}
            // Focusable so a keyboard can scroll it: WebKit (macOS, Linux) never makes a scrolling
            // box a tab stop by itself.
            tabIndex={open ? 0 : -1}
            role="region"
            aria-label="The model's thinking"
            onScroll={() => {
              const el = boxRef.current;
              if (!el) return;
              pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < PIN_SLACK_PX;
              placeRef.current = pinnedRef.current ? null : el.scrollTop;
              onFold?.({ open: chosen, scrollTop: placeRef.current });
            }}
            className="mt-1 max-h-60 overflow-y-auto"
          >
            <p className="whitespace-pre-wrap break-words border-l-2 border-rule pl-3 text-xs leading-snug text-ink3">
              {deferredText}
            </p>
          </div>
        </Collapsible>
      </div>
    </div>
  );
}

/** A finished turn's thinking: folded, timed — or, for a turn PM answered without thinking because
 *  the conversation left no room, the one line that says so. When the user had opened the live fold,
 *  it stays open instead, at the place they left it (`fold`). */
export function ThinkingBlock({
  text,
  seconds,
  skipped,
  fold,
  onFold,
}: ChatThought & { onFold?: (fold: ThoughtFold) => void }) {
  if (skipped && !text) {
    return (
      <div className="flex justify-start" data-help="chat-thinking">
        <p className="max-w-[80%] px-1 text-xs text-ink4">{NO_ROOM_COPY}</p>
      </div>
    );
  }
  return (
    <ThinkingFold
      text={text}
      title={thinkingTitle({ live: false, answered: true, seconds })}
      autoOpen={false}
      live={false}
      fold={fold}
      onFold={onFold}
    />
  );
}

/** The thinking while a reply streams: open and counting while the model thinks, folded under
 *  "Thought for N s" once the answer starts. This IS the progress indicator while no answer has
 *  arrived, so ChatView draws no empty reply bubble beside it. `onFold` hands the user's click and
 *  scroll to `useChatStream`, which carries them to the settled fold. */
export function LiveThinkingBlock({
  thought,
  onFold,
}: {
  thought: LiveThought;
  onFold?: (fold: ThoughtFold) => void;
}) {
  const now = useNowTick(1000);
  const answered = thought.answeredAt !== null;
  const seconds = Math.max(
    0,
    Math.floor(((thought.answeredAt ?? now.getTime()) - thought.startedAt) / 1000),
  );
  return (
    <ThinkingFold
      text={thought.text}
      title={thinkingTitle({ live: true, answered, seconds })}
      autoOpen={!answered}
      live
      fold={thought.fold}
      onFold={onFold}
    />
  );
}
