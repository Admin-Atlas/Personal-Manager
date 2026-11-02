// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What a pinboard note looks like while you type in it (Obsidian's "live preview"). The note is
// still one Markdown string, edited in place; this works out how to DRAW it: headings large, bold
// bold, the dialect's markers as the glyphs they stand for (". " a bullet, "- " a dash, "[] " a
// checkbox), tables as a grid. On the lines the cursor is on (`active`), the markers come back as
// faint text, so they can still be edited.
//
// Pure: text in, a flat list of marks out, with offsets into the text. NoteEditor turns them into
// CodeMirror decorations; keeping that glue thin is what lets this be tested without an editor.
//
// It reads the same dialect toRenderMarkdown does (matchMarker, fenceMask, findTables), so a line
// looks here the way it renders in the vault copy and the reader.

import { fenceMask } from "./noteFences";
import { isRomanToken, matchMarker } from "./notesMarkdown";
import { findTables } from "./noteTables";

export type PreviewMark =
  /** A class for a whole line (0-based line number). */
  | { kind: "line"; line: number; cls: string }
  /** Text not drawn at all (a marker off the cursor's line). */
  | { kind: "hide"; from: number; to: number }
  /** Text drawn as a glyph instead. */
  | { kind: "glyph"; from: number; to: number; glyph: "bullet" | "dash" | "box" | "ticked" }
  /** Inline styling. A link carries its address. */
  | { kind: "style"; from: number; to: number; cls: StyleClass; href?: string }
  /** A whole table, drawn as a grid while the cursor is outside it. `index` is its order in the
   *  note, which `tableCellOffset` takes. */
  | { kind: "table"; from: number; to: number; rows: string[][]; index: number };

export type StyleClass = "strong" | "em" | "strike" | "code" | "link" | "marker" | "done";

const HEADING_RE = /^( {0,3})(#{1,6})(\s+)/;
const RULE_RE = /^ {0,3}([-*_])( *\1){2,} *$/;
const TASK_RE = /^\[([ xX]?)\](\s|$)/;

/** The marks for `text`, given the lines the cursor or selection is on. */
export function previewMarks(text: string, active: ReadonlySet<number>): PreviewMark[] {
  const lines = text.split("\n");
  const code = fenceMask(lines);
  const starts: number[] = [];
  let off = 0;
  for (const l of lines) {
    starts.push(off);
    off += l.length + 1;
  }
  const out: PreviewMark[] = [];

  const inTable = lines.map(() => false);
  findTables(lines).forEach((t, index) => {
    for (let i = t.start; i < t.end; i++) inTable[i] = true;
    let editing = false;
    for (let i = t.start; i < t.end; i++) if (active.has(i)) editing = true;
    if (editing) {
      for (let i = t.start; i < t.end; i++) out.push({ kind: "line", line: i, cls: "table-src" });
    } else {
      const last = t.end - 1;
      out.push({
        kind: "table",
        from: starts[t.start],
        to: starts[last] + lines[last].length,
        rows: t.rows,
        index,
      });
    }
  });

  lines.forEach((line, i) => {
    if (inTable[i]) return;
    const s = starts[i];
    const on = active.has(i);
    if (code[i]) {
      out.push({ kind: "line", line: i, cls: "codeblock" });
      return;
    }
    if (RULE_RE.test(line)) {
      out.push({ kind: "line", line: i, cls: "rule" });
      if (!on) out.push({ kind: "hide", from: s, to: s + line.length });
      else out.push({ kind: "style", from: s, to: s + line.length, cls: "marker" });
      return;
    }
    // Where the line's own text starts, after any marker the line is drawn without.
    let body = 0;
    const h = HEADING_RE.exec(line);
    if (h) {
      out.push({ kind: "line", line: i, cls: `h${h[2].length}` });
      body = h[0].length;
      marker(out, s + h[1].length, s + body, on);
    } else {
      const m = matchMarker(line);
      if (m) {
        const at = m.indent.length;
        const tokenEnd = at + m.token.length;
        body = line.length - m.content.length;
        const box = /^\[([ xX]?)\]$/.exec(m.token);
        // A literal GFM task, "- [ ] x", is a checkbox too: the bullet and the brackets are one marker.
        const gfm = !box && "-+*".includes(m.token) ? TASK_RE.exec(m.content) : null;
        if (box || gfm) {
          const ticked = (box ? box[1] : gfm![1]).toLowerCase() === "x";
          const to = box ? s + tokenEnd : s + body + gfm![0].trimEnd().length;
          if (gfm) body += gfm[0].length;
          out.push({ kind: "line", line: i, cls: "li" });
          if (on) out.push({ kind: "style", from: s + at, to, cls: "marker" });
          else out.push({ kind: "glyph", from: s + at, to, glyph: ticked ? "ticked" : "box" });
          if (ticked && body < line.length)
            out.push({ kind: "style", from: s + body, to: s + line.length, cls: "done" });
        } else if (m.token === ">") {
          out.push({ kind: "line", line: i, cls: "quote" });
          marker(out, s + at, s + body, on);
        } else if (m.token === "." || m.token === "*" || m.token === "-" || m.token === "+") {
          out.push({ kind: "line", line: i, cls: "li" });
          const glyph = m.token === "." || m.token === "*" ? "bullet" : "dash";
          if (on) out.push({ kind: "style", from: s + at, to: s + tokenEnd, cls: "marker" });
          else out.push({ kind: "glyph", from: s + at, to: s + tokenEnd, glyph });
        } else if (/^\d+\.$/.test(m.token) || isRomanToken(m.token)) {
          // A number or a roman numeral IS its own label: it stays, drawn as a list marker.
          out.push({ kind: "line", line: i, cls: "li" });
          out.push({ kind: "style", from: s + at, to: s + tokenEnd, cls: "marker" });
        }
      }
    }
    inline(out, line.slice(body), s + body, on);
  });
  return out;
}

/** A line's leading marker: hidden off the cursor's line, faint on it. */
function marker(out: PreviewMark[], from: number, to: number, on: boolean) {
  if (to <= from) return;
  out.push(on ? { kind: "style", from, to, cls: "marker" } : { kind: "hide", from, to });
}

// Inline spans, in the order they claim text: code first (nothing inside it is formatting), then
// links, bold, strikethrough, italic. Each claimed delimiter is blanked in `scan` so a later pattern
// can't reuse it — "**" must never read as two italic stars.
const INLINE: {
  re: RegExp;
  cls: StyleClass;
  open: number;
  close: (m: RegExpExecArray) => number;
}[] = [
  { re: /\[([^\]\n]+)\]\(([^)\s]+)\)/g, cls: "link", open: 1, close: (m) => m[2].length + 3 },
  { re: /(\*\*|__)(?=\S)(.+?)(?<=\S)\1/g, cls: "strong", open: 2, close: () => 2 },
  { re: /~~(?=\S)(.+?)(?<=\S)~~/g, cls: "strike", open: 2, close: () => 2 },
  {
    re: /(?<![\w*])\*(?=[^\s*])(.+?)(?<=[^\s*])\*(?![\w*])|(?<![\w_])_(?=[^\s_])(.+?)(?<=[^\s_])_(?![\w_])/g,
    cls: "em",
    open: 1,
    close: () => 1,
  },
];

/** One inline span in a line: its content (`from`–`to`) and the delimiter text either side of it
 *  (`delim` opens, `end` closes), offsets into the line. The rich note editor parses with the same
 *  spans, so a line formats identically in both editors. */
export interface InlineSpan {
  cls: "strong" | "em" | "strike" | "code" | "link";
  from: number;
  to: number;
  delim: string;
  end: string;
  href?: string;
}

export function inlineSpans(text: string): InlineSpan[] {
  const blank = (s: string, from: number, to: number) =>
    s.slice(0, from) + "\u0000".repeat(to - from) + s.slice(to);
  const out: InlineSpan[] = [];
  let scan = text;
  for (const m of text.matchAll(/(`+)([^`]|[^`].*?[^`])\1(?!`)/g)) {
    const from = m.index;
    const to = from + m[0].length;
    const tick = m[1].length;
    out.push({ cls: "code", from: from + tick, to: to - tick, delim: m[1], end: m[1] });
    scan = blank(scan, from, to);
  }
  for (const { re, cls, open, close } of INLINE) {
    for (const m of scan.matchAll(re)) {
      const from = m.index;
      const to = from + m[0].length;
      const inner = [from + open, to - close(m)] as const;
      out.push({
        cls: cls as InlineSpan["cls"],
        from: inner[0],
        to: inner[1],
        delim: text.slice(from, inner[0]),
        end: text.slice(inner[1], to),
        ...(cls === "link" ? { href: m[2] } : {}),
      });
      // A link's text is not re-read as formatting of the line around it, but stays readable for
      // emphasis inside it — only its brackets and address are claimed.
      scan = blank(scan, from, inner[0]);
      scan = blank(scan, inner[1], to);
    }
  }
  return out;
}

function inline(out: PreviewMark[], text: string, base: number, on: boolean) {
  for (const sp of inlineSpans(text)) {
    const a = base + sp.from - sp.delim.length;
    delims(out, a, base + sp.from, base + sp.to, base + sp.to + sp.end.length, on);
    out.push({
      kind: "style",
      from: base + sp.from,
      to: base + sp.to,
      cls: sp.cls,
      ...(sp.href != null ? { href: sp.href } : {}),
    });
  }
}

/** An inline span's two delimiters: hidden off the cursor's line, faint on it. */
function delims(out: PreviewMark[], a: number, b: number, c: number, d: number, on: boolean) {
  marker(out, a, b, on);
  marker(out, c, d, on);
}

/** The smallest single change that turns `from` into `to`: the editor applies an outside edit (an
 *  undo, a sync) as this, so the cursor is carried across it instead of thrown to the end. */
export function minimalChange(
  from: string,
  to: string,
): { from: number; to: number; insert: string } {
  let a = 0;
  while (a < from.length && a < to.length && from[a] === to[a]) a++;
  let b = 0;
  while (
    b < from.length - a &&
    b < to.length - a &&
    from[from.length - 1 - b] === to[to.length - 1 - b]
  )
    b++;
  return { from: a, to: from.length - b, insert: to.slice(a, to.length - b) };
}
