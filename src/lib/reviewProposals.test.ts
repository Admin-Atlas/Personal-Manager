// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  proposalCache,
  proposeOnArrival,
  resetArrivalProposals,
  seedReviewEdit,
  withProposalRun,
} from "./reviewProposals";
import { REVIEW_AI_KEY } from "./reviewPrefs";
import type { Document, MetadataProposal } from "./types";

const aiProviderStatus = vi.hoisted(() => vi.fn());
const proposeMetadata = vi.hoisted(() => vi.fn());
vi.mock("./ipc", () => ({
  aiProviderStatus,
  proposeMetadata,
  cachedProposals: vi.fn(async () => []),
  reviewQueue: vi.fn(async () => []),
}));

const proposal: MetadataProposal = {
  project: "BIMUN 2026",
  tags: ["logistics", "travel"],
  importance: "high",
  reasoning: "Mentions the delegation's flight booking and the March deadline.",
};

const doc = { project: "Unsorted", tags: [] as string[], importance: null };

describe("seedReviewEdit", () => {
  it("uses the AI proposal when the user hasn't touched the row", () => {
    // The bug: this returned the document's blank values, so a proposal produced while the Review
    // tab was closed (post-sync run) or restored from the DB after a restart painted its reasoning
    // over an Unsorted/untriaged/no-tags row.
    expect(seedReviewEdit(undefined, proposal, doc)).toEqual({
      project: "BIMUN 2026",
      tags: ["logistics", "travel"],
      importance: "high",
    });
  });

  it("keeps a hand-edit ahead of the proposal", () => {
    const hand = { project: "Archive", tags: ["old"], importance: "low" as const };
    expect(seedReviewEdit(hand, proposal, doc)).toBe(hand);
  });

  it("falls back to the document when there is no proposal", () => {
    // Suggestions off, or a document ingested since the last run — these must still seed from the
    // document rather than blanking, so removing the fallback branch is a regression.
    expect(seedReviewEdit(undefined, undefined, doc)).toEqual({
      project: "Unsorted",
      tags: [],
      importance: null,
    });
  });

  it("preserves document values that are already filed", () => {
    // A general chat re-queued by `reevaluate_on_append` keeps its project/tags/importance, so the
    // no-proposal fallback is not always blank.
    const filed = { project: "BIMUN 2026", tags: ["notes"], importance: "medium" as const };
    expect(seedReviewEdit(undefined, undefined, filed)).toEqual(filed);
  });

  it("does not alias the proposal's tag array into the edit", () => {
    // Defensive, not a live bug: today's TagEditor always rebuilds the array. But `decisionFor`
    // compares the edit against the proposal to decide what to log as a correction, so a shared
    // array would make a tag change move both sides and log nothing.
    const seeded = seedReviewEdit(undefined, proposal, doc);
    expect(seeded.tags).toEqual(proposal.tags);
    seeded.tags.push("mutated");
    expect(proposal.tags).toEqual(["logistics", "travel"]);
  });
});

// A second caller's ids used to be dropped: it received the in-flight promise and its own `fn` was
// never invoked, so two connectors finishing close together silently lost the second's documents.
describe("withProposalRun", () => {
  it("runs a joiner's work after the in-flight run instead of discarding it", async () => {
    const order: string[] = [];
    let releaseFirst: () => void = () => {};
    const first = withProposalRun(async () => {
      order.push("first:start");
      await new Promise<void>((r) => (releaseFirst = r));
      order.push("first:end");
    });
    const second = withProposalRun(async () => {
      order.push("second:start");
    });
    releaseFirst();
    await Promise.all([first, second]);
    expect(order).toEqual(["first:start", "first:end", "second:start"]);
  });

  it("still starts the queued run when the one before it fails", async () => {
    let ran = false;
    const failing = withProposalRun(async () => {
      throw new Error("no credits");
    });
    await expect(failing).rejects.toThrow("no credits");
    await withProposalRun(async () => {
      ran = true;
    });
    expect(ran).toBe(true);
  });

  it("surfaces a run's own error to its own caller only", async () => {
    const bad = withProposalRun(async () => {
      throw new Error("boom");
    });
    await expect(bad).rejects.toThrow("boom");
    await expect(withProposalRun(async () => {})).resolves.toBeUndefined();
  });
});

// Suggestions used to be triggered by a sync FINISHING and nothing else, so a file the live watcher
// picked up — or one dropped in by hand — waited for the next sync (or for Review to be opened)
// before it got one, for no reason a user could perceive. They are now driven by the arrival itself.
describe("proposeOnArrival", () => {
  const landed = (id: number, reviewed = false) => ({ id, reviewed }) as unknown as Document;
  /** The ids each `proposeMetadata` call was asked to propose for, in call order. */
  const batches = () => proposeMetadata.mock.calls.map((c) => c[1]);

  beforeEach(() => {
    vi.clearAllMocks();
    resetArrivalProposals();
    proposalCache.clear();
    localStorage.setItem(REVIEW_AI_KEY, "true");
    aiProviderStatus.mockResolvedValue({ has_cloud_key: true, local_configured: false });
    proposeMetadata.mockResolvedValue(undefined);
  });
  afterEach(() => {
    resetArrivalProposals();
    localStorage.clear();
  });

  it("proposes in batches of five so suggestions keep pace with the files", async () => {
    proposeOnArrival(Array.from({ length: 12 }, (_, i) => landed(i + 1)));
    await vi.waitFor(() => expect(proposeMetadata).toHaveBeenCalledTimes(3));
    expect(batches()).toEqual([
      [1, 2, 3, 4, 5],
      [6, 7, 8, 9, 10],
      [11, 12],
    ]);
  });

  it("reads the key store once per burst, not once per batch", async () => {
    // It probes the keychain, and forty probes across one sync is a real cost for an answer that
    // cannot meaningfully change mid-burst.
    proposeOnArrival(Array.from({ length: 12 }, (_, i) => landed(i + 1)));
    await vi.waitFor(() => expect(proposeMetadata).toHaveBeenCalledTimes(3));
    expect(aiProviderStatus).toHaveBeenCalledTimes(1);
  });

  it("spends nothing when AI suggestions are switched off", async () => {
    localStorage.setItem(REVIEW_AI_KEY, "false");
    proposeOnArrival(Array.from({ length: 12 }, (_, i) => landed(i + 1)));
    await new Promise((r) => setTimeout(r, 0));
    expect(aiProviderStatus).not.toHaveBeenCalled();
    expect(proposeMetadata).not.toHaveBeenCalled();
  });

  it("stays quiet when no model is linked", async () => {
    aiProviderStatus.mockResolvedValue({ has_cloud_key: false, local_configured: false });
    proposeOnArrival(Array.from({ length: 6 }, (_, i) => landed(i + 1)));
    await vi.waitFor(() => expect(aiProviderStatus).toHaveBeenCalled());
    expect(proposeMetadata).not.toHaveBeenCalled();
  });

  it("never re-bills a document that already has a suggestion, or one already filed", async () => {
    proposalCache.set(2, proposal);
    const docs = [landed(1), landed(2), landed(3, true), landed(4)];
    docs.push(...[5, 6, 7, 8].map((id) => landed(id)));
    proposeOnArrival(docs);
    await vi.waitFor(() => expect(proposeMetadata).toHaveBeenCalled());
    // 2 is cached and 3 is already reviewed; only the rest are worth paying for.
    expect(batches().flat()).toEqual([1, 4, 5, 6, 7, 8]);
  });

  it("proposes for a lone file once the batch stops waiting for company", async () => {
    // The watcher case: drop one file into a tracked folder and a fifth may never arrive. A batch
    // that only ever fires at five would leave it unsuggested indefinitely.
    vi.useFakeTimers();
    try {
      proposeOnArrival([landed(1)]);
      expect(proposeMetadata).not.toHaveBeenCalled(); // still waiting
      await vi.advanceTimersByTimeAsync(2000);
      expect(batches()).toEqual([[1]]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not queue the same document twice when a sync re-announces it", async () => {
    proposeOnArrival([landed(1), landed(2), landed(1)]);
    proposeOnArrival([landed(2), landed(3), landed(4), landed(5)]);
    await vi.waitFor(() => expect(proposeMetadata).toHaveBeenCalled());
    expect(batches().flat()).toEqual([1, 2, 3, 4, 5]);
  });

  it("abandons the drain when a batch fails rather than retrying it per batch", async () => {
    // A model that is down would otherwise mean one failed round-trip per five files for the whole
    // sync. The post-sync sweep and the Review tab re-derive what is still unproposed.
    proposeMetadata.mockRejectedValueOnce(new Error("no credits"));
    proposeOnArrival(Array.from({ length: 12 }, (_, i) => landed(i + 1)));
    await vi.waitFor(() => expect(proposeMetadata).toHaveBeenCalledTimes(1));
    await new Promise((r) => setTimeout(r, 0));
    expect(proposeMetadata).toHaveBeenCalledTimes(1);
  });
});
