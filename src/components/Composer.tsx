// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useRecorder } from "../lib/useRecorder";
import { listTags } from "../lib/ipc";
import { completeMention, matchTags, mentionAtCaret } from "../lib/mentions";
import type { TagSummary } from "../lib/types";
import { MentionSuggest } from "./MentionSuggest";
import { Button, Textarea } from "./ui";

// The mic·input·send cluster is capped to exactly the conversation column width (ChatView's
// max-w-3xl = 48rem) and centered by the 1fr gutters, so it sits directly under the message
// column; the Context/Explain tools live in those gutters, hugging the cluster from outside.
const COMPOSER_GRID = "grid grid-cols-[1fr_minmax(0,48rem)_1fr] gap-2";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void;
  /** Compact chat tool anchored at the FAR LEFT of the row (the context meter) — it and the
   *  matching {@link rightTools} bracket the mic·input·send cluster so the input stays visually
   *  centered. Renders its own popover; absent tools simply render nothing. */
  leftTools?: ReactNode;
  /** Compact chat tool anchored at the FAR RIGHT of the row (retrieval-explain). See {@link leftTools}. */
  rightTools?: ReactNode;
}

export function Composer({ disabled, onSend, leftTools, rightTools }: Props) {
  const [text, setText] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  // The tallest the input may grow before it scrolls internally: 40% of the chat pane, so a long
  // draft never buries most of the history. Measured from the enclosing chat <main> (the composer
  // root's parent) so it adapts to the global chat and the narrower per-project chat alike.
  const [maxHeight, setMaxHeight] = useState<number>();

  // `@tag` (#276). Typing `@` offers the tags that exist; picking one inserts it. The pin itself is
  // read server-side from the sent message, so this list is discovery, never the mechanism — a
  // typed mention works with the panel closed, and a failure to load tags costs nothing.
  const [tags, setTags] = useState<TagSummary[]>([]);
  const [mentionQuery, setMentionQuery] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const listboxId = useId();
  const optionId = (i: number) => `${listboxId}-opt-${i}`;
  const suggestions = mentionQuery === null ? [] : matchTags(tags, mentionQuery);
  const open = suggestions.length > 0;

  // Loaded once per mount rather than per keystroke: the registry is small, and a fetch on every
  // `@` would put a round-trip between the keypress and the list.
  useEffect(() => {
    let live = true;
    listTags()
      .then((t) => {
        if (live) setTags(t);
      })
      .catch(() => {
        /* discovery sugar — a chat that cannot list tags still sends fine */
      });
    return () => {
      live = false;
    };
  }, []);

  // Re-read the token under the caret after any change to the text or the caret position.
  const syncMention = useCallback((value: string, caret: number | null) => {
    const at = caret === null ? null : mentionAtCaret(value, caret);
    setMentionQuery(at ? at.query : null);
    setActive(0);
  }, []);

  function pick(name: string) {
    const el = textareaRef.current;
    if (!el) return;
    const at = mentionAtCaret(text, el.selectionStart ?? text.length);
    if (!at) return;
    const next = completeMention(text, at, name);
    setText(next.text);
    setMentionQuery(null);
    // The caret has to be restored after React has painted the new value, or it lands at the end.
    requestAnimationFrame(() => {
      el.focus();
      el.setSelectionRange(next.caret, next.caret);
    });
  }

  // Voice input (spec §4 P1): a dictated clip is transcribed on-device and
  // appended to the box for the user to review/edit — never auto-sent.
  const recorder = useRecorder((spoken) => {
    setText((current) => (current.trim() ? `${current.trim()} ${spoken}` : spoken));
    textareaRef.current?.focus();
  });

  // Grow the textarea to fit its content, capped at `cap`px; past that it scrolls inside. Reset to
  // auto first so it can also shrink as the draft gets shorter (and back to one row on send).
  const autosize = useCallback((cap: number) => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, cap)}px`;
    el.style.overflowY = el.scrollHeight > cap ? "auto" : "hidden";
  }, []);

  useEffect(() => {
    const main = rootRef.current?.parentElement;
    if (!main) return;
    // 40% of the chat pane, remeasured on any resize. Re-grow directly here too: a WIDTH-only change
    // rewraps the draft (changing scrollHeight) without changing clientHeight, so maxHeight — and the
    // effect below — wouldn't fire, and the box would clip until the next keystroke.
    const update = () => {
      const cap = Math.round(main.clientHeight * 0.4);
      setMaxHeight(cap);
      autosize(cap);
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(main);
    return () => ro.disconnect();
  }, [autosize]);

  // Re-grow on every content change (and when the cap first lands), shrinking back to one row on send.
  useLayoutEffect(() => {
    autosize(maxHeight ?? Infinity);
  }, [text, maxHeight, autosize]);

  function submit() {
    const trimmed = text.trim();
    if (!trimmed || disabled) return;
    onSend(trimmed);
    setText("");
    setMentionQuery(null);
  }

  function toggleMic() {
    if (recorder.state === "recording") recorder.stop();
    else if (recorder.state === "idle") void recorder.start();
  }

  const recording = recorder.state === "recording";
  const transcribing = recorder.state === "transcribing";
  const micTitle = recording
    ? "Stop and transcribe"
    : transcribing
      ? "Transcribing…"
      : "Record voice (transcribed on your device)";

  return (
    <div ref={rootRef} className="border-t border-border p-4">
      <div className={`${COMPOSER_GRID} items-end`} data-help="chat-composer">
        {/* Left gutter — Context meter hangs just outside the conversation width, hugging the cluster.
            min-w-0 lets both gutters take an equal fr share (a tool overflows outward rather than
            flooring its track wider than the empty side), so the cluster stays centered under the column. */}
        <div className="flex min-w-0 items-center justify-end">{leftTools}</div>

        {/* Center — mic·input·send, exactly the conversation column width and centered under it.
            `relative` anchors the `@` suggestion panel to this cluster. */}
        <div className="relative flex items-end gap-2">
          <MentionSuggest
            items={suggestions}
            active={active}
            listboxId={listboxId}
            optionId={optionId}
            onPick={pick}
            onHover={setActive}
          />
          <button
            type="button"
            onClick={toggleMic}
            disabled={transcribing}
            title={micTitle}
            aria-label={micTitle}
            data-help="composer-mic"
            className={`flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--radius-sm)] border text-sm transition-colors disabled:cursor-not-allowed disabled:opacity-60 ${
              recording ? "animate-pulse" : "border-border2 text-ink3 hover:text-ink2"
            }`}
            style={
              recording
                ? {
                    color: "var(--st-due)",
                    background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                  }
                : undefined
            }
          >
            {transcribing ? <SpinnerIcon /> : recording ? <StopIcon /> : <MicIcon />}
          </button>

          <Textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              syncMention(e.target.value, e.target.selectionStart);
              // Resize in the same event the value changes so the box grows AND shrinks immediately —
              // including when text is deleted or cleared, not just on the next layout pass.
              autosize(maxHeight ?? Infinity);
            }}
            // Arrow keys and clicks move the caret without changing the value, so the suggestion
            // list has to follow them too — otherwise it would keep offering completions for a
            // token the caret has already left.
            onKeyUp={(e) => syncMention(e.currentTarget.value, e.currentTarget.selectionStart)}
            onClick={(e) => syncMention(e.currentTarget.value, e.currentTarget.selectionStart)}
            onBlur={() => setMentionQuery(null)}
            onKeyDown={(e) => {
              // While the list is open it owns the arrow keys, Enter, Tab and Escape. Enter is the
              // one that matters: it must complete the mention rather than send a half-typed one.
              if (open) {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setActive((i) => Math.min(i + 1, suggestions.length - 1));
                  return;
                }
                if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setActive((i) => Math.max(i - 1, 0));
                  return;
                }
                if (e.key === "Enter" || e.key === "Tab") {
                  e.preventDefault();
                  pick(suggestions[active].name);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setMentionQuery(null);
                  return;
                }
              }
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                submit();
              }
            }}
            role="combobox"
            aria-expanded={open}
            aria-controls={open ? listboxId : undefined}
            aria-autocomplete="list"
            aria-activedescendant={open ? optionId(active) : undefined}
            rows={1}
            placeholder="Ask anything…  (Enter to send, @ to pin a tag)"
            className="flex-1 px-4 py-2"
          />
          <Button
            variant="primary"
            onClick={submit}
            disabled={disabled || !text.trim()}
            className="px-4 py-2"
          >
            Send
          </Button>
        </div>

        {/* Right gutter — Explain tool, hugging the cluster's right edge (see left gutter re min-w-0). */}
        <div className="flex min-w-0 items-center justify-start">{rightTools}</div>
      </div>

      {(recording || transcribing || recorder.error) && (
        <div className={`mt-2 ${COMPOSER_GRID}`}>
          <div />
          <div role="status" aria-live="polite">
            {recording && (
              <p className="text-xs" style={{ color: "var(--st-due)" }}>
                Recording… click the mic to stop.
              </p>
            )}
            {transcribing && (
              <p className="text-xs text-ink4">
                Transcribing on your device… the first time also downloads the voice model.
              </p>
            )}
            {recorder.error && (
              <p className="text-xs" style={{ color: "var(--st-due)" }}>
                {recorder.error}
              </p>
            )}
          </div>
          <div />
        </div>
      )}
    </div>
  );
}

function MicIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-5 w-5" fill="none" stroke="currentColor" strokeWidth={2}>
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0" strokeLinecap="round" />
      <line x1="12" y1="18" x2="12" y2="21" strokeLinecap="round" />
    </svg>
  );
}

function StopIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-4 w-4" fill="currentColor">
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </svg>
  );
}

function SpinnerIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-5 w-5 animate-spin"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
    >
      <path d="M12 3a9 9 0 1 0 9 9" strokeLinecap="round" />
    </svg>
  );
}
