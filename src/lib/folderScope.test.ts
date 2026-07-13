// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { applyFolderToggle, pruneStrandedSeed } from "./folderScope";

describe("applyFolderToggle", () => {
  const empty = { selected: [], excluded: [] };

  it("checking a root folder (no included ancestor) adds a seed root", () => {
    // Drive/OneDrive "Choose folders": picking a top-level folder includes it recursively.
    expect(applyFolderToggle(empty, "A", false, true)).toEqual({
      selected: ["A"],
      excluded: [],
    });
  });

  it("unchecking an included child (via ancestor) excludes its subtree", () => {
    const cur = { selected: ["A"], excluded: [] };
    expect(applyFolderToggle(cur, "B", true, false)).toEqual({
      selected: ["A"],
      excluded: ["B"],
    });
  });

  it("re-checking an excluded child under an included ancestor just drops the exclude", () => {
    const cur = { selected: ["A"], excluded: ["B"] };
    expect(applyFolderToggle(cur, "B", true, true)).toEqual({
      selected: ["A"],
      excluded: [],
    });
  });

  it("unchecking an explicit seed root removes it (no exclude when no ancestor covers it)", () => {
    const cur = { selected: ["A"], excluded: [] };
    expect(applyFolderToggle(cur, "A", false, false)).toEqual({
      selected: [],
      excluded: [],
    });
  });

  it("unchecking a folder that is both a seed root and ancestor-covered removes and excludes it", () => {
    const cur = { selected: ["A", "B"], excluded: [] };
    // B is selected but also under A → drop from selected AND exclude so the subtree is pruned.
    expect(applyFolderToggle(cur, "B", true, false)).toEqual({
      selected: ["A"],
      excluded: ["B"],
    });
  });

  it("local (whole folder indexed): unchecking a subfolder only adds an exclude", () => {
    // rootIncluded → ancestorIncluded is true at the top level; selected stays empty.
    expect(applyFolderToggle(empty, "Archive", true, false)).toEqual({
      selected: [],
      excluded: ["Archive"],
    });
  });

  it("local: re-checking an excluded subfolder clears it without adding a seed root", () => {
    const cur = { selected: [], excluded: ["Archive"] };
    expect(applyFolderToggle(cur, "Archive", true, true)).toEqual({
      selected: [],
      excluded: [],
    });
  });

  it("is idempotent on the exclude set (no duplicates)", () => {
    const cur = { selected: ["A"], excluded: ["B"] };
    expect(applyFolderToggle(cur, "B", true, false)).toEqual(cur);
  });
});

describe("pruneStrandedSeed", () => {
  it("drops a redundant seed that sits under an included ancestor", () => {
    // Q3 was picked before its ancestor Projects; once Projects covers it, Q3 is redundant.
    const cur = { selected: ["Q3", "Projects"], excluded: [] };
    expect(pruneStrandedSeed(cur, "Q3", true, false)).toEqual({
      selected: ["Projects"],
      excluded: [],
    });
  });

  it("drops a stranded seed under an excluded ancestor (the leak fix)", () => {
    // Q3 is its own seed but sits under excluded 2024 → would be indexed despite the exclude.
    const cur = { selected: ["Q3", "Projects"], excluded: ["2024"] };
    expect(pruneStrandedSeed(cur, "Q3", false, true)).toEqual({
      selected: ["Projects"],
      excluded: ["2024"],
    });
  });

  it("leaves a genuine top-level seed untouched, returning the same object", () => {
    const cur = { selected: ["Projects"], excluded: [] };
    expect(pruneStrandedSeed(cur, "Projects", false, false)).toBe(cur);
  });

  it("no-ops for a folder that isn't a seed, returning the same object", () => {
    const cur = { selected: ["Projects"], excluded: [] };
    expect(pruneStrandedSeed(cur, "Other", true, false)).toBe(cur);
  });
});
