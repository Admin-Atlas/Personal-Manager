// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { insertTable, moveCell, tableCellOffset, tableEnter, tableLineMask } from "./noteTables";
import { toRenderMarkdown } from "./notesMarkdown";

const GRID_2x3 = ["|     |     |", "| --- | --- |", "|     |     |", "|     |     |"].join("\n");

describe("insertTable", () => {
  it("writes an empty grid with the header row counted, caret in the first cell", () => {
    const r = insertTable("", 0, 0, 2, 3);
    expect(r.text).toBe(GRID_2x3);
    expect(r.selStart).toBe(2);
    expect(r.selEnd).toBe(2);
  });

  it("puts a blank line between the table and the text either side", () => {
    const r = insertTable("before\nafter", 7, 7, 2, 1);
    expect(r.text).toBe("before\n\n|     |     |\n| --- | --- |\n\nafter");
  });

  it("starts on its own line from mid-line, and replaces a selection", () => {
    const r = insertTable("one two", 3, 7, 1, 1);
    expect(r.text).toBe("one\n\n|     |\n| --- |");
  });

  it("doesn't stack blank lines that are already there", () => {
    const r = insertTable("a\n\n", 3, 3, 1, 1);
    expect(r.text).toBe("a\n\n|     |\n| --- |");
  });
});

describe("moveCell", () => {
  const table = ["| a | bb |", "|---|---|", "| ccc | d |"].join("\n");

  it("moves to the next cell, aligning the columns and selecting the cell's text", () => {
    const r = moveCell(table, 2, 1)!;
    expect(r.text).toBe(["| a   | bb  |", "| --- | --- |", "| ccc | d   |"].join("\n"));
    expect(r.text.slice(r.selStart, r.selEnd)).toBe("bb");
  });

  it("wraps from a row's last cell to the next row's first", () => {
    const r = moveCell(table, 7, 1)!;
    expect(r.text.slice(r.selStart, r.selEnd)).toBe("ccc");
  });

  it("adds a row from the very last cell", () => {
    const r = moveCell(table, table.length - 2, 1)!;
    expect(r.text.split("\n")).toHaveLength(4);
    expect(r.text.split("\n")[3]).toBe("|     |     |");
    expect(r.selStart).toBe(r.text.lastIndexOf("\n") + 3);
  });

  it("goes back with Shift+Tab, and stays put in the first cell", () => {
    const back = moveCell(table, table.indexOf("ccc"), -1)!;
    expect(back.text.slice(back.selStart, back.selEnd)).toBe("bb");
    const first = moveCell(table, 2, -1)!;
    expect(first.text.slice(first.selStart, first.selEnd)).toBe("a");
  });

  it("keeps the delimiter row's alignment", () => {
    const r = moveCell("| a | b |\n|:-:|--:|\n| 1 | 2 |", 2, 1)!;
    expect(r.text.split("\n")[1]).toBe("| :-: | --: |");
  });

  it("is null outside a table, so Tab keeps indenting", () => {
    expect(moveCell("| not a table", 3, 1)).toBeNull();
    expect(moveCell("text\n\n" + table, 1, 1)).toBeNull();
  });

  it("ignores a table inside fenced code", () => {
    expect(moveCell("```\n" + table + "\n```", 6, 1)).toBeNull();
  });

  it("keeps the text around the table", () => {
    const r = moveCell(`top\n\n${table}\n\nend`, 7, 1)!;
    expect(r.text.startsWith("top\n\n| a   |")).toBe(true);
    expect(r.text.endsWith("\n\nend")).toBe(true);
  });

  it("treats an escaped pipe as cell text", () => {
    const r = moveCell("| a \\| b | c |\n|---|---|", 2, 1)!;
    expect(r.text.split("\n")[0]).toBe("| a \\| b | c   |");
    expect(r.text.slice(r.selStart, r.selEnd)).toBe("c");
  });
});

describe("tableEnter", () => {
  it("adds an empty row under the caret's row", () => {
    const r = tableEnter(GRID_2x3, 2)!;
    expect(r.text.split("\n")).toHaveLength(5);
    expect(r.selStart).toBe(r.text.indexOf("\n", r.text.indexOf("---")) + 3);
  });

  it("leaves the table from an empty last row", () => {
    const r = tableEnter(`${GRID_2x3}\n\nafter`, GRID_2x3.length - 3)!;
    expect(r.text).toBe(
      ["|     |     |", "| --- | --- |", "|     |     |", "", "", "after"].join("\n"),
    );
    expect(r.text.slice(r.selStart)).toBe("\nafter");
  });

  it("adds a row rather than leaving when the last row has text", () => {
    const filled = "| a |\n| --- |\n| b |";
    expect(tableEnter(filled, filled.length - 2)!.text.split("\n")).toHaveLength(4);
  });

  it("is null outside a table", () => {
    expect(tableEnter("just text", 4)).toBeNull();
  });
});

describe("tableCellOffset", () => {
  const note = "intro\n\n| a | b |\n| --- | --- |\n| c | d |\n\n| x |\n| --- |";

  it("finds a cell of the nth table by rendered row and column", () => {
    expect(note.slice(tableCellOffset(note, 0, 1, 1)!)).toMatch(/^d \|/);
    expect(note.slice(tableCellOffset(note, 0, 0, 0)!)).toMatch(/^a \|/);
    expect(note.slice(tableCellOffset(note, 1, 0, 0)!)).toMatch(/^x \|/);
  });

  it("is null for a table or row that isn't there", () => {
    expect(tableCellOffset(note, 2, 0, 0)).toBeNull();
    expect(tableCellOffset(note, 0, 5, 0)).toBeNull();
  });
});

describe("tables through toRenderMarkdown", () => {
  it("passes rows through untouched, without the prose hard break", () => {
    expect(toRenderMarkdown(GRID_2x3)).toBe(GRID_2x3);
  });

  it("separates a table from prose touching either side", () => {
    expect(toRenderMarkdown("above\n| a |\n| --- |\nbelow")).toBe(
      "above  \n\n| a |\n| --- |\n\nbelow  ",
    );
  });

  it("is idempotent", () => {
    const once = toRenderMarkdown("above\n| a |\n| --- |\n| b |\nbelow");
    expect(toRenderMarkdown(once)).toBe(once);
  });

  it("leaves a lone pipe line as prose", () => {
    expect(tableLineMask(["| just a line"])).toEqual([false]);
    expect(toRenderMarkdown("| just a line")).toBe("| just a line  ");
  });
});
