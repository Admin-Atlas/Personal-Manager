// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  markerKind,
  nextToken,
  noteSchema as S,
  parseNote,
  roundTrips,
  serializeNote,
  toggleToken,
} from "./richNote";
import { toggleTaskAt } from "./notesMarkdown";

/** Notes the rich editor must hold without changing a character. */
const EXACT = [
  "",
  "just a line",
  "two\nlines",
  "a\n\nblank between\n",
  "  indented prose",
  "trailing spaces  ",
  "# Title\n## Sub\n###### Six",
  ". milk\n. eggs\n  . free range\n  - a dash under it\n. bread",
  "- dash one\n+ dash plus\n* star bullet",
  "[] call Sam\n[x] done\n[ ] spaced\n[X] upper",
  "- [ ] gfm task\n- [x] gfm done\n* [ ] star task",
  "1. one\n2. two\n10. ten\n  1. nested",
  "i. one\nii. two\niv. four",
  "> quoted\n> again\n>  wide gap",
  "---\n***\n_ _ _",
  "```js\nconst a = 1;\n  indented\n```",
  "```\nunclosed fence\n. not a list",
  "**bold** and *it* and __under__ and _em_ and ~~gone~~ and `code` and ``two`ticks``",
  "a [link](https://a.b) and [**bold link**](https://c.d)",
  "**bold with *italic* inside** then *italic with **bold** inside*",
  "5 * 3 * 2 = 30, and a lone * star",
  "| a   | b   |\n| --- | --- |\n| 1   | 2   |",
  "| left | mid | right |\n| :--- | :-: | ----: |\n| x    | y   | z     |",
  "| a \\| b | c   |\n| ------ | --- |",
  "| a \\\\\\| b | c   |\n| -------- | --- |",
  "| C:\\new | c   |\n| ------ | --- |",
  "intro\n\n| a   |\n| --- |\n\nafter",
  "# Plan\n\n. one\n[] two\n\n> note\n\n```\ncode\n```\n\n| a   |\n| --- |\n| 1   |",
  "café ☕ — “quotes” 日本語",
];

describe("parseNote / serializeNote: exact for the note dialect", () => {
  for (const note of EXACT) {
    it(JSON.stringify(note).slice(0, 60), () => {
      expect(serializeNote(parseNote(note))).toBe(note);
      expect(roundTrips(note)).toBe(true);
    });
  }

  it("re-lays a table out, and still counts it as a round trip", () => {
    const note = "| a | bb |\n|---|---|\n| 1 | 2 |";
    expect(serializeNote(parseNote(note))).toBe("| a   | bb  |\n| --- | --- |\n| 1   | 2   |");
    expect(roundTrips(note)).toBe(true);
  });

  it("is idempotent on its own output", () => {
    for (const note of EXACT) {
      const once = serializeNote(parseNote(note));
      expect(serializeNote(parseNote(once))).toBe(once);
    }
  });
});

describe("roundTrips refuses what it can't hold, so those notes stay Markdown", () => {
  it.each([
    ["an odd list indent", ". a\n   . three spaces"],
    ["a list that starts indented", "  . orphan"],
    ["a jump of two levels", ". a\n    . two deep"],
    ["an empty line inside a fence", "```\n\n```"],
  ])("%s", (_why, note) => {
    expect(roundTrips(note)).toBe(false);
  });
});

describe("the document's shape", () => {
  it("nests list items by two-space indent", () => {
    const doc = parseNote(". a\n  . b\n. c");
    const list = doc.firstChild!;
    expect(list.type).toBe(S.nodes.list);
    expect(list.childCount).toBe(2);
    expect(list.firstChild!.lastChild!.type).toBe(S.nodes.list);
  });

  it("keeps each item's own marker and drops delimiters from the text", () => {
    const doc = parseNote("- [x] **ship** it");
    const item = doc.firstChild!.firstChild!;
    expect(item.attrs.token).toBe("- [x]");
    expect(item.textContent).toBe("ship it");
    expect(item.firstChild!.firstChild!.marks.map((m) => m.type.name)).toEqual(["strong"]);
  });

  it("holds a table as rows of cells, header first", () => {
    const table = parseNote("| a | b |\n| --- | --- |\n| 1 | 2 |").firstChild!;
    expect(table.type).toBe(S.nodes.table);
    expect(table.firstChild!.firstChild!.type).toBe(S.nodes.table_header);
    expect(table.lastChild!.lastChild!.textContent).toBe("2");
  });
});

describe("serializeNote after edits", () => {
  it("moves the spaces at the edge of bold outside it", () => {
    const p = S.nodes.paragraph.create(null, [
      S.text("a"),
      S.text(" bold ", [S.marks.strong.create()]),
      S.text("b"),
    ]);
    const out = serializeNote(S.nodes.doc.create(null, [p]));
    expect(out).toBe("a **bold** b");
    expect(roundTrips(out)).toBe(true);
  });

  it("writes a new heading level's marker", () => {
    const h = S.nodes.heading.create({ level: 3, prefix: "# " }, S.text("x"));
    expect(serializeNote(S.nodes.doc.create(null, [h]))).toBe("### x");
  });

  it("writes nested items two spaces a level, whatever they were before", () => {
    const item = (token: string, text: string, nested?: ReturnType<typeof S.nodes.list.create>) =>
      S.nodes.list_item.create({ token }, [
        S.nodes.paragraph.create(null, S.text(text)),
        ...(nested ? [nested] : []),
      ]);
    const doc = S.nodes.doc.create(null, [
      S.nodes.list.create(null, [item(".", "a", S.nodes.list.create(null, [item("-", "b")]))]),
    ]);
    expect(serializeNote(doc)).toBe(". a\n  - b");
  });

  it("escapes a pipe typed into a table cell", () => {
    const table = parseNote("| a   |\n| --- |").firstChild!;
    const cell = S.nodes.table_header.create(null, S.text("x | y"));
    const doc = S.nodes.doc.create(null, [
      table.copy(table.content.replaceChild(0, S.nodes.table_row.create(null, [cell]))),
    ]);
    expect(serializeNote(doc)).toBe("| x \\| y |\n| ------ |");
  });

  it("keeps a backslash typed before a pipe from ending the cell", () => {
    const table = parseNote("| a   |\n| --- |").firstChild!;
    const cell = S.nodes.table_header.create(null, S.text("x \\| y"));
    const doc = S.nodes.doc.create(null, [
      table.copy(table.content.replaceChild(0, S.nodes.table_row.create(null, [cell]))),
    ]);
    const note = serializeNote(doc);
    expect(note).toBe("| x \\\\\\| y |\n| -------- |");
    expect(parseNote(note).textContent).toBe("x \\| y");
  });
});

describe("markers", () => {
  it("draws each token as the kind it is", () => {
    expect(markerKind(".")).toBe("bullet");
    expect(markerKind("*")).toBe("bullet");
    expect(markerKind("-")).toBe("dash");
    expect(markerKind("[]")).toBe("task");
    expect(markerKind("- [x]")).toBe("ticked");
    expect(markerKind("3.")).toBe("number");
    expect(markerKind("iv.")).toBe("number");
  });

  it("ticks the way the rendered note always did", () => {
    for (const token of ["[]", "[ ]", "[x]", "[X]", "- [ ]", "- [x]", "*  [ ]"]) {
      const line = `${token} task`;
      expect(`${toggleToken(token)} task`).toBe(toggleTaskAt(line, 0));
    }
  });

  it("continues a list the way Enter in the text always did", () => {
    expect(nextToken(".")).toBe(".");
    expect(nextToken("9.")).toBe("10.");
    expect(nextToken("iii.")).toBe("iv.");
    expect(nextToken("[x]")).toBe("[]");
    expect(nextToken("- [x]")).toBe("- [ ]");
  });
});
