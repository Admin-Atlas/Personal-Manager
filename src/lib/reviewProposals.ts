// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared state for AI filing suggestions, lifted out of ReviewView so two things can drive them:
// the Review tab (an explicit user action) and a connector sync finishing (background convenience).
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
import type { MetadataProposal } from "./types";

/** Proposals by document id, for the life of the session. Survives unmounting the Review tab. */
export const proposalCache = new Map<number, MetadataProposal>();

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

/** Start a proposal run — or JOIN the one already in flight rather than starting a second. That
 *  join is the whole point: the Review tab opening while a post-sync background run is still going
 *  must not bill a second time for the same documents. Both callers then await the same work, and
 *  both see its results through {@link subscribeToProposals}.
 *
 *  Errors propagate to every joiner; each decides whether to surface them. */
export function withProposalRun(fn: () => Promise<void>): Promise<void> {
  if (current) return current;
  const run = fn().finally(() => {
    if (current === run) current = null;
  });
  current = run;
  return run;
}

/**
 * Propose for everything newly unreviewed, after a connector sync — so the Review tab is ready
 * when it's opened rather than starting its work then.
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
