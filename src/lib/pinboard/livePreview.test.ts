// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { minimalChange, previewMarks, type PreviewMark } from "./livePreview";

const NONE = new Set<number>();

/** Each mark with the text it covers, for readable assertions. */
function show(text: string, active: ReadonlySet<number> = NONE) {
  return previewMarks(text, active).map((m: PreviewMark) =>
    m.kind === "line"
      ? `line ${m.line} ${m.cls}`
      : `${m.kind} ${JSON.stringify(text.slice(m.from, m.to))}${"cls" in m ? ` ${m.cls}` : ""}${"glyph" in m ? ` ${m.glyph}` : ""}`,
  );
}

describe("previewMarks: markers off the cursor's line", () => {
  it("draws the dialect's bullets, dashes and checkboxes as glyphs", () => {
    const marks = show(". milk\n- eggs\n[] call Sam\n[x] done");
    expect(marks).toContain('glyph "." bullet');
    expect(marks).toContain('glyph "-" dash');
    expect(marks).toContain('glyph "[]" box');
    expect(marks).toContain('glyph "[x]" ticked');
    expect(marks).toContain('style "done" done');
  });

  it("reads a literal GFM task as one checkbox", () => {
    expect(show("- [ ] ship it")).toContain('glyph "- [ ]" box');
  });

  it("hides heading and quote markers, and classes their lines", () => {
    const marks = show("## Plan\n> quoted");
    expect(marks).toContain("line 0 h2");
    expect(marks).toContain('hide "## "');
    expect(marks).toContain("line 1 quote");
    expect(marks).toContain('hide "> "');
  });

  it("keeps a number or roman numeral as its own label", () => {
    const marks = show("1. one\nii. two");
    expect(marks).toContain('style "1." marker');
    expect(marks).toContain('style "ii." marker');
    expect(marks.some((m) => m.startsWith("hide"))).toBe(false);
  });

  it("styles inline spans and hides their delimiters", () => {
    const marks = show("a **bold** and *it* and ~~gone~~ and `x*y*` and [PM](https://a.b)");
    expect(marks).toContain('style "bold" strong');
    expect(marks).toContain('style "it" em');
    expect(marks).toContain('style "gone" strike');
    expect(marks).toContain('style "x*y*" code');
    expect(marks).toContain('style "PM" link');
    expect(marks).toContain('hide "**"');
    expect(marks).toContain('hide "](https://a.b)"');
    // Nothing inside the code span is read as italic.
    expect(marks).not.toContain('style "y" em');
  });

  it("never reads bold's stars as italic", () => {
    const marks = show("**bold**");
    expect(marks.filter((m) => m.endsWith(" em"))).toEqual([]);
  });

  it("carries a link's address", () => {
    const link = previewMarks("[PM](https://a.b)", NONE).find(
      (m) => m.kind === "style" && m.cls === "link",
    );
    expect(link).toMatchObject({ href: "https://a.b" });
  });

  it("leaves fenced code alone", () => {
    const marks = show("```\n**not bold**\n. not a bullet\n```");
    expect(marks.filter((m) => !m.startsWith("line"))).toEqual([]);
    expect(marks).toContain("line 1 codeblock");
  });

  it("draws a table as one grid, with its order in the note", () => {
    const text = "| a | b |\n| --- | --- |\n| 1 | 2 |";
    const [table] = previewMarks(text, NONE).filter((m) => m.kind === "table");
    expect(table).toMatchObject({
      from: 0,
      to: text.length,
      index: 0,
      rows: [
        ["a", "b"],
        ["1", "2"],
      ],
    });
  });

  it("hides a horizontal rule's dashes and draws the rule as the line", () => {
    expect(show("---")).toEqual(["line 0 rule", 'hide "---"']);
  });
});

describe("previewMarks: the cursor's line", () => {
  it("shows its markers again, faintly, but keeps the formatting", () => {
    const marks = show("## **Plan**\n. milk", new Set([0, 1]));
    expect(marks).toContain("line 0 h2");
    expect(marks).toContain('style "## " marker');
    expect(marks).toContain('style "**" marker');
    expect(marks).toContain('style "Plan" strong');
    expect(marks).toContain('style "." marker');
    expect(marks.some((m) => m.startsWith("hide") || m.startsWith("glyph"))).toBe(false);
  });

  it("only touches the lines it's on", () => {
    const marks = show(". one\n. two", new Set([1]));
    expect(marks).toContain('glyph "." bullet');
    expect(marks).toContain('style "." marker');
  });

  it("turns a table it's in back into text, in a fixed-width font", () => {
    const marks = show("| a |\n| --- |\n| 1 |", new Set([2]));
    expect(marks).toEqual(["line 0 table-src", "line 1 table-src", "line 2 table-src"]);
  });
});

describe("minimalChange", () => {
  it("is the one span that differs", () => {
    expect(minimalChange("hello world", "hello brave world")).toEqual({
      from: 6,
      to: 6,
      insert: "brave ",
    });
    expect(minimalChange("abcdef", "aef")).toEqual({ from: 1, to: 4, insert: "" });
  });

  it("doesn't count repeated characters twice", () => {
    expect(minimalChange("aa", "aaa")).toEqual({ from: 2, to: 2, insert: "a" });
    expect(minimalChange("aaa", "aa")).toEqual({ from: 2, to: 3, insert: "" });
  });

  it("is empty for equal texts", () => {
    expect(minimalChange("same", "same")).toEqual({ from: 4, to: 4, insert: "" });
  });
});
