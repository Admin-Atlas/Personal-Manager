// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What's waiting to be sent, above the composer (#152).
//
// A queued message is one the user has committed to but PM has not said yet, and the gap between
// those two states is the whole reason this exists. So it shows the message in full rather than a
// count, keeps them in the order they'll go, and lets any of them be changed or taken back — the
// thought that arrived while PM was mid-answer is often half-right, or obsolete, by the time PM
// finishes answering.
//
// Editing PAUSES dispatch (`hold`). Without that, a reply landing while someone is fixing a typo
// sends the message as it was before the fix and the row disappears from under the cursor — the
// correction discarded and the version being corrected sent anyway.
//
// When the queue stops after a failure it says so plainly and offers to try again, rather than
// silently holding messages that look sent.

import { useEffect, useRef, useState } from "react";

import type { QueuedMessage } from "../lib/useSendQueue";
import { Button } from "./ui";

interface Props {
  queued: QueuedMessage[];
  stalled: boolean;
  onRemove: (id: number) => void;
  onEdit: (id: number, text: string) => void;
  onHold: (on: boolean) => void;
  onResume: () => void;
}

export function QueuedMessages({ queued, stalled, onRemove, onEdit, onHold, onResume }: Props) {
  const [editingId, setEditingId] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // The hold is released on unmount too: leaving the chat with an editor open would otherwise freeze
  // the next conversation's queue with nothing on screen to explain it.
  useEffect(() => () => onHold(false), [onHold]);

  // A held message can still be removed from under the editor (or the queue cleared on a chat
  // switch). If the row being edited is gone, close the editor and release rather than holding
  // dispatch for something that no longer exists.
  useEffect(() => {
    if (editingId !== null && !queued.some((m) => m.id === editingId)) {
      setEditingId(null);
      onHold(false);
    }
  }, [queued, editingId, onHold]);

  function beginEdit(m: QueuedMessage) {
    setEditingId(m.id);
    setDraft(m.text);
    onHold(true);
  }

  function commit() {
    if (editingId === null) return;
    onEdit(editingId, draft);
    setEditingId(null);
    onHold(false);
  }

  function cancel() {
    setEditingId(null);
    onHold(false);
  }

  if (queued.length === 0) return null;
  return (
    <div className="mx-auto w-full max-w-3xl shrink-0 px-2 pb-1" data-help="chat-queue">
      {/* Polite, not assertive: these appear while a reply is streaming, and an assertive region
          would interrupt the reply being read out. */}
      <ul aria-live="polite" className="flex flex-col gap-1">
        {queued.map((m) => (
          <li
            key={m.id}
            className="flex items-center gap-2 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1"
          >
            <span className="shrink-0 font-mono text-[10px] uppercase tracking-wide text-ink4">
              {editingId === m.id ? "editing" : "queued"}
            </span>
            {editingId === m.id ? (
              <input
                ref={inputRef}
                autoFocus
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                // Blur commits rather than cancels: clicking away from an edit you just made and
                // finding it reverted is the more surprising of the two.
                onBlur={commit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    commit();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    cancel();
                  }
                }}
                aria-label="Edit this queued message"
                className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border2 bg-bg px-1.5 py-0.5 text-xs text-ink2"
              />
            ) : (
              <button
                type="button"
                onClick={() => beginEdit(m)}
                title="Edit before it sends"
                aria-label={`Edit "${m.text}"`}
                className="min-w-0 flex-1 truncate text-left text-xs text-ink3 hover:text-ink2"
              >
                {m.text}
              </button>
            )}
            <button
              type="button"
              onClick={() => onRemove(m.id)}
              aria-label={`Don't send "${m.text}"`}
              title="Take this one back"
              className="shrink-0 rounded-[var(--radius-sm)] px-1 text-xs text-ink4 transition-colors hover:text-ink2"
            >
              ×
            </button>
          </li>
        ))}
      </ul>
      {stalled && (
        <div className="mt-1 flex items-center gap-2">
          <p className="flex-1 text-xs text-[var(--st-due)]">
            That didn&rsquo;t send, so the rest are still waiting.
          </p>
          <Button variant="tertiary" onClick={onResume}>
            Try again
          </Button>
        </div>
      )}
    </div>
  );
}
