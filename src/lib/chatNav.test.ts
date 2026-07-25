// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { globalChats, projectChats } from "./chatNav";
import type { Conversation } from "./types";

function conv(id: number, title: string, project?: string | null): Conversation {
  return { id, title, created_at: "2026-07-25T10:00:00Z", project } as Conversation;
}

describe("globalChats", () => {
  it("keeps only unscoped conversations, in the given order", () => {
    const list = [conv(1, "one"), conv(2, "two", "Alpha"), conv(3, "three", null)];
    expect(globalChats(list).map((c) => c.id)).toEqual([1, 3]);
  });

  it("treats an empty-string project as global", () => {
    // Defensive: the column is nullable, but an empty string must not render a nameless project row.
    expect(globalChats([conv(1, "one", "")]).map((c) => c.id)).toEqual([1]);
  });

  it("handles an empty list", () => {
    expect(globalChats([])).toEqual([]);
  });
});

describe("projectChats", () => {
  it("groups scoped chats under their project", () => {
    const list = [conv(1, "a", "Alpha"), conv(2, "b", "Beta"), conv(3, "c", "Alpha")];
    expect(projectChats(list, [])).toEqual([
      { project: "Alpha", chats: [list[0], list[2]] },
      { project: "Beta", chats: [list[1]] },
    ]);
  });

  it("includes a known project that has no chats yet", () => {
    expect(projectChats([], ["Gamma"])).toEqual([{ project: "Gamma", chats: [] }]);
  });

  it("includes a project that has chats but is missing from the known list", () => {
    // `list_projects` is DISTINCT over documents, so a project with only chats is absent from it.
    const list = [conv(1, "a", "Chat only")];
    expect(projectChats(list, ["Docs only"])).toEqual([
      { project: "Chat only", chats: [list[0]] },
      { project: "Docs only", chats: [] },
    ]);
  });

  it("sorts by name case-insensitively so the section doesn't reshuffle", () => {
    const out = projectChats([conv(1, "a", "zebra")], ["Apple", "banana"]);
    expect(out.map((p) => p.project)).toEqual(["Apple", "banana", "zebra"]);
  });

  it("ignores global chats and empty project names", () => {
    const list = [conv(1, "a"), conv(2, "b", ""), conv(3, "c", "Real")];
    expect(projectChats(list, [""]).map((p) => p.project)).toEqual(["Real"]);
  });

  it("does not duplicate a project present in both inputs", () => {
    const list = [conv(1, "a", "Alpha")];
    expect(projectChats(list, ["Alpha"])).toEqual([{ project: "Alpha", chats: [list[0]] }]);
  });
});
