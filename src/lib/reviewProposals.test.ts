// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { seedReviewEdit } from "./reviewProposals";
import type { MetadataProposal } from "./types";

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
