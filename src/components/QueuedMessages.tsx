// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What's waiting to be sent, above the composer (#152).
//
// A queued message is one the user has committed to but PM has not said yet, and the gap between
// those two states is the whole reason this exists. So it shows the message in full rather than a
// count, keeps them in the order they'll go, and lets any of them be taken back — the thought that
// arrived while PM was mid-answer is often obsolete by the time PM finishes answering.
//
// When the queue stops after a failure it says so plainly and offers to try again, rather than
// silently holding messages that look sent.

import type { QueuedMessage } from "../lib/useSendQueue";
import { Button } from "./ui";

interface Props {
  queued: QueuedMessage[];
  stalled: boolean;
  onRemove: (id: number) => void;
  onResume: () => void;
}

export function QueuedMessages({ queued, stalled, onRemove, onResume }: Props) {
  if (queued.length === 0) return null;
  return (
    <div className="mx-auto w-full max-w-3xl px-2 pb-1" data-help="chat-queue">
      {/* Polite, not assertive: these appear while a reply is streaming, and an assertive region
          would interrupt the reply being read out. */}
      <ul aria-live="polite" className="flex flex-col gap-1">
        {queued.map((m) => (
          <li
            key={m.id}
            className="flex items-center gap-2 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1"
          >
            <span className="shrink-0 font-mono text-[10px] uppercase tracking-wide text-ink4">
              queued
            </span>
            <span className="min-w-0 flex-1 truncate text-xs text-ink3" title={m.text}>
              {m.text}
            </span>
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
