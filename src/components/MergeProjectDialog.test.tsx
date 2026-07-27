// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sentence a user reads immediately before an irreversible, undo-less action, so its
// honesty matters more than its prose: no invented counts, no "0 milestones", and no claim
// that something moves when the source project is empty.
//
// Pure formatting, so this stays in the default `node` environment — no jsdom opt-in needed.

import { describe, expect, it } from "vitest";
import { movesClause } from "./MergeProjectDialog";
import type { MergePreview } from "../lib/types";

function preview(over: Partial<MergePreview> = {}): MergePreview {
  return {
    files: 0,
    chats: 0,
    milestones: 0,
    into_canonical: "Marketing",
    ...over,
  };
}

describe("movesClause", () => {
  it("lists all three kinds in a readable clause", () => {
    expect(movesClause(preview({ files: 12, chats: 5, milestones: 3 }))).toBe(
      "12 files, 5 chats and 3 milestones",
    );
  });

  it("omits the kinds that are zero rather than printing '0 milestones'", () => {
    expect(movesClause(preview({ files: 4 }))).toBe("4 files");
    expect(movesClause(preview({ files: 4, milestones: 2 }))).toBe("4 files and 2 milestones");
    expect(movesClause(preview({ chats: 7 }))).toBe("7 chats");
  });

  it("singularises a count of one", () => {
    expect(movesClause(preview({ files: 1, chats: 1, milestones: 1 }))).toBe(
      "1 file, 1 chat and 1 milestone",
    );
  });

  // An empty source is a real case (a project whose documents were already re-filed by hand).
  // The dialog branches on null to say "nothing will move" instead of asserting a move.
  it("returns null when the source project is empty", () => {
    expect(movesClause(preview())).toBeNull();
  });
});
