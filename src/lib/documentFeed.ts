// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Live document arrivals: the fan-out for `documents://landed`, which the backend emits once per
// newly-committed, unreviewed document during a sync or an import.
//
// Module-level rather than a React context because both consumers (the Documents list and the Review
// queue) unmount on tab switch, and arrivals keep landing while neither is mounted — a document that
// arrives while the user is in Chat must still be there when they open Review.
//
// Two things this owns that a bare `listen()` would not:
//
//   1. COALESCING. A sync can land documents faster than React can usefully re-render. Arrivals are
//      buffered and delivered as an array on a trailing timer, so a hundred files cost a handful of
//      renders instead of a hundred.
//
//   2. A MONOTONIC SEQUENCE, which closes a lost-update race. Both views load by awaiting a query and
//      then replacing their state wholesale; a document committing between the query returning and
//      the state being set would be silently dropped until the next full refresh. A view captures
//      `landingSeq()` BEFORE its await and unions `landingsSince(seq)` into the result, so anything
//      that landed during the gap is recovered rather than lost.

import type { Document } from "./types";

/** How long to gather arrivals before delivering them. Long enough to batch a fast sync into a few
 *  renders, short enough that a single dropped file still feels immediate. */
const COALESCE_MS = 250;

/** Ceiling on the replay buffer. It exists only to heal the query-vs-setState gap, which is
 *  milliseconds — it is not a session log, and must not grow with a 10,000-file sync. */
const REPLAY_CAP = 200;

type Listener = (documents: Document[]) => void;

const listeners = new Set<Listener>();
/** Arrivals awaiting delivery on the next flush. */
let pending: Document[] = [];
let timer: ReturnType<typeof setTimeout> | null = null;

/** Monotonically increasing count of everything ever announced this session. */
let seq = 0;
/** The tail of recent arrivals, each tagged with the sequence value it was given. */
let replay: { seq: number; document: Document }[] = [];

/** The current arrival sequence. Capture this BEFORE an await, then pass it to `landingsSince`. */
export function landingSeq(): number {
  return seq;
}

/** Documents that landed after `since` — what a wholesale `setState` would otherwise have dropped.
 *  Returns them in arrival order. */
export function landingsSince(since: number): Document[] {
  return replay.filter((r) => r.seq > since).map((r) => r.document);
}

/** Subscribe to coalesced arrivals. Returns an unsubscribe. */
export function onDocumentsLanded(fn: Listener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

function flush() {
  timer = null;
  const batch = pending;
  pending = [];
  if (batch.length === 0) return;
  for (const fn of listeners) {
    // One listener throwing must not rob the others of the batch, or a render error in one view
    // silently stops the other view updating for the rest of the session.
    try {
      fn(batch);
    } catch {
      /* a subscriber's own problem */
    }
  }
}

/** Record an arrival and schedule its delivery. Called by the `documents://landed` subscription in
 *  ipc.ts; exported for tests. */
export function pushLanding(document: Document): void {
  seq += 1;
  replay.push({ seq, document });
  if (replay.length > REPLAY_CAP) replay = replay.slice(-REPLAY_CAP);
  pending.push(document);
  if (timer === null) timer = setTimeout(flush, COALESCE_MS);
}

/**
 * Drop all buffered state — for tests, and for the writer-baton curtain, where the backend closes
 * the store under a still-mounted webview (`vault://curtain`).
 *
 * NOT for a vault swap, which the old comment here claimed: every path that points PM at a different
 * store reloads the webview, so no buffered arrival can outlive one.
 *
 * This resets `seq` to 0, which is safe only because the curtain gate short-circuits ABOVE the whole
 * app tree — every view holding a captured `landingSeq()` is unmounted by then. Render the curtain
 * as an overlay instead of an early return and a captured seq would survive this reset, so
 * `landingsSince` would under-report: exactly the lost update the sequence exists to close.
 */
export function resetDocumentFeed(): void {
  if (timer !== null) clearTimeout(timer);
  timer = null;
  pending = [];
  replay = [];
  seq = 0;
}

/** Merge arrivals into an existing list, newest first, without duplicating a row already present.
 *  Pure, so the dedup and ordering are testable without a store. */
export function mergeLandings(existing: Document[], landed: Document[]): Document[] {
  const known = new Set(existing.map((d) => d.id));
  const fresh = landed.filter((d) => {
    if (known.has(d.id)) return false;
    known.add(d.id); // the same document can appear twice within one batch
    return true;
  });
  if (fresh.length === 0) return existing;
  return [...fresh.reverse(), ...existing];
}
