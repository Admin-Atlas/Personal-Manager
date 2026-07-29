// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { beforeEach, describe, expect, it } from "vitest";
import {
  landingSeq,
  landingsSince,
  mergeLandings,
  pushLanding,
  resetDocumentFeed,
} from "./documentFeed";
import type { Document } from "./types";

const doc = (id: number): Document => ({ id, title: `d${id}` }) as Document;

describe("mergeLandings", () => {
  it("puts arrivals at the front, newest first", () => {
    const merged = mergeLandings([doc(1)], [doc(2), doc(3)]);
    expect(merged.map((d) => d.id)).toEqual([3, 2, 1]);
  });

  it("never duplicates a row already on screen", () => {
    // A document can be both in the loaded page and replayed from the gap buffer.
    const merged = mergeLandings([doc(1), doc(2)], [doc(2)]);
    expect(merged.map((d) => d.id)).toEqual([1, 2]);
  });

  it("dedups within a single batch", () => {
    const merged = mergeLandings([], [doc(5), doc(5)]);
    expect(merged.map((d) => d.id)).toEqual([5]);
  });

  it("returns the original array when nothing is new, so React can skip the render", () => {
    const existing = [doc(1)];
    expect(mergeLandings(existing, [doc(1)])).toBe(existing);
    expect(mergeLandings(existing, [])).toBe(existing);
  });
});

describe("the arrival sequence", () => {
  beforeEach(resetDocumentFeed);

  it("replays only what landed after the captured point", () => {
    pushLanding(doc(1));
    // A view captures the sequence, then awaits its query...
    const since = landingSeq();
    pushLanding(doc(2));
    pushLanding(doc(3));
    // ...and recovers what committed during that await, which a wholesale setState would drop.
    expect(landingsSince(since).map((d) => d.id)).toEqual([2, 3]);
  });

  it("reports nothing when no document landed during the gap", () => {
    pushLanding(doc(1));
    expect(landingsSince(landingSeq())).toEqual([]);
  });

  it("starts clean after a reset, so one vault's arrivals never reach another", () => {
    pushLanding(doc(1));
    resetDocumentFeed();
    expect(landingSeq()).toBe(0);
    expect(landingsSince(0)).toEqual([]);
  });
});
