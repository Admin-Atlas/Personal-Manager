// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared state for AI filing suggestions, lifted out of ReviewView so several things can drive them:
// the Review tab (an explicit user action), a document LANDING (the live path — see
// `proposeOnArrival`), and a connector sync finishing (the backstop sweep).
//
// Three pieces live here because both callers need all three:
//   * `proposalCache` — module-level, so leaving and returning to the Review tab never re-bills.
//   * a single-run guard — so a sync-triggered run and a Review-tab run can't overlap and pay twice
//     for the same documents.
//   * a subscription — so proposals produced in the background paint live in an already-open
//     Review tab instead of only after a reload.
//
// The DB is still the durable copy (`document_proposals`, #486); this cache is the in-session
// mirror that avoids a round-trip.

import { aiProviderStatus, cachedProposals, proposeMetadata, reviewQueue } from "./ipc";
import { readReviewAiEnabled } from "./reviewPrefs";
import type { Document, Importance, MetadataProposal } from "./types";

/** Proposals by document id, for the life of the session. Survives unmounting the Review tab. */
export const proposalCache = new Map<number, MetadataProposal>();

/** The three fields a Review row lets you change. Structural, so the view can keep its own alias. */
export interface ReviewEdit {
  project: string;
  tags: string[];
  importance: Importance;
}

/** The document values a row falls back to when nothing better is known. */
export interface ReviewEditSeed {
  project: string;
  tags: string[];
  importance: Importance;
}

/**
 * What a Review row's Project / Importance / Tags controls should show, in precedence order:
 *
 *   1. the user's own hand-edit, if they made one this session;
 *   2. the AI's proposal — freshly streamed, or restored from `document_proposals` after a restart;
 *   3. the document's stored values.
 *
 * Step 2 is the one that was missing (#486 shipped the DB round-trip but this seeding kept reading
 * only the hand-edit cache), and it caused two separately-reported bugs: a proposal produced while
 * the Review tab was closed — the post-sync background run — painted its reasoning over blank
 * fields, and a restart did the same to every restored proposal. Blank fields are not cosmetic
 * here: `decisionFor` commits what these controls hold while reporting the proposal as
 * `proposed_*`, so approving a blanked row files it to Unsorted and logs a fabricated correction
 * against the model.
 *
 * Pure and exported so the rule is pinned by tests rather than living inline in a 900-line view.
 */
export function seedReviewEdit(
  handEdit: ReviewEdit | undefined,
  proposal: MetadataProposal | undefined,
  doc: ReviewEditSeed,
): ReviewEdit {
  if (handEdit) return handEdit;
  // Copy the tag array rather than aliasing the cached proposal's: `decisionFor` compares the edit
  // against the proposal to decide what to log as a correction, so a caller that ever mutated tags
  // in place would move both sides at once and the correction would silently never be logged.
  // Today's TagEditor always rebuilds the array, so this is a guard, not a live fix.
  if (proposal) {
    return { project: proposal.project, tags: [...proposal.tags], importance: proposal.importance };
  }
  return { project: doc.project, tags: [...doc.tags], importance: doc.importance };
}

type ProposalListener = (documentId: number, proposal: MetadataProposal) => void;
const listeners = new Set<ProposalListener>();

/** Watch for proposals arriving from ANY run — the Review tab's own, or a background one after a
 *  sync. Returns an unsubscribe function. */
export function subscribeToProposals(fn: ProposalListener): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Record a proposal once, centrally: cache it, then tell whoever is listening. Every run funnels
 *  through here so the cache and an open Review tab can never disagree. */
export function publishProposal(documentId: number, proposal: MetadataProposal): void {
  proposalCache.set(documentId, proposal);
  for (const fn of listeners) fn(documentId, proposal);
}

/** Drop cached entries for documents that are no longer in the queue (committed, or removed
 *  elsewhere), so the cache can't grow without bound across a long session. */
export function pruneProposalCache(liveIds: Set<number>): void {
  for (const id of [...proposalCache.keys()]) if (!liveIds.has(id)) proposalCache.delete(id);
}

let current: Promise<void> | null = null;

/** The proposal run in flight, or a resolved promise when there is none. Await it to let one
 *  settle before starting work that depends on a clean slate (e.g. "Re-propose"). */
export function currentProposalRun(): Promise<void> {
  return current ?? Promise.resolve();
}

/** Start a proposal run — or QUEUE behind the one already in flight rather than starting a second.
 *  Not billing twice for the same documents is the whole point: the Review tab opening while a
 *  post-sync background run is still going must join, not duplicate. Both callers see results
 *  through {@link subscribeToProposals}.
 *
 *  A joiner's `fn` runs after the in-flight one settles, rather than being dropped. It used to be
 *  discarded outright — the joiner got the running promise and its own ids were never proposed at
 *  all, so two connectors finishing close together silently lost the second one's documents until
 *  something reloaded Review. Each `fn` re-derives what still needs proposing (the caches and the
 *  `reviewed` flag are re-read at that point), so a queued run over already-handled documents costs
 *  nothing.
 *
 *  Errors propagate to that run's own caller; a failed run must not stop the queued one starting,
 *  or one bad sync would wedge proposals for the session. */
export function withProposalRun(fn: () => Promise<void>): Promise<void> {
  // With nothing in flight, start synchronously — deferring even the first run by a microtask would
  // change when callers observe `proposing`. A joiner chains, swallowing the predecessor's failure
  // so a rejected run still lets the next one start.
  const run = current ? current.catch(() => {}).then(fn) : fn();
  // Sequencing handle only: errors are the caller's to surface via `run`, so this copy absorbs them
  // rather than surfacing as an unhandled rejection when nobody awaits `currentProposalRun()`.
  const tracked: Promise<void> = run
    .catch(() => {})
    .then(() => {
      if (current === tracked) current = null;
    });
  current = tracked;
  return run;
}

// --- the live path: propose as documents land -----------------------------------------------------

/**
 * How many arrivals one live proposal batch covers.
 *
 * Deliberately small. Proposing everything in one call at the end of a sync is cheaper per token —
 * one big call reuses its cached prompt prefix better — but it means a long sync fills the Review
 * queue with files that all say nothing until it finishes. Five keeps the suggestions visibly in
 * step with the files arriving, which is the point of showing them arriving at all. The
 * all-at-once mode is a usage-gated card, not the default.
 */
const ARRIVAL_BATCH_SIZE = 5;

/** How long a partial batch waits for company before going anyway — so the tail of a sync, or a
 *  single file dropped into a watched folder, doesn't sit unsuggested waiting for a fifth. */
const ARRIVAL_FLUSH_MS = 1500;

/** Landed document ids still owed a suggestion, oldest first. */
let arrivalQueue: number[] = [];
let arrivalTimer: ReturnType<typeof setTimeout> | null = null;
let draining = false;

function scheduleArrivalFlush(): void {
  if (arrivalTimer !== null) return;
  arrivalTimer = setTimeout(() => {
    arrivalTimer = null;
    void drainArrivals();
  }, ARRIVAL_FLUSH_MS);
}

/**
 * Note documents that just landed and propose for them, in small batches, as they arrive.
 *
 * Wired to `onDocumentsLanded` at app scope, so it covers EVERY way a document arrives — a Drive /
 * OneDrive / local-folder sync, the live filesystem watcher, and a drag-and-drop import — not just
 * the paths that end in a sync-finished event. Suggestions used to hang off that event alone, which
 * meant a file the watcher picked up, or one dropped in by hand, waited for the next sync to
 * complete (or for the Review tab to be opened) before it got one. Nothing about how a file reached
 * PM should change whether PM offers to file it.
 *
 * Gated on the same AI-suggestions switch as every other paid call, re-read on each arrival — so
 * turning suggestions off stops the spend at the next file, not at the end of the sync.
 */
export function proposeOnArrival(documents: Document[]): void {
  if (!readReviewAiEnabled()) return;
  for (const d of documents) {
    // The backend already withholds reviewed rows before emitting. Re-checked because this is a
    // spend gate: paying for a suggestion about a document the user has already filed is money for
    // an answer nobody asked for.
    if (d.reviewed || proposalCache.has(d.id) || arrivalQueue.includes(d.id)) continue;
    arrivalQueue.push(d.id);
  }
  if (arrivalQueue.length === 0) return;
  if (arrivalQueue.length >= ARRIVAL_BATCH_SIZE) {
    if (arrivalTimer !== null) {
      clearTimeout(arrivalTimer);
      arrivalTimer = null;
    }
    void drainArrivals();
  } else {
    scheduleArrivalFlush();
  }
}

/** Work the arrival queue down a batch at a time. One drainer at a time — documents landing while
 *  it runs extend the queue rather than starting a second, racing drain. */
async function drainArrivals(): Promise<void> {
  if (draining) return;
  draining = true;
  try {
    // Read once per drain rather than per batch. A burst of arrivals is one situation, and this
    // reads the key store — checking it every five files would mean dozens of keychain probes
    // across one sync for an answer that cannot meaningfully change mid-burst.
    const status = await aiProviderStatus();
    if (!status.has_cloud_key && !status.local_configured) {
      // No model linked, so stay quiet — and drop the queue rather than let it grow for the length
      // of a long sync. Nothing is stranded: both the post-sync sweep and the Review tab re-derive
      // what still needs proposing from the store once a model is linked.
      arrivalQueue = [];
      return;
    }
    while (arrivalQueue.length > 0) {
      const batch = arrivalQueue
        .splice(0, ARRIVAL_BATCH_SIZE)
        // An earlier batch, the post-sync sweep, or the Review tab's own run may have covered these
        // while this one waited its turn in `withProposalRun`.
        .filter((id) => !proposalCache.has(id));
      if (batch.length === 0) continue;
      try {
        await withProposalRun(async () => {
          await proposeMetadata((event) => {
            if (event.type === "proposed") publishProposal(event.document_id, event.proposal);
          }, batch);
        });
      } catch {
        // Give up on the whole drain at the first failure instead of replaying it batch after batch
        // against a model that is down — with a 200-file sync in flight that would be forty failed
        // round-trips. Clearing the queue is what stops the `finally` below turning this into a
        // retry loop; the post-sync sweep and the Review tab are the backstops that re-derive what
        // is still unproposed.
        arrivalQueue = [];
        break;
      }
    }
  } catch {
    // Background convenience — never surfaced. (A failing `aiProviderStatus` lands here.)
  } finally {
    draining = false;
    // Documents that landed as this drain was ending would otherwise sit with no drainer running
    // and no timer armed.
    if (arrivalQueue.length > 0) scheduleArrivalFlush();
  }
}

/** Drop the pending arrival state. For tests, and for a vault lock — arrivals from a previous vault
 *  must never be proposed into a different one. */
export function resetArrivalProposals(): void {
  if (arrivalTimer !== null) clearTimeout(arrivalTimer);
  arrivalTimer = null;
  arrivalQueue = [];
  draining = false;
}

/**
 * Propose for everything newly unreviewed, after a connector sync — the BACKSTOP to the live
 * arrival path above, not the main event any more.
 *
 * Still worth running: it re-derives from the store, so it catches whatever the live path missed —
 * documents already queued from a previous session, files that landed before a model was linked, and
 * any batch that failed mid-drain. It costs nothing when there is nothing to do (everything already
 * proposed is filtered out before the call, and it returns early on an empty set).
 *
 * Silent and best-effort by design: the user didn't ask for this at this moment, so a missing
 * model, no credits, or an unreachable endpoint produce no visible error at all. (The Review tab's
 * own run does surface a calm reason, because there the user pressed something.)
 *
 * Gated on the same AI-suggestions switch as everything else, so it can never spend tokens the
 * user hasn't opted into. Nothing is re-billed: documents that already have a proposal — in this
 * session's cache or persisted in the DB from a previous one — are excluded before the call.
 */
export async function runProposalsAfterSync(): Promise<void> {
  if (!readReviewAiEnabled()) return;
  try {
    await withProposalRun(async () => {
      const [queue, cached] = await Promise.all([reviewQueue(), cachedProposals()]);
      const liveIds = new Set(queue.map((d) => d.id));
      pruneProposalCache(liveIds);
      for (const { document_id, proposal } of cached) {
        if (liveIds.has(document_id) && !proposalCache.has(document_id)) {
          proposalCache.set(document_id, proposal);
        }
      }
      const missing = queue.filter((d) => !proposalCache.has(d.id)).map((d) => d.id);
      if (missing.length === 0) return;

      // No model linked yet → stay quiet. Suggestions simply wait until one is.
      const status = await aiProviderStatus();
      if (!status.has_cloud_key && !status.local_configured) return;

      await proposeMetadata((event) => {
        if (event.type === "proposed") publishProposal(event.document_id, event.proposal);
      }, missing);
    });
  } catch {
    // Background convenience: never surface. The Review tab will propose on demand as before.
  }
}
