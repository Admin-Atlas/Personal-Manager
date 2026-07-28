// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Type-ahead: let someone write their next message while PM is still answering the last one (#152).
//
// **This is sugar over a backend that will not tolerate it.** A turn-pair is one user message and its
// reply, and `chat::assert_user_turn_allowed` refuses a second consecutive user turn at the write
// layer — so stacked messages must never reach it. Everything here exists to keep that true while the
// UI pretends otherwise:
//
//   * messages leave the queue **one at a time, in order**, and the next only starts once the previous
//     exchange has been persisted (`send` resolves at that point);
//   * a failed send **stops the queue** and puts its message back at the head. Continuing would be the
//     one way to genuinely break alternation: if a turn failed after its user message was inserted but
//     before the reply, the next send hits the alternation guard and errors too. Stopping also means a
//     dead API key doesn't burn the whole queue against it, one message at a time;
//   * nothing is dropped and nothing is reordered — a stopped queue keeps everything, in sequence,
//     for the user to resume or delete.
//
// Draining is a promise chain guarded by a ref, deliberately NOT an effect watching a `busy` flag.
// An effect sees the state of the render it was scheduled in, so the moment two sends land in one
// tick it either fires twice (two user turns in flight — exactly what must never happen) or not at
// all. A single serialised loop cannot do either.
//
// Nothing is persisted. A queued message has not been said yet; surviving a restart as a message
// about to be sent into a conversation the user may have moved on from is worse than losing it.

import { useCallback, useRef, useState } from "react";

/** How many messages may wait BEHIND the one in flight. Small on purpose: the queue is for finishing
 *  a thought that arrived while PM was talking, not for scripting a conversation in advance — and
 *  every extra queued message is one more written against a reply the user has not read yet. */
export const QUEUE_LIMIT = 3;

export interface QueuedMessage {
  /** Stable per-message id so a chip can be removed without matching on text (two identical
   *  messages are perfectly legal). */
  id: number;
  text: string;
}

export interface SendQueue {
  /** The messages still waiting. The one in flight is NOT here — it is already on screen as the
   *  optimistic user bubble, and showing it twice would read as a duplicate. */
  queued: QueuedMessage[];
  /** A send failed, so draining paused with everything still queued. */
  stalled: boolean;
  /** Whether another message would exceed [`QUEUE_LIMIT`]. */
  full: boolean;
  /** Queue `text` and start (or continue) draining. Returns false when the queue is full, so the
   *  caller can keep the user's draft in the box rather than swallowing it. */
  enqueue: (text: string) => boolean;
  /** Drop one waiting message. */
  remove: (id: number) => void;
  /** Replace a waiting message's text. A no-op once it has gone out — a sent message cannot be
   *  un-said, and silently rewriting the next one instead would be worse than doing nothing. */
  edit: (id: number, text: string) => void;
  /** Pause dispatch while an editor is open, and release it when the editor closes.
   *
   *  Not optional. Without it, a reply landing mid-edit sends the message as it was BEFORE the edit
   *  and the row vanishes from under the cursor — the user's correction discarded and the version
   *  they were fixing sent anyway. Nothing else in the queue is time-critical enough to justify that. */
  hold: (on: boolean) => void;
  /** Try again after a failure, from the message that failed. */
  resume: () => void;
  /** Discard everything, without sending. Call when the conversation on screen changes — queued text
   *  was written for the chat the user was looking at, and delivering it to another is worse than
   *  losing it. */
  clear: () => void;
}

/**
 * Serialise sends so the UI can accept them faster than the backend will take them.
 *
 * `send` resolves `true` when the exchange completed and `false` when it did not; it must never
 * reject (both chat views already swallow their own errors into visible state).
 */
export function useSendQueue(send: (text: string) => Promise<boolean>): SendQueue {
  const [queued, setQueued] = useState<QueuedMessage[]>([]);
  const [stalled, setStalled] = useState(false);

  // The queue itself lives in a ref, with `queued` as its render mirror. The drain loop reads it
  // between awaits, and state read through a closure would be whatever it was when the loop started.
  const pending = useRef<QueuedMessage[]>([]);
  const draining = useRef(false);
  // Dispatch is paused while an editor is open. A ref, not state: the drain loop tests it between
  // awaits, and a value captured when the loop started would be stale by exactly the moment it matters.
  const held = useRef(false);
  const nextId = useRef(1);
  const sendRef = useRef(send);
  sendRef.current = send;

  const publish = useCallback(() => setQueued(pending.current), []);

  const drain = useCallback(async () => {
    if (draining.current) return; // one loop, always — see the note on effects above
    draining.current = true;
    try {
      while (pending.current.length > 0 && !held.current) {
        const [head, ...rest] = pending.current;
        // Removed BEFORE sending: from here on it is in flight, and the chips must show only what is
        // still waiting.
        pending.current = rest;
        publish();
        const ok = await sendRef.current(head.text);
        if (!ok) {
          // Back at the head, so order survives a failure and the user resumes from where it broke.
          pending.current = [head, ...pending.current];
          publish();
          setStalled(true);
          return;
        }
      }
    } finally {
      draining.current = false;
    }
  }, [publish]);

  const enqueue = useCallback(
    (text: string) => {
      if (pending.current.length >= QUEUE_LIMIT) return false;
      pending.current = [...pending.current, { id: nextId.current++, text }];
      publish();
      setStalled(false);
      void drain();
      return true;
    },
    [drain, publish],
  );

  const remove = useCallback(
    (id: number) => {
      pending.current = pending.current.filter((m) => m.id !== id);
      publish();
      // Removing the message that failed clears the stall: there is nothing left blocking the queue,
      // and leaving the warning up would describe a state that no longer exists.
      if (pending.current.length === 0) setStalled(false);
    },
    [publish],
  );

  const edit = useCallback(
    (id: number, text: string) => {
      const trimmed = text.trim();
      // An empty edit is a deletion by another name; treat it as one rather than queueing a blank
      // message the backend would reject.
      pending.current = trimmed
        ? pending.current.map((m) => (m.id === id ? { ...m, text: trimmed } : m))
        : pending.current.filter((m) => m.id !== id);
      publish();
      if (pending.current.length === 0) setStalled(false);
    },
    [publish],
  );

  const hold = useCallback(
    (on: boolean) => {
      held.current = on;
      // Releasing restarts the loop, which the hold may have exited. Not on stall: a stalled queue
      // waits for an explicit retry, and closing an editor is not one.
      if (!on && !stalled) void drain();
    },
    [drain, stalled],
  );

  const resume = useCallback(() => {
    setStalled(false);
    void drain();
  }, [drain]);

  const clear = useCallback(() => {
    pending.current = [];
    publish();
    setStalled(false);
    // A hold belongs to an editor that is going away with the conversation; leaving it set would
    // freeze the next chat's queue with nothing on screen to explain why.
    held.current = false;
  }, [publish]);

  return {
    queued,
    stalled,
    full: queued.length >= QUEUE_LIMIT,
    enqueue,
    remove,
    edit,
    hold,
    resume,
    clear,
  };
}
