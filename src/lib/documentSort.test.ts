// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { beforeEach, describe, expect, it } from "vitest";
import {
  DOC_SORT_KEY,
  isSortKey,
  nextDocSort,
  readDocSort,
  sortDocuments,
  writeDocSort,
  type DocSort,
} from "./documentSort";
import type { Document } from "./types";

/** A document row with only the fields a sort reads; the rest are inert defaults. */
function doc(over: Partial<Document> & { title: string }): Document {
  return {
    id: 1,
    source_path: null,
    ext: null,
    byte_size: null,
    chunk_count: 0,
    created_at: null,
    ingested_at: "2026-01-01T00:00:00Z",
    project: "Unsorted",
    importance: null,
    reviewed: true,
    source_type: "vault",
    source_state: "ok",
    external_ref: null,
    source_id: null,
    source_parent_folder_id: null,
    source_parent_folder_name: null,
    source_author: null,
    source_last_modified_by: null,
    source_created_at: null,
    source_modified_at: null,
    source_size_bytes: null,
    pm_refreshed_at: null,
    ...over,
  } as unknown as Document;
}

const titles = (ds: Document[]) => ds.map((d) => d.title);

describe("nextDocSort", () => {
  it("flips direction on the same header and starts a new one in its natural direction", () => {
    expect(nextDocSort(null, "title")).toEqual({ key: "title", dir: "asc" });
    expect(nextDocSort({ key: "title", dir: "asc" }, "title")).toEqual({
      key: "title",
      dir: "desc",
    });
    // Biggest / most recent first is the more useful first click on these.
    expect(nextDocSort(null, "size")).toEqual({ key: "size", dir: "desc" });
    expect(nextDocSort({ key: "title", dir: "desc" }, "importance")).toEqual({
      key: "importance",
      dir: "desc",
    });
  });
});

describe("sortDocuments", () => {
  it("returns the backend's own order untouched when there is no sort", () => {
    const rows = [doc({ title: "b" }), doc({ title: "a" })];
    const out = sortDocuments(rows, null);
    expect(out).toBe(rows); // not merely equal — no copy, no reorder
  });

  it("does not mutate its input", () => {
    const rows = [doc({ title: "b" }), doc({ title: "a" })];
    sortDocuments(rows, { key: "title", dir: "asc" });
    expect(titles(rows)).toEqual(["b", "a"]);
  });

  it("keeps unknown values LAST in both directions", () => {
    // The objection this answers: ordering by a column most rows have no answer for would otherwise
    // bank the Unknowns at whichever end you just clicked towards, burying every row that HAS one.
    const rows = [
      doc({ title: "no-author" }),
      doc({ title: "zoe", source_author: "Zoe" }),
      doc({ title: "adam", source_author: "Adam" }),
    ];
    expect(titles(sortDocuments(rows, { key: "author", dir: "asc" }))).toEqual([
      "adam",
      "zoe",
      "no-author",
    ]);
    expect(titles(sortDocuments(rows, { key: "author", dir: "desc" }))).toEqual([
      "zoe",
      "adam",
      "no-author",
    ]);
  });

  it("groups by where a document lives, with the two kinds of trouble last", () => {
    const rows = [
      doc({ title: "drive", source_id: "gdrive:me@example.com:1", source_type: "index_only" }),
      doc({ title: "gone", source_id: "gdrive:me@example.com:2", source_state: "source_missing" }),
      doc({ title: "here" }),
      doc({ title: "device", source_id: "local:k:3", source_type: "index_only" }),
      doc({
        title: "offline",
        source_id: "onedrive:me@example.com:4",
        source_state: "unreachable",
      }),
      doc({ title: "onedrive", source_id: "onedrive:me@example.com:5", source_type: "index_only" }),
    ];
    expect(titles(sortDocuments(rows, { key: "source", dir: "asc" }))).toEqual([
      "here",
      "device",
      "drive",
      "onedrive",
      "offline",
      "gone",
    ]);
    // Descending reaches the broken rows from the other side — neither direction buries them.
    expect(titles(sortDocuments(rows, { key: "source", dir: "desc" }))[0]).toBe("gone");
  });

  it("ranks a tracked local folder as this device even though it is index-only", () => {
    // The trap: `source_type` alone says "index_only" for a file sitting on this very machine, so a
    // rank keyed on the type would file it with the clouds.
    const rows = [
      doc({ title: "cloud", source_id: "gdrive:me@example.com:1", source_type: "index_only" }),
      doc({ title: "mine", source_id: "local:k:2", source_type: "index_only" }),
    ];
    expect(titles(sortDocuments(rows, { key: "source", dir: "asc" }))).toEqual(["mine", "cloud"]);
  });

  it("falls back to the title so equal rows keep a stable order", () => {
    const rows = [doc({ title: "b", importance: "high" }), doc({ title: "a", importance: "high" })];
    expect(titles(sortDocuments(rows, { key: "importance", dir: "asc" }))).toEqual(["a", "b"]);
    // The tiebreak takes the direction factor with it, so flipping the arrow reverses ties too —
    // the whole list reverses, which is what an arrow flip looks like. Unknowns are the deliberate
    // exception (above): they are pre-multiplied so they stay at the bottom either way.
    expect(titles(sortDocuments(rows, { key: "importance", dir: "desc" }))).toEqual(["b", "a"]);
  });
});

describe("the stored sort", () => {
  beforeEach(() => localStorage.clear());

  it("round-trips, and clearing removes the key rather than storing a null", () => {
    const sort: DocSort = { key: "size", dir: "desc" };
    writeDocSort(sort);
    expect(readDocSort()).toEqual(sort);
    writeDocSort(null);
    expect(localStorage.getItem(DOC_SORT_KEY)).toBeNull();
    expect(readDocSort()).toBeNull();
  });

  it("reads junk as no sort at all", () => {
    // A key retired in a later version must not strand the table in an ordering nothing on screen
    // can explain — the oneOf-coercion trap from PR #538, one layer up.
    localStorage.setItem(DOC_SORT_KEY, JSON.stringify({ key: "retiredColumn", dir: "asc" }));
    expect(readDocSort()).toBeNull();
    localStorage.setItem(DOC_SORT_KEY, JSON.stringify({ key: "size", dir: "sideways" }));
    expect(readDocSort()).toBeNull();
    localStorage.setItem(DOC_SORT_KEY, "not json");
    expect(readDocSort()).toBeNull();
    localStorage.setItem(DOC_SORT_KEY, JSON.stringify(["size", "asc"]));
    expect(readDocSort()).toBeNull();
    localStorage.setItem(DOC_SORT_KEY, "null");
    expect(readDocSort()).toBeNull();
  });

  it("accepts every key a header can actually produce", () => {
    expect(isSortKey("title")).toBe(true);
    expect(isSortKey("source")).toBe(true);
    expect(isSortKey("synced")).toBe(true);
    expect(isSortKey("nonsense")).toBe(false);
    expect(isSortKey(undefined)).toBe(false);
  });
});
