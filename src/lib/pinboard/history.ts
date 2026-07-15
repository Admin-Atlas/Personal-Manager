// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Undo/redo for the Pinboard, as a pure value — no React, no timers, no clock (the caller passes
 *  `now`), so every rule below is unit-testable in isolation (in the spirit of grid.ts and the
 *  backend's pure cores).
 *
 *  The board is one immutable value behind ONE writer (usePinboard's setBoard), so history here is
 *  simply a stack of past boards rather than a command log — nothing needs an inverse. Three rules
 *  earn their keep:
 *
 *  1. COALESCE BY BUCKET AGE, not by idle gap. Every keystroke is a board change, so typing has to
 *     be grouped or Ctrl+Z would delete one character at a time. The obvious rule — "merge while the
 *     gap since the last keystroke is short" — collapses a whole paragraph typed without pausing into
 *     ONE entry, so a single undo wipes it. Instead a bucket has a fixed LIFETIME: entries merge into
 *     it until it is `bucketMs` old, then the next change starts a new one. That makes Ctrl+Z "undo
 *     the last few seconds", regardless of how long you have been typing.
 *
 *  2. BUDGET IN BYTES, not entries. Snapshots share structure for everything untouched, but a note's
 *     text is a fresh string every keystroke, so a long note is copied in full into each entry. A
 *     flat "keep 50" cap is a memory leak measured in the size of the note being edited; the cap here
 *     is the retained text, which is the thing that actually grows.
 *
 *  3. SOME CHANGES ARE NOT UNDOABLE AT ALL. A commit can be `silent` (change the present, record
 *     nothing — z-order, and anything mirroring state that lives outside the board) or a `barrier`
 *     (change the present and DROP the history, for a change the board cannot honestly reverse
 *     because it also wrote to the backend). See usePinboard for which is which.
 */

export interface History<T> {
  past: Entry<T>[];
  present: T;
  future: T[];
}

interface Entry<T> {
  /** The board as it was BEFORE the change this entry records. */
  value: T;
  /** What kind of change opened this bucket (e.g. `text:<id>`); null never merges. */
  key: string | null;
  /** When the bucket was opened — its age, not its last touch, decides when it closes. */
  openedAt: number;
}

export interface CommitOptions<T> {
  /** Group consecutive same-key changes into one undo step for `bucketMs` (typing, mostly). */
  key?: string | null;
  now: number;
  /** How long a bucket accepts merges. */
  bucketMs?: number;
  /** Total characters of `text` to retain across `past` before the oldest entries are dropped. */
  budget?: number;
  /** Sum the undoable weight of a board — the caller knows its shape. Default: no weight, so the
   *  byte budget never bites and only `maxEntries` applies. */
  weigh?: (value: T) => number;
  /** A hard floor on entries, so a single enormous note can't leave the stack empty. */
  maxEntries?: number;
}

export const BUCKET_MS = 3000;
/** ~2M characters of retained note text across the whole stack — generous for real boards, but a cap
 *  a pasted novel can actually reach on a low-RAM box. */
export const TEXT_BUDGET = 2_000_000;
export const MAX_ENTRIES = 200;

export function initHistory<T>(present: T): History<T> {
  return { past: [], present, future: [] };
}

/** Replace everything — the board arrived from the store, so there is nothing to go back to. Never
 *  `commit` a load: the first Ctrl+Z would restore whatever the board was before it, which on a fresh
 *  install is the empty default. */
export function resetHistory<T>(present: T): History<T> {
  return initHistory(present);
}

/** Record a change. Returns the same history object when `next` is the present, so a mutator that
 *  decided to do nothing (grid's unchanged-landing, raiseWidget's already-on-top) costs no entry. */
export function commit<T>(h: History<T>, next: T, opts: CommitOptions<T>): History<T> {
  if (Object.is(next, h.present)) return h;

  const { key = null, now, bucketMs = BUCKET_MS } = opts;
  const top = h.past[h.past.length - 1];
  // Merge into the open bucket: same kind of change, and the bucket is still young. `key: null` is
  // "never merge" on both sides, so a delete between two keystrokes can't be swallowed by them.
  const merge = top != null && key != null && top.key === key && now - top.openedAt < bucketMs;

  const past = merge
    ? h.past // the bucket already holds the value from BEFORE the run of changes — keep it
    : [...h.past, { value: h.present, key, openedAt: now }];

  return { past: trim(past, opts), present: next, future: [] };
}

/** Change the present without recording anything: the change isn't the user's to undo (z-order), or
 *  it mirrors state that lives outside the board and so can't be rolled back with it. */
export function commitSilent<T>(h: History<T>, next: T): History<T> {
  if (Object.is(next, h.present)) return h;
  return { ...h, present: next };
}

/** Change the present and DROP the history: this change reached past the board (it wrote to the
 *  backend), so nothing before it can be honestly restored. Better a plain "you can't undo that" than
 *  an undo that half-works. */
export function commitBarrier<T>(_h: History<T>, next: T): History<T> {
  return initHistory(next);
}

export function canUndo<T>(h: History<T>): boolean {
  return h.past.length > 0;
}
export function canRedo<T>(h: History<T>): boolean {
  return h.future.length > 0;
}

export function undo<T>(h: History<T>): History<T> {
  const top = h.past[h.past.length - 1];
  if (!top) return h;
  return { past: h.past.slice(0, -1), present: top.value, future: [h.present, ...h.future] };
}

export function redo<T>(h: History<T>): History<T> {
  const [next, ...rest] = h.future;
  if (next === undefined) return h;
  // Redone steps never re-merge: `key: null` closes the bucket, so undo walks back one step at a
  // time through exactly what redo replayed.
  return {
    past: [...h.past, { value: h.present, key: null, openedAt: 0 }],
    present: next,
    future: rest,
  };
}

/** Drop the OLDEST entries until the stack is within budget — the far end of the stack is the part
 *  the user is least likely to reach for. `maxEntries` is a hard count; the byte budget is what
 *  actually bites on a board with a big note in it. */
function trim<T>(past: Entry<T>[], opts: CommitOptions<T>): Entry<T>[] {
  const { weigh, budget = TEXT_BUDGET, maxEntries = MAX_ENTRIES } = opts;
  let out = past.length > maxEntries ? past.slice(past.length - maxEntries) : past;
  if (!weigh) return out;
  let total = out.reduce((n, e) => n + weigh(e.value), 0);
  // Always keep one entry: a single note over budget should still be undoable once.
  while (out.length > 1 && total > budget) {
    total -= weigh(out[0].value);
    out = out.slice(1);
  }
  return out;
}
