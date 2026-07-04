// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useRef, useState, type ReactNode } from "react";
import { useRecorder } from "../lib/useRecorder";
import { Button, Textarea } from "./ui";

interface Props {
  disabled: boolean;
  onSend: (text: string) => void;
  /** Compact chat tools shown on the input row itself (the context meter + retrieval-explain
   *  triggers) — space-efficient alternative to full-width bars stacked above the composer.
   *  Each renders its own popover; absent tools simply render nothing. */
  tools?: ReactNode;
}

export function Composer({ disabled, onSend, tools }: Props) {
  const [text, setText] = useState("");
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Voice input (spec §4 P1): a dictated clip is transcribed on-device and
  // appended to the box for the user to review/edit — never auto-sent.
  const recorder = useRecorder((spoken) => {
    setText((current) => (current.trim() ? `${current.trim()} ${spoken}` : spoken));
    textareaRef.current?.focus();
  });

  function submit() {
    const trimmed = text.trim();
    if (!trimmed || disabled) return;
    onSend(trimmed);
    setText("");
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
    <div className="border-t border-border p-4">
      <div className="mx-auto max-w-3xl" data-help="chat-composer">
        <div className="flex items-end gap-2">
          {tools && <div className="flex shrink-0 items-center gap-1.5">{tools}</div>}
          <button
            type="button"
            onClick={toggleMic}
            disabled={transcribing}
            title={micTitle}
            aria-label={micTitle}
            data-help="composer-mic"
            className={`flex h-[46px] w-[46px] shrink-0 items-center justify-center rounded-[var(--radius-sm)] border text-sm transition-colors disabled:cursor-not-allowed disabled:opacity-60 ${
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
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                submit();
              }
            }}
            rows={1}
            placeholder="Ask anything…  (Enter to send, Shift+Enter for a new line)"
            className="max-h-40 flex-1 px-4 py-3"
          />
          <Button
            variant="primary"
            onClick={submit}
            disabled={disabled || !text.trim()}
            className="px-4 py-3"
          >
            Send
          </Button>
        </div>

        {recording && (
          <p className="mt-2 text-xs" style={{ color: "var(--st-due)" }}>
            Recording… click the mic to stop.
          </p>
        )}
        {transcribing && (
          <p className="mt-2 text-xs text-ink4">
            Transcribing on your device… the first time also downloads the voice model.
          </p>
        )}
        {recorder.error && (
          <p className="mt-2 text-xs" style={{ color: "var(--st-due)" }}>
            {recorder.error}
          </p>
        )}
      </div>
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
