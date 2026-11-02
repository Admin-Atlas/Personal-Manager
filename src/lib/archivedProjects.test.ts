// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { archivedSet, isArchived, splitByArchivedHome } from "./archivedProjects";

describe("archived projects (#1018)", () => {
  it("matches names trimmed and case-folded, like the backend roster", () => {
    const set = archivedSet(["Atlas"]);
    expect(isArchived(set, "atlas ")).toBe(true);
    expect(isArchived(set, "Atlas 2")).toBe(false);
    expect(isArchived(set, null)).toBe(false);
  });

  it("hides a file only when its HOME project is archived", () => {
    const docs = [
      { id: 1, project: "Atlas" },
      { id: 2, project: "Live" }, // linked to Atlas elsewhere, but lives in a live project
      { id: 3, project: "Unsorted" },
    ];
    const { shown, hidden } = splitByArchivedHome(docs, archivedSet(["Atlas"]));
    expect(hidden.map((d) => d.id)).toEqual([1]);
    expect(shown.map((d) => d.id)).toEqual([2, 3]);
  });

  it("hides nothing when nothing is archived", () => {
    const docs = [{ project: "Atlas" }];
    expect(splitByArchivedHome(docs, archivedSet([])).shown).toEqual(docs);
  });
});
