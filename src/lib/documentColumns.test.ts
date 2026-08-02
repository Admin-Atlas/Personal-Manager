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
  it("is exactly what the table showed before the picker existed", () => {
    // The whole point of seeding from Depth rather than inventing a new default: nobody's table
    // changes shape until they ask it to. The four source facts are OFF at every Depth.
    expect(defaultColumns(false)).toEqual(["project", "importance", "chunks"]);
    expect(defaultColumns(true)).toEqual(["project", "importance", "chunks", "ingested"]);
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
