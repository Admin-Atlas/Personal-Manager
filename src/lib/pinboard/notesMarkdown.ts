// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pinboard notes are authored as Markdown and rendered through the app's single sanitizing
// <Markdown> boundary (src/lib/markdown.tsx). Two pure, UI-free helpers back that:
//
//   • continueList — smart list continuation while typing. On Enter it continues the current
//     list (next bullet, incremented number, next roman numeral, a fresh checkbox) or, on an
//     empty item, clears the marker to exit the list — so a note behaves like a plain-text
//     list editor without a rich-text engine.
//   • toRenderMarkdown — normalise the note's shorthand marker dialect to the GFM the renderer
//     understands, leaving everything else byte-for-byte.
//
// Supported line markers (each needs a trailing space): "." and "-" bullets, "1." numbered,
// "i." roman, ">" arrow/quote, "[]" checkbox (also "[x]"). "." and "-" both render as bullets
// (GFM has no distinct dot marker); ">" renders as a blockquote (its native Markdown meaning);
// roman items keep their exact labels. These fidelity choices are documented, not accidental.

/** One marker match on a single line: the leading indent, the marker token (no trailing space),
 *  and the content after it. */
interface MarkerMatch {
  indent: string;
  token: string;
  content: string;
}

// A line is a list item when it is: optional indent, a marker token, at least one space, then
// content. The alternation order matters — "." must be tried before "\d+\." / roman so a bare
// dot bullet isn't mis-read, and digits before roman letters.
const MARKER_RE = /^(\s*)(-|\.|>|\[[ xX]?\]|\d+\.|[ivxlcdmIVXLCDM]+\.)\s+(.*)$/;

function matchMarker(line: string): MarkerMatch | null {
  const m = MARKER_RE.exec(line);
  if (!m) return null;
  return { indent: m[1], token: m[2], content: m[3] };
}

// --- roman numerals (small range; list depth never approaches the cap) ---------------------

const ROMAN: ReadonlyArray<readonly [number, string]> = [
  [1000, "m"],
  [900, "cm"],
  [500, "d"],
  [400, "cd"],
  [100, "c"],
  [90, "xc"],
  [50, "l"],
  [40, "xl"],
  [10, "x"],
  [9, "ix"],
  [5, "v"],
  [4, "iv"],
  [1, "i"],
];

function toRoman(n: number): string {
  if (n < 1 || n > 3999) return String(n);
  let out = "";
  for (const [v, s] of ROMAN) {
    while (n >= v) {
      out += s;
      n -= v;
    }
  }
  return out;
}

/** Parse a canonical lowercase-or-uppercase roman numeral, or null if it isn't one — the
 *  round-trip check rejects non-canonical letter runs (e.g. "iic") so ordinary words that
 *  happen to be roman letters aren't treated as list markers. */
function fromRoman(s: string): number | null {
  const lower = s.toLowerCase();
  if (!/^[ivxlcdm]+$/.test(lower)) return null;
  let n = 0;
  let i = 0;
  for (const [v, sym] of ROMAN) {
    while (lower.startsWith(sym, i)) {
      n += v;
      i += sym.length;
    }
  }
  return i === lower.length && toRoman(n) === lower ? n : null;
}

function isRomanToken(token: string): boolean {
  return /^[ivxlcdmIVXLCDM]+\.$/.test(token) && fromRoman(token.slice(0, -1)) != null;
}

/** The marker that should begin the NEXT item after one whose token is `token`. */
function nextMarker(token: string): string | null {
  if (token === "-" || token === "." || token === ">") return `${token} `;
  if (/^\[[ xX]?\]$/.test(token)) return "[] "; // a continued checkbox is always fresh/unchecked
  if (/^\d+\.$/.test(token)) return `${parseInt(token, 10) + 1}. `;
  if (isRomanToken(token)) {
    const val = fromRoman(token.slice(0, -1));
    if (val == null) return null;
    const r = toRoman(val + 1);
    return `${token === token.toUpperCase() ? r.toUpperCase() : r}. `;
  }
  return null;
}

export interface ListEdit {
  text: string;
  caret: number;
}

/**
 * Compute the result of pressing Enter inside a note textarea at a collapsed caret, list-aware:
 *  - on a list item with content → continue with the next marker on a new line;
 *  - on an empty list item (just the marker) → clear the marker and exit the list;
 *  - anywhere else → return null (the caller lets a plain newline happen).
 */
export function continueList(value: string, caret: number): ListEdit | null {
  const lineStart = value.lastIndexOf("\n", caret - 1) + 1;
  const nextNl = value.indexOf("\n", caret);
  const lineEnd = nextNl === -1 ? value.length : nextNl;
  const line = value.slice(lineStart, lineEnd);

  const m = matchMarker(line);
  if (!m) return null;
  const next = nextMarker(m.token);
  if (next == null) return null; // matched a non-canonical roman token — leave it alone

  if (m.content.trim() === "") {
    // Empty item: drop the marker and exit the list (no new bullet).
    return { text: value.slice(0, lineStart) + value.slice(lineEnd), caret: lineStart };
  }

  const insert = `\n${m.indent}${next}`;
  return {
    text: value.slice(0, caret) + insert + value.slice(caret),
    caret: caret + insert.length,
  };
}

/**
 * Normalise a note's shorthand marker dialect to GFM for the shared <Markdown> renderer:
 *  - "[]" / "[x]" → GFM task-list items ("- [ ]" / "- [x]");
 *  - "." bullets → "-" bullets (GFM has no separate dot marker);
 *  - roman items keep their label but gain a hard line break so a run stays multi-line and
 *    isn't collapsed into one paragraph (GFM merges single newlines);
 *  - "-", "1.", ">" already render natively (bullet, ordered list, blockquote) — untouched.
 * Every non-marker line passes through byte-for-byte.
 */
export function toRenderMarkdown(raw: string): string {
  return raw
    .split("\n")
    .map((line) => {
      const m = matchMarker(line);
      if (!m) return line;
      const { indent, token, content } = m;

      const cb = /^\[([ xX]?)\]$/.exec(token);
      if (cb) return `${indent}- [${cb[1].toLowerCase() === "x" ? "x" : " "}] ${content}`;

      if (token === ".") return `${indent}- ${content}`;

      // Two trailing spaces = a Markdown hard break, so consecutive roman items each keep their
      // own line (and their exact "i."/"ii." labels) instead of merging.
      if (isRomanToken(token)) return `${indent}${token} ${content}  `;

      return line;
    })
    .join("\n");
}

// --- toolbar / keyboard formatting helpers (pure) -------------------------------------------
// Back the note editor's formatting buttons and their keyboard shortcuts. Each returns the new
// text plus the selection to restore, so the caller round-trips through the controlled <textarea>
// without touching the DOM. They emit the note's shorthand dialect so continueList/toRenderMarkdown
// keep working.

export interface TextEdit {
  text: string;
  selStart: number;
  selEnd: number;
}

export type LineMarkerKind = "bullet" | "number" | "checkbox" | "heading" | "quote";

/** The marker each kind writes (number counts up per line, so it's templated below). */
const LINE_PREFIX: Record<Exclude<LineMarkerKind, "number">, string> = {
  bullet: "- ",
  checkbox: "[] ",
  heading: "# ",
  quote: "> ",
};

/** Length of the given kind's marker at the start of a line (incl. indent + trailing space), or 0. */
const MARKER_LEN_RE: Record<LineMarkerKind, RegExp> = {
  bullet: /^(\s*)- /,
  number: /^(\s*)\d+\. /,
  checkbox: /^(\s*)\[[ xX]?\] /,
  heading: /^(\s*)#{1,6} /,
  quote: /^(\s*)> /,
};

function markerLen(line: string, kind: LineMarkerKind): number {
  return MARKER_LEN_RE[kind].exec(line)?.[0].length ?? 0;
}

/** Split a line into its indent and its content, stripping a leading heading OR any list marker so
 *  one kind can be swapped for another cleanly. */
function splitMarker(line: string): { indent: string; content: string } {
  const indent = /^\s*/.exec(line)?.[0] ?? "";
  const rest = line.slice(indent.length);
  const heading = /^#{1,6}\s+/.exec(rest);
  if (heading) return { indent, content: rest.slice(heading[0].length) };
  const m = matchMarker(line);
  if (m) return { indent: m.indent, content: m.content };
  return { indent, content: rest };
}

/** The block of whole lines a selection touches, plus whether the selection was collapsed. A
 *  selection ending exactly at a line boundary doesn't pull the next line in (standard editor
 *  convention). Shared by the line-marker toggle and the indent/outdent helpers. */
function selectedLines(
  value: string,
  selStart: number,
  selEnd: number,
): { blockStart: number; blockEnd: number; lines: string[]; collapsed: boolean } {
  const collapsed = selStart === selEnd;
  const blockStart = value.lastIndexOf("\n", selStart - 1) + 1;
  const scanFrom = !collapsed && value[selEnd - 1] === "\n" ? selEnd - 1 : selEnd;
  const nextNl = value.indexOf("\n", scanFrom);
  const blockEnd = nextNl === -1 ? value.length : nextNl;
  return { blockStart, blockEnd, lines: value.slice(blockStart, blockEnd).split("\n"), collapsed };
}

/**
 * Toggle a line-level marker across every line the selection touches. If all non-blank lines already
 * carry the marker it's removed; otherwise any existing marker is swapped for this one (indent kept).
 */
export function applyLineMarker(
  value: string,
  selStart: number,
  selEnd: number,
  kind: LineMarkerKind,
): TextEdit {
  const { blockStart, blockEnd, lines, collapsed } = selectedLines(value, selStart, selEnd);

  const nonBlank = lines.filter((l) => l.trim() !== "");
  const allMarked = nonBlank.length > 0 && nonBlank.every((l) => markerLen(l, kind) > 0);
  // Skip blank lines only when the block has real content (don't mark the gaps in a multi-line
  // selection). If the whole block is blank — an empty note or an empty line — still add the marker so
  // a list can be STARTED there; otherwise the button/shortcut would be a dead no-op.
  const skipBlank = nonBlank.length > 0;

  let n = 1;
  const out = lines.map((line) => {
    if (skipBlank && line.trim() === "") return line;
    if (allMarked) {
      const indent = /^\s*/.exec(line)?.[0] ?? "";
      return indent + line.slice(markerLen(line, kind));
    }
    const { indent, content } = splitMarker(line);
    const prefix = kind === "number" ? `${n++}. ` : LINE_PREFIX[kind];
    return `${indent}${prefix}${content}`;
  });

  const block = out.join("\n");
  return {
    text: value.slice(0, blockStart) + block + value.slice(blockEnd),
    // A collapsed caret becomes a caret at the end of the (now-marked) line so typing continues
    // naturally; a real selection stays selected over the affected block.
    selStart: collapsed ? blockStart + block.length : blockStart,
    selEnd: blockStart + block.length,
  };
}

// --- indent / outdent (pure) ----------------------------------------------------------------
// Back the note editor's Tab / Shift+Tab / Backspace behaviour. One indent level = two spaces —
// enough to nest a task/list item under the one above once rendered. `continueList` already
// carries a line's indent to the next item, so once a checkbox is indented the ones typed after
// it inherit that indent automatically.

/** One indentation level. Two spaces nests a list item under its parent in GFM. */
const INDENT = "  ";

/** Strip one indent level (a leading tab, or up to two leading spaces) from a line. */
function stripOneIndent(line: string): { line: string; removed: number } {
  if (line.startsWith("\t")) return { line: line.slice(1), removed: 1 };
  let n = 0;
  while (n < INDENT.length && line[n] === " ") n++;
  return { line: line.slice(n), removed: n };
}

/** Indent every line the selection touches by one level (blank lines left alone). */
export function indentLines(value: string, selStart: number, selEnd: number): TextEdit {
  const { blockStart, blockEnd, lines, collapsed } = selectedLines(value, selStart, selEnd);
  const out = lines.map((l) => (l.trim() === "" ? l : INDENT + l));
  const block = out.join("\n");
  const firstAdded = lines[0].trim() === "" ? 0 : INDENT.length;
  return {
    text: value.slice(0, blockStart) + block + value.slice(blockEnd),
    // A collapsed caret sits on the first line → nudge it past the spaces just inserted; a real
    // selection re-covers the whole (now-indented) block.
    selStart: collapsed ? selStart + firstAdded : blockStart,
    selEnd: collapsed ? selStart + firstAdded : blockStart + block.length,
  };
}

/** Outdent every line the selection touches by one level (a no-op on already-flush lines). */
export function outdentLines(value: string, selStart: number, selEnd: number): TextEdit {
  const { blockStart, blockEnd, lines, collapsed } = selectedLines(value, selStart, selEnd);
  let firstRemoved = 0;
  const out = lines.map((l, i) => {
    const { line, removed } = stripOneIndent(l);
    if (i === 0) firstRemoved = removed;
    return line;
  });
  const block = out.join("\n");
  const caret = Math.max(blockStart, selStart - firstRemoved);
  return {
    text: value.slice(0, blockStart) + block + value.slice(blockEnd),
    selStart: collapsed ? caret : blockStart,
    selEnd: collapsed ? caret : blockStart + block.length,
  };
}

/** If a collapsed caret sits inside the leading indent of an indented list item, that indent's
 *  length; else null — so the editor can turn Backspace-in-indent into an outdent. */
export function listIndentBeforeCaret(value: string, caret: number): number | null {
  const lineStart = value.lastIndexOf("\n", caret - 1) + 1;
  const before = value.slice(lineStart, caret);
  if (before.length === 0 || before.trim() !== "") return null; // caret isn't within pure indent
  const nextNl = value.indexOf("\n", caret);
  const line = value.slice(lineStart, nextNl === -1 ? value.length : nextNl);
  const m = matchMarker(line);
  return m && m.indent.length > 0 ? m.indent.length : null;
}

/**
 * Toggle an inline wrap (bold `**`, italic `*`, code `` ` ``) around the selection. Unwraps when
 * already wrapped; with a collapsed caret, inserts the pair and puts the caret between.
 */
export function toggleWrap(
  value: string,
  selStart: number,
  selEnd: number,
  delim: string,
): TextEdit {
  const inner = value.slice(selStart, selEnd);
  const before = value.slice(Math.max(0, selStart - delim.length), selStart);
  const after = value.slice(selEnd, selEnd + delim.length);
  // Only treat the selection as already-wrapped when EXACTLY `delim` sits on each side — not when
  // those chars belong to a longer run of the same marker (e.g. italic `*` selected inside bold
  // `**foo**`, where a naive unwrap would strip one of the bold asterisks). Guard against a same-char
  // neighbour just outside the candidate delimiters.
  const runChar = delim[0];
  const outerBefore = value[selStart - delim.length - 1] ?? "";
  const outerAfter = value[selEnd + delim.length] ?? "";
  if (before === delim && after === delim && outerBefore !== runChar && outerAfter !== runChar) {
    return {
      text: value.slice(0, selStart - delim.length) + inner + value.slice(selEnd + delim.length),
      selStart: selStart - delim.length,
      selEnd: selEnd - delim.length,
    };
  }
  if (inner) {
    return {
      text: value.slice(0, selStart) + delim + inner + delim + value.slice(selEnd),
      selStart: selStart + delim.length,
      selEnd: selEnd + delim.length,
    };
  }
  const pos = selStart + delim.length;
  return {
    text: value.slice(0, selStart) + delim + delim + value.slice(selEnd),
    selStart: pos,
    selEnd: pos,
  };
}

/**
 * Where to put the caret after an undo/redo swaps a textarea's text out from under it.
 *
 * Restoring the string but leaving the caret at the end feels broken — you undo three seconds of
 * typing mid-paragraph and land at the bottom of the note. The edit is bracketed by whatever the two
 * versions still share, so the caret belongs at the end of the common PREFIX: that's the point the
 * two texts start to differ, i.e. where the change was.
 *
 * The common SUFFIX is what makes it right for a deletion as well as an insertion, and it must be
 * bounded so the two runs can't overlap and count the same characters twice (they would on repeated
 * characters — "aa" → "aaa" shares a 2-char prefix AND a 2-char suffix of a 3-char string).
 */
export function caretForRestore(from: string, to: string): number {
  let prefix = 0;
  const max = Math.min(from.length, to.length);
  while (prefix < max && from[prefix] === to[prefix]) prefix++;
  let suffix = 0;
  while (suffix < max - prefix && from[from.length - 1 - suffix] === to[to.length - 1 - suffix]) {
    suffix++;
  }
  return to.length - suffix;
}
