// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Documents table's column model (#701): Depth seeds it, the picker owns it, Reset hands it
// back. The load-bearing property is that a user who never opens the picker sees no change at all.

import { beforeEach, describe, expect, it } from "vitest";
import {
  DOC_COLUMNS_KEY,
  DOC_COLUMN_KEYS,
  defaultColumns,
  readColumns,
  toggleColumn,
  writeColumns,
  type DocColumnKey,
} from "./documentColumns";

beforeEach(() => localStorage.clear());

describe("defaultColumns", () => {
  it("shows more of the document as Depth rises, and never more than that", () => {
    // Deliberately hand-chosen rather than "whatever the table happened to show before the picker
    // existed", which is what these numbers used to pin. `min` answers where a document lives;
    // `standard` adds the two facts people actually scan a library by plus how much it matters;
    // `power` adds the chunk count, which is a statement about the index rather than the document.
    expect(defaultColumns("min")).toEqual(["project"]);
    expect(defaultColumns("standard")).toEqual(["project", "importance", "author", "size"]);
    expect(defaultColumns("power")).toEqual(["project", "importance", "chunks", "author", "size"]);
  });

  it("leaves the per-document questions off until they are asked for", () => {
    // Who last changed it, when it was created, when it was updated, when PM ingested or last
    // synced it: real questions, but questions about ONE document, and a column answers them for a
    // thousand at the cost of the width the title needs.
    for (const depth of ["min", "standard", "power"] as const) {
      for (const key of ["modifiedBy", "created", "updated", "ingested", "synced"] as const) {
        expect(defaultColumns(depth)).not.toContain(key);
      }
    }
  });

  it("returns columns in canonical display order whatever order they are listed in", () => {
    for (const depth of ["min", "standard", "power"] as const) {
      const cols = defaultColumns(depth);
      const canonical = DOC_COLUMN_KEYS.filter((k) => cols.includes(k));
      expect(cols).toEqual([...canonical]);
    }
  });

  it("only moves a user who has never opened the picker", () => {
    // The migration property, and the thing to say in What's New: a stored choice is a full explicit
    // set, never a diff from the Depth default, so changing these defaults cannot reshape the table
    // of anyone who has ever ticked a box — until they press Reset.
    writeColumns(["chunks", "ingested"]);
    expect(readColumns()).toEqual(["chunks", "ingested"]);
    writeColumns(null);
    expect(readColumns()).toBeNull();
  });
});

describe("readColumns / writeColumns", () => {
  it("returns null when the user has never chosen, so the table keeps following Depth", () => {
    expect(readColumns()).toBeNull();
  });

  it("round-trips a choice in canonical display order", () => {
    writeColumns(["size", "project"]);
    expect(readColumns()).toEqual(["project", "size"]);
  });

  it("clears back to following Depth", () => {
    writeColumns(["size"]);
    writeColumns(null);
    expect(readColumns()).toBeNull();
    expect(localStorage.getItem(DOC_COLUMNS_KEY)).toBeNull();
  });

  it("drops a column key it no longer knows rather than stranding the whole preference", () => {
    // The enum-deletion trap from PR #538, one layer up: a value removed in a later version must
    // not make the stored choice unreadable and silently reset everything the user had picked.
    localStorage.setItem(DOC_COLUMNS_KEY, JSON.stringify(["project", "retiredColumn", "size"]));
    expect(readColumns()).toEqual(["project", "size"]);
  });

  it("treats junk, and an empty set, as no choice at all", () => {
    // A table showing only titles is not something anyone chose, so it reads as unset and Depth
    // takes over — rather than leaving the user with one column and no obvious way back.
    localStorage.setItem(DOC_COLUMNS_KEY, "not json");
    expect(readColumns()).toBeNull();
    localStorage.setItem(DOC_COLUMNS_KEY, JSON.stringify([]));
    expect(readColumns()).toBeNull();
    localStorage.setItem(DOC_COLUMNS_KEY, JSON.stringify(["nothing", "real"]));
    expect(readColumns()).toBeNull();
  });
});

describe("toggleColumn", () => {
  it("adds and removes, always in canonical order", () => {
    const start: DocColumnKey[] = ["project", "chunks"];
    expect(toggleColumn(start, "importance")).toEqual(["project", "importance", "chunks"]);
    expect(toggleColumn(start, "project")).toEqual(["chunks"]);
  });

  it("orders a newly added column by the table's layout, not by when it was clicked", () => {
    // The header row and the body cells both map over DOC_COLUMN_KEYS, so a set in click order
    // would put the header and the cell in different places.
    const all = DOC_COLUMN_KEYS.reduce<DocColumnKey[]>((acc, k) => toggleColumn(acc, k), []);
    expect(all).toEqual([...DOC_COLUMN_KEYS]);
  });
});
