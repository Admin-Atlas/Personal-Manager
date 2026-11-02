// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Tables in pinboard notes (#1030). A note stays plain Markdown, so a table is a GFM pipe table —
// the vault copy reads as a table anywhere outside the board, and nothing here needs a rich-text
// engine. What makes it usable rather than a wall of pipes is the editing around it, all pure text
// edits in the same shape as notesMarkdown's:
//
//   • insertTable — the size-picker's output: an empty grid on its own lines, caret in the first cell.
//   • moveCell — Tab / Shift+Tab hop between cells. Each hop also re-aligns the columns, so the raw
//     text stays a readable grid however unevenly the cells were typed. Tab past the last cell adds
//     a row, the way it does in Docs and Word.
//   • tableEnter — Enter adds a row below; Enter on an empty last row leaves the table.
//   • tableCellOffset — where in the source a rendered cell's text starts, so clicking a cell of the
//     idle note opens the editor right there.
//
// Only a block GFM itself would render as a table counts: a pipe row followed by a delimiter row,
// then the pipe rows under it. A lone line that happens to start with "|" is prose. Fenced code is
// skipped, for the reason notesMarkdown's fenceMask gives: inside a fence a line means only itself.

import { fenceMask } from "./noteFences";
import type { TextEdit } from "./notesMarkdown";

/** The size picker's bounds: columns × rows, the header row included. */
export const TABLE_PICKER_COLS = 8;
export const TABLE_PICKER_ROWS = 8;

export type Align = "none" | "left" | "center" | "right";

/** One table in a note: its line span (`end` exclusive) and its cells, header first. The delimiter
 *  row is not in `rows`; its alignments are kept apart so a re-format writes them back. */
export interface TableBlock {
  start: number;
  end: number;
  rows: string[][];
  aligns: Align[];
}

const ROW_RE = /^ {0,3}\|/;
const DELIM_CELL_RE = /^:?-+:?$/;

/** A row's cells: the leading pipe (and a trailing one, when typed) dropped, split on every pipe
 *  that isn't escaped, each trimmed. An escaped `\|` is cell text, not a boundary. */
function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|") && !s.endsWith("\\|")) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = "";
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "\\" && s[i + 1] === "|") {
      cur += "\\|";
      i++;
    } else if (s[i] === "|") {
      cells.push(cur.trim());
      cur = "";
    } else {
      cur += s[i];
    }
  }
  cells.push(cur.trim());
  return cells;
}

function delimAligns(line: string): Align[] | null {
  if (!ROW_RE.test(line)) return null;
  const cells = splitRow(line);
  if (cells.length === 0 || !cells.every((c) => DELIM_CELL_RE.test(c))) return null;
  return cells.map((c) => {
    const l = c.startsWith(":");
    const r = c.endsWith(":");
    return l && r ? "center" : r ? "right" : l ? "left" : "none";
  });
}

/** Every table in the note, in order. */
export function findTables(lines: readonly string[]): TableBlock[] {
  const code = fenceMask(lines);
  const out: TableBlock[] = [];
  for (let i = 0; i + 1 < lines.length; i++) {
    if (code[i] || code[i + 1] || !ROW_RE.test(lines[i])) continue;
    const aligns = delimAligns(lines[i + 1]);
    if (!aligns) continue;
    let end = i + 2;
    while (end < lines.length && !code[end] && ROW_RE.test(lines[end])) end++;
    const rows = [lines[i], ...lines.slice(i + 2, end)].map(splitRow);
    out.push({ start: i, end, rows, aligns });
    i = end - 1;
  }
  return out;
}

/** Which source lines belong to a table — for toRenderMarkdown, which must pass them through
 *  untouched (its prose rule would append a hard break to every row). */
export function tableLineMask(lines: readonly string[]): boolean[] {
  const mask = lines.map(() => false);
  for (const t of findTables(lines)) for (let i = t.start; i < t.end; i++) mask[i] = true;
  return mask;
}

const width = (s: string) => [...s].length;

/** Lay a table out as aligned source lines, and say where each cell's text starts: `at(r, c)` is
 *  the offset into the returned text (r = 0 is the header row). Columns are as wide as their widest
 *  cell, three at least, so an empty grid still reads as one. */
export function layout(
  rows: string[][],
  aligns: Align[],
): { text: string; at: (r: number, c: number) => number } {
  const cols = Math.max(aligns.length, ...rows.map((r) => r.length));
  const cell = (r: string[], c: number) => r[c] ?? "";
  const widths = Array.from({ length: cols }, (_, c) =>
    Math.max(3, ...rows.map((r) => width(cell(r, c)))),
  );
  const pad = (s: string, w: number) => s + " ".repeat(w - width(s));
  const line = (r: string[]) => `| ${widths.map((w, c) => pad(cell(r, c), w)).join(" | ")} |`;
  const delim = `| ${widths
    .map((w, c) => {
      const a = aligns[c] ?? "none";
      if (a === "center") return `:${"-".repeat(w - 2)}:`;
      if (a === "left") return `:${"-".repeat(w - 1)}`;
      if (a === "right") return `${"-".repeat(w - 1)}:`;
      return "-".repeat(w);
    })
    .join(" | ")} |`;
  const lines = [line(rows[0]), delim, ...rows.slice(1).map(line)];
  const lineStart: number[] = [];
  let off = 0;
  for (const l of lines) {
    lineStart.push(off);
    off += l.length + 1;
  }
  // `widths` counts code points and `.length` counts UTF-16 units; the offset into the line has to
  // be in units, so sum the cells actually written before this one.
  const at = (r: number, c: number) => {
    const li = r === 0 ? 0 : r + 1;
    let x = 2;
    for (let k = 0; k < c; k++) x += pad(cell(rows[r], k), widths[k]).length + 3;
    return lineStart[li] + x;
  };
  return { text: lines.join("\n"), at };
}

/** Line index of the character at `offset`, and where that line starts. */
function lineAt(value: string, offset: number): { line: number; start: number } {
  let line = 0;
  let start = 0;
  for (let i = 0; i < offset && i < value.length; i++) {
    if (value[i] === "\n") {
      line++;
      start = i + 1;
    }
  }
  return { line, start };
}

/** The table holding the caret, and the (row, column) the caret is in. The delimiter row counts as
 *  the header, since it has no cells of its own to be in. */
function locate(value: string, caret: number) {
  const lines = value.split("\n");
  const { line, start } = lineAt(value, caret);
  const table = findTables(lines).find((t) => line >= t.start && line < t.end);
  if (!table) return null;
  const row = line <= table.start + 1 ? 0 : line - table.start - 1;
  const before = lines[line].slice(0, caret - start);
  const pipes = before.replace(/\\\|/g, "").split("|").length - 1;
  const cols = Math.max(table.aligns.length, ...table.rows.map((r) => r.length));
  const col = Math.min(Math.max(pipes - 1, 0), cols - 1);
  return { lines, table, row, col, cols };
}

/** Replace a table's lines with a fresh layout and put the selection on cell (r, c)'s text. */
function rewrite(
  lines: string[],
  table: TableBlock,
  rows: string[][],
  r: number,
  c: number,
  select: boolean,
): TextEdit {
  const { text, at } = layout(rows, table.aligns);
  const before = lines.slice(0, table.start).join("\n");
  const prefix = table.start > 0 ? `${before}\n` : "";
  const after = lines.slice(table.end);
  const out = prefix + text + (after.length ? `\n${after.join("\n")}` : "");
  const pos = prefix.length + at(r, c);
  const len = select ? (rows[r][c] ?? "").length : 0;
  return { text: out, selStart: pos, selEnd: pos + len };
}

const padRow = (r: string[], cols: number) => [
  ...r,
  ...Array(Math.max(0, cols - r.length)).fill(""),
];

/** An empty `cols` × `rows` table (the header row is one of the `rows`) at the selection, on lines
 *  of its own, with a blank line either side wherever text would otherwise touch it. */
export function insertTable(
  value: string,
  selStart: number,
  selEnd: number,
  cols: number,
  rows: number,
): TextEdit {
  const grid = Array.from({ length: Math.max(1, rows) }, () => Array(Math.max(1, cols)).fill(""));
  const { text, at } = layout(grid, []);
  let head = value.slice(0, selStart);
  let tail = value.slice(selEnd);
  // A table must start a block: on a fresh line, and after a blank one when prose comes before it,
  // or GFM reads the prose line as part of something else.
  if (head.length) {
    const h = head.replace(/[ \t]+$/, "");
    head = h + (h.endsWith("\n\n") ? "" : h.endsWith("\n") ? "\n" : "\n\n");
  }
  // And end one: a line typed straight under the last row would join the table as another row.
  if (tail.length) {
    const t = tail.replace(/^[ \t]+/, "");
    tail = (t.startsWith("\n\n") ? "" : t.startsWith("\n") ? "\n" : "\n\n") + t;
  }
  const pos = head.length + at(0, 0);
  return { text: head + text + tail, selStart: pos, selEnd: pos };
}

/** Tab (`dir` 1) or Shift+Tab (-1) inside a table: re-align it and move to the next or previous
 *  cell, selecting its text. Tab in the last cell adds a row. Null when the caret isn't in a table,
 *  so the caller falls back to its ordinary Tab. */
export function moveCell(value: string, caret: number, dir: 1 | -1): TextEdit | null {
  const at = locate(value, caret);
  if (!at) return null;
  const { lines, table, row, col, cols } = at;
  const rows = table.rows.map((r) => padRow(r, cols));
  let r = row;
  let c = col + dir;
  if (c >= cols) {
    c = 0;
    r++;
    if (r >= rows.length) rows.push(Array(cols).fill(""));
  } else if (c < 0) {
    if (r === 0) c = 0;
    else {
      r--;
      c = cols - 1;
    }
  }
  return rewrite(lines, table, rows, r, c, true);
}

/** Enter inside a table: a new empty row under the caret's row (under the header from the header or
 *  delimiter row), caret in its first cell. On an empty LAST row it leaves instead: the row goes and
 *  the caret lands on a fresh line under the table. Null outside a table. */
export function tableEnter(value: string, caret: number): TextEdit | null {
  const at = locate(value, caret);
  if (!at) return null;
  const { lines, table, row, cols } = at;
  const rows = table.rows.map((r) => padRow(r, cols));
  const last = rows.length - 1;
  if (row > 0 && row === last && rows[row].every((c) => c === "")) {
    rows.pop();
    const { text } = layout(rows, table.aligns);
    const prefix = table.start > 0 ? `${lines.slice(0, table.start).join("\n")}\n` : "";
    const after = lines.slice(table.end);
    // A blank line under the table, then the caret's line, so what is typed next is prose.
    const rest = after.length && after[0].trim() === "" ? after.slice(1) : after;
    const out = `${prefix}${text}\n\n${rest.length ? `\n${rest.join("\n")}` : ""}`;
    const pos = prefix.length + text.length + 2;
    return { text: out, selStart: pos, selEnd: pos };
  }
  rows.splice(row + 1, 0, Array(cols).fill(""));
  return rewrite(lines, table, rows, row + 1, 0, false);
}

/** Where cell (`row`, `col`) of the note's `tableIndex`-th table starts in the source — row 0 is the
 *  header, matching an HTML table's `rowIndex`. Past the cell's opening pipe and one space. Null when
 *  there is no such table or cell (the rendered tables and the source disagree), so the caller just
 *  opens the editor. */
export function tableCellOffset(
  value: string,
  tableIndex: number,
  row: number,
  col: number,
): number | null {
  const lines = value.split("\n");
  const table = findTables(lines)[tableIndex];
  if (!table || row < 0 || row >= table.rows.length) return null;
  const li = row === 0 ? table.start : table.start + 1 + row;
  let start = 0;
  for (let i = 0; i < li; i++) start += lines[i].length + 1;
  const line = lines[li];
  let pipes = 0;
  for (let i = 0; i < line.length; i++) {
    if (line[i] === "\\" && line[i + 1] === "|") {
      i++;
      continue;
    }
    if (line[i] !== "|") continue;
    if (pipes === col) {
      const x = line[i + 1] === " " ? i + 2 : i + 1;
      return start + x;
    }
    pipes++;
  }
  return null;
}

/** The note with every table laid out afresh — so two notes that differ only in how their tables'
 *  columns are padded compare equal. The rich editor's round-trip check reads through this: column
 *  padding is spacing, not content, and re-padding is all the rich editor ever does to a table. */
export function normalizeTables(value: string): string {
  const lines = value.split("\n");
  const tables = findTables(lines);
  for (let k = tables.length - 1; k >= 0; k--) {
    const t = tables[k];
    const { text } = layout(t.rows, t.aligns);
    lines.splice(t.start, t.end - t.start, ...text.split("\n"));
  }
  return lines.join("\n");
}
