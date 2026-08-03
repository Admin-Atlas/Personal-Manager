// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the source knows, and — the part that was decided up front and matters most — what it says
// when the source knows nothing (#701).

import { describe, expect, it } from "vitest";
import { SOURCE_FACT_KEYS, sourceFactValue, sourceFacts, UNKNOWN } from "./sourceFacts";
import type { Document } from "./types";

function doc(over: Partial<Document> = {}): Document {
  return {
    id: 1,
    title: "A",
    source_path: null,
    ext: "md",
    byte_size: 100,
    chunk_count: 1,
    created_at: null,
    ingested_at: "2026-07-30T09:00:00Z",
    project: "Unsorted",
    linked_projects: [],
    tags: [],
    importance: null,
    reviewed: false,
    last_activity: null,
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
    source_author: null,
    source_last_modified_by: null,
    source_created_at: null,
    source_size_bytes: null,
    source_modified_at: null,
    pm_refreshed_at: null,
    ...over,
  };
}

describe("sourceFacts", () => {
  it("renders what the provider said", () => {
    const facts = sourceFacts(
      doc({
        source_author: "Jane Okafor",
        source_last_modified_by: "Sam Reyes",
        source_created_at: "2025-11-04T09:00:00Z",
        source_modified_at: "2025-12-20T09:00:00Z",
        source_size_bytes: 2_411_724,
      }),
    );
    expect(facts.map((f) => [f.label, f.value])).toEqual([
      ["Author", "Jane Okafor"],
      ["Modified by", "Sam Reyes"],
      // DD-MM-YYYY, like every other date in the app.
      ["Created", "04-11-2025"],
      // "Updated", not "Modified": it sits next to "Modified by", and two headings a word apart
      // that mean different things is a pair the reader has to stop and disambiguate.
      ["Updated", "20-12-2025"],
      ["Size", "2 MB"],
    ]);
    expect(facts.every((f) => f.known)).toBe(true);
  });

  it('says "Unknown" — never blank, never "you" — when the provider said nothing', () => {
    // The decision this module exists to hold. A local file has no author PM could honestly report,
    // and naming the person looking at the screen would be worse than no answer.
    const facts = sourceFacts(doc());
    expect(facts.map((f) => f.value)).toEqual([UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN, UNKNOWN]);
    expect(facts.every((f) => !f.known)).toBe(true);
  });

  it("always returns every fact, so two compare cards stay level", () => {
    // The duplicates surface renders these side by side. A card that dropped its unknown rows would
    // put "Created" on one side level with "Size" on the other — which is exactly the confusion the
    // panel was asked to remove.
    const known = sourceFacts(doc({ source_author: "Jane Okafor" }));
    const unknown = sourceFacts(doc());
    expect(known).toHaveLength(SOURCE_FACT_KEYS.length);
    expect(unknown).toHaveLength(SOURCE_FACT_KEYS.length);
    expect(known.map((f) => f.key)).toEqual(unknown.map((f) => f.key));
  });

  it("treats a zero-byte file as a size, but a missing size as unknown", () => {
    // Distinct cases with distinct answers: an empty file really is 0 B, whereas a Google-native
    // Doc has no byte size at all. `??` on a number would have collapsed them.
    expect(sourceFactValue(doc({ source_size_bytes: 0 }), "size")).toBe("0 B");
    expect(sourceFactValue(doc({ source_size_bytes: null }), "size")).toBe(UNKNOWN);
  });
});
