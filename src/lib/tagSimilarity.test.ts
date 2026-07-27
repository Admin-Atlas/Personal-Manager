// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Folding two tags into one is a bulk vault rewrite with no undo, so a WRONG suggestion is
// expensive and a missed one is free (the user can still rename either tag by hand). These tests
// pin that asymmetry: the rules stay narrow, and the pairs that must NOT be offered matter more
// than the ones that must.

import { describe, expect, it } from "vitest";
import { findSimilarTags, foldKey, looksLikeSameTag } from "./tagSimilarity";
import type { TagSummary } from "./types";

const tag = (name: string, documents = 1, kind: TagSummary["kind"] = "group"): TagSummary => ({
  name,
  kind,
  documents,
});

describe("looksLikeSameTag", () => {
  it("ignores case, spacing and punctuation", () => {
    expect(looksLikeSameTag("meeting-notes", "meeting notes")).toBe(true);
    expect(looksLikeSameTag("MeetingNotes", "meeting-notes")).toBe(true);
    expect(looksLikeSameTag("chair_application", "chair application")).toBe(true);
  });

  it("catches the simple plural, in either direction", () => {
    expect(looksLikeSameTag("tax", "taxes")).toBe(true);
    expect(looksLikeSameTag("invoices", "invoice")).toBe(true);
    expect(looksLikeSameTag("policy", "policies")).toBe(true);
    expect(looksLikeSameTag("chair", "chairs")).toBe(true);
  });

  // The expensive direction. Each of these is a plausible edit-distance "match" and a real pair of
  // different things; offering any of them as a one-click fold would destroy data.
  it("does not pair genuinely different short words", () => {
    expect(looksLikeSameTag("tax", "fax")).toBe(false);
    expect(looksLikeSameTag("cv", "cs")).toBe(false);
    expect(looksLikeSameTag("bimun", "ammun")).toBe(false);
    expect(looksLikeSameTag("spec", "specification")).toBe(false);
    expect(looksLikeSameTag("research", "recruitment")).toBe(false);
  });

  it("is not fooled by an empty or punctuation-only name", () => {
    expect(looksLikeSameTag("", "tax")).toBe(false);
    expect(looksLikeSameTag("---", "tax")).toBe(false);
  });
});

describe("findSimilarTags", () => {
  it("puts the tag carrying more documents first — the one a fold should keep", () => {
    const pairs = findSimilarTags([tag("tax", 2), tag("taxes", 9)]);
    expect(pairs).toHaveLength(1);
    expect(pairs[0][0].name).toBe("taxes");
    expect(pairs[0][1].name).toBe("tax");
  });

  it("breaks a tie alphabetically so the list does not reshuffle between renders", () => {
    const a = findSimilarTags([tag("taxes", 3), tag("tax", 3)]);
    const b = findSimilarTags([tag("tax", 3), tag("taxes", 3)]);
    expect(a[0][0].name).toBe(b[0][0].name);
    expect(a[0][0].name).toBe("tax");
  });

  // Three spellings of one word must not produce a tangle where accepting one suggestion silently
  // invalidates another that is still on screen.
  it("uses each tag in at most one pair", () => {
    const pairs = findSimilarTags([tag("tax", 1), tag("taxes", 2), tag("Tax", 3)]);
    expect(pairs).toHaveLength(1);
    const named = pairs.flat().map((t) => t.name);
    expect(new Set(named).size).toBe(2);
  });

  // Projects have their own merge flow, with entities and aliases behind it. Offering one here
  // would route a project merge through a path that knows nothing about either.
  it("never offers a project", () => {
    expect(findSimilarTags([tag("Sales", 5, "project"), tag("sale", 1, "project")])).toEqual([]);
    expect(findSimilarTags([tag("Sales", 5, "project"), tag("sales", 1)])).toEqual([]);
  });

  it("finds nothing in a clean vocabulary", () => {
    expect(findSimilarTags([tag("invoice"), tag("meeting-notes"), tag("research")])).toEqual([]);
  });
});

describe("foldKey", () => {
  it("strips everything that is not a letter or digit", () => {
    expect(foldKey("Meeting-Notes 2024!")).toBe("meetingnotes2024");
  });
});
