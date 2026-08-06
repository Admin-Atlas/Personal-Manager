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
// Supported line markers (each needs a trailing space): "." round bullets, "-" dash points, "1."
// numbered, "i." roman, ">" arrow/quote, "[]" checkbox (also "[x]"). ">" renders as a blockquote (its
// native Markdown meaning) and roman items keep their exact labels. These fidelity choices are
// documented, not accidental.
//
// **"." and "-" are two kinds of BULLET, not a bullet and some prose.** They used to render
// identically, because both became the same GFM `- ` item and Markdown records nothing about which
// character an author typed. They are now emitted as two different GFM bullet characters — "." as
// `*`, "-" as `+` — which a remark plugin reads back off the source to tag the list (see
// `src/lib/markdownDashLists.ts`), so one gets a disc and the other an en dash. Both stay real list
// items: nesting and hanging indent are the point, and a dash point rendered as prose with a dash
// typed in front of it would lose both. Both are ordinary GFM, so the copy this hands to the vault
// stays portable and reads as a plain bullet anywhere outside the board.
//
// Note that "-" is no longer a passthrough marker and round bullets are no longer emitted as "-".
// That pairing is forced, not stylistic: "-" is now an INPUT marker meaning "dash point", so if
// round bullets still came out as "-" the transform would read its own output back as dash points.

import { DASH_MARKER } from "../markdownDashLists";

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
//
// "*" and "+" are here because they are what the two bullet kinds are EMITTED as. Without them,
// re-running the transform over its own output would read every list line as prose and append a
// second hard break each time — and that output is what gets ingested into the vault, so a
// repeatedly-promoted note would grow whitespace. Including them also means a pasted "*" or "+"
// list is READ as the kind it already RENDERS as, which is the only self-consistent answer.
const MARKER_RE = /^(\s*)(-|\+|\.|\*|>|\[[ xX]?\]|\d+\.|[ivxlcdmIVXLCDM]+\.)\s+(.*)$/;

/** The GFM bullet a round "." bullet is emitted as.
 *
 *  NOT "-", which is what it used to be: "-" is now the dash point's own input marker, so emitting
 *  round bullets as "-" would make the transform read its own output back as dash points. The two
 *  alphabets have to stay disjoint. "*" and "+" both render as an ordinary disc anywhere outside a
 *  PM note, which is what keeps the vault copy portable. */
const BULLET_MARKER = "*";

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
  if (token.length === 1 && "-+.*>".includes(token)) return `${token} `;
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

/** Whether a source line renders as a CONTAINER block — a list item or a blockquote — as opposed to
 *  inline text. The distinction matters for lazy continuation: CommonMark folds an unmarked line that
 *  follows one of these INTO it, so the author's line break disappears. Roman items are deliberately
 *  excluded: they render as ordinary paragraph text (GFM has no roman list), so a following prose
 *  line is already joined by the hard break the roman branch emits. */
function opensContainer(line: string): boolean {
  const m = matchMarker(line);
  if (!m) return false;
  return m.token === ">" || !isRomanToken(m.token);
}

/**
 * Normalise a note's shorthand marker dialect to GFM for the shared <Markdown> renderer:
 *  - "[]" / "[x]" → GFM task-list items ("* [ ]" / "* [x]"), on the bullet marker so a checklist
 *    and the bullets around it stay one list;
 *  - "." bullets → "*" bullets (a disc, once rendered);
 *  - "-" dash points → "+" bullets, which `remarkDashLists` tags so they render with an en dash —
 *    EXCEPT a literal GFM task ("- [ ] x"), which passes through untouched: `countTasks` and
 *    `toggleTaskAt` match the SOURCE line, so a rendered checkbox that no longer has a source
 *    counterpart in the same order would tick the wrong line;
 *  - roman items keep their label but gain a hard line break so a run stays multi-line and
 *    isn't collapsed into one paragraph (GFM merges single newlines);
 *  - "-", "1.", ">" already render natively (bullet, ordered list, blockquote) — untouched.
 *  - a plain prose line gains that same two-space hard break, so a note's manual line breaks
 *    survive rendering (GFM otherwise folds a single newline into a space, merging the lines).
 *    Blank lines stay blank (paragraph breaks) and an already-broken line isn't doubled, so the
 *    pass stays idempotent.
 *  - a plain prose line that FOLLOWS a list item or quote gets a blank line inserted before it, so
 *    it ends the list instead of being swallowed by the last item (see below).
 *
 * Line endings are normalised first. A pasted CRLF note would otherwise put the `\r` between the
 * text and the two-space hard break — `"line one\r  \n"` — which stops being a hard break and turns
 * the pair into two separate paragraphs.
 */
export function toRenderMarkdown(raw: string): string {
  const lines = raw.replace(/\r\n?/g, "\n").split("\n");
  const out: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const m = matchMarker(line);
    if (!m) {
      // Plain prose: keep the author's own line breaks. A bare single newline is a GFM soft
      // break (renders as a space), so a note typed across several lines would collapse into
      // one. Two trailing spaces make it a hard break — the same rule the roman branch uses.
      // Leave blank lines and already-broken lines alone so re-running is a no-op.
      if (line.trim() === "") {
        out.push(line);
        continue;
      }
      // A marker-less line after a list item or quote is a LAZY CONTINUATION in CommonMark: it is
      // folded into that item and the line break vanishes ("- item\nplain" → one bullet reading
      // "item plain"). In this dialect a marker is explicit, so an unmarked line is unambiguously
      // the author leaving the list — close it with a blank line. Two trailing spaces cannot save
      // this one: the problem is block-level, not a soft break inside a paragraph.
      if (i > 0 && opensContainer(lines[i - 1])) out.push("");
      out.push(line.endsWith("  ") ? line : `${line}  `);
      continue;
    }
    const { indent, token, content } = m;

    const cb = /^\[([ xX]?)\]$/.exec(token);
    if (cb) {
      // The BULLET marker, so a checklist and the round bullets above it stay ONE list. A marker
      // change starts a new list in CommonMark, and two lists carry the `ul` margin between them
      // where a shared one carries only the much smaller `li` one.
      out.push(`${indent}${BULLET_MARKER} [${cb[1].toLowerCase() === "x" ? "x" : " "}] ${content}`);
      continue;
    }

    if (token === "." || token === BULLET_MARKER) {
      out.push(GFM_TASK_RE.test(line) ? line : `${indent}${BULLET_MARKER} ${content}`);
      continue;
    }

    // A dash point becomes a "+" bullet, which survives the parse as a distinguishable marker. A
    // literal GFM task keeps its "-" so it stays an ordinary task item in an ordinary list — see the
    // doc comment above for why that one must not move.
    if (token === "-" || token === DASH_MARKER) {
      out.push(GFM_TASK_RE.test(line) ? line : `${indent}${DASH_MARKER} ${content}`);
      continue;
    }

    // Two trailing spaces = a Markdown hard break, so consecutive roman items each keep their
    // own line (and their exact "i."/"ii." labels) instead of merging. `content` is right-trimmed
    // first: the marker regex's `(.*)` swallows any trailing spaces, so re-running the pass would
    // otherwise append another pair every time. Harmless to the renderer, but this output is what
    // gets ingested into the vault (F-52), so a repeatedly-promoted note would grow whitespace.
    if (isRomanToken(token)) {
      out.push(`${indent}${token} ${content.replace(/[ \t]+$/, "")}  `);
      continue;
    }

    out.push(line);
  }
  return out.join("\n");
}

// --- ticking a checkbox in the RENDERED note (pure) ------------------------------------------
// A rendered note's checkboxes used to be inert: the sanitizer forces `disabled` on every `<input>`
// it emits, so the only way to tick one was to open the editor and retype the marker. Rather than
// relax that (SCHEMA is the boundary for INGESTED content too — a Drive document must never render
// live controls), the note widget catches the click, works out which task item was hit, and calls
// this. So the checkbox stays disabled in the DOM and the toggle is a source edit, like every other
// note change.
//
// Two dialects render as a task item and both have to be counted, in line order, or the Nth
// rendered box would map to the wrong line: the note's own `[] foo` shorthand, and a literal GFM
// `- [x] foo` (which `toRenderMarkdown` passes through untouched, since `-` wins the marker
// alternation and the brackets survive as content).

/** The note dialect: `[]` / `[ ]` / `[x]` as the line's own marker. */
const NOTE_TASK_RE = /^(\s*)\[([ xX]?)\](\s+)(.*)$/;
/** Literal GFM: a bullet whose content starts with a checkbox. */
const GFM_TASK_RE = /^(\s*[-*+]\s+)\[([ xX])\](\s+)(.*)$/;

/** Whether a line renders as a tickable task item, and its checked state. */
function matchTask(line: string): { checked: boolean } | null {
  const m = NOTE_TASK_RE.exec(line) ?? GFM_TASK_RE.exec(line);
  return m ? { checked: m[2].toLowerCase() === "x" } : null;
}

/** How many tickable checkboxes the rendered note has — the bound the caller's index must respect. */
export function countTasks(raw: string): number {
  return raw.split("\n").reduce((n, line) => n + (matchTask(line) ? 1 : 0), 0);
}

/**
 * Flip the `index`-th checkbox (0-based, in rendered order) and return the new note text, or `null`
 * when the index names no checkbox — so a stale click after an edit is a no-op rather than a write
 * that ticks the wrong line.
 *
 * `checked` forces a state instead of flipping, which is what the DOM event actually carries: the
 * browser has already painted the input's new value by the time we hear about it, so echoing that
 * value keeps the source and the pixels in agreement even if two clicks land in one frame.
 */
export function toggleTaskAt(raw: string, index: number, checked?: boolean): string | null {
  if (!Number.isInteger(index) || index < 0) return null;
  const lines = raw.split("\n");
  let seen = -1;
  for (let i = 0; i < lines.length; i++) {
    const hit = matchTask(lines[i]);
    if (!hit) continue;
    if (++seen !== index) continue;
    const next = checked ?? !hit.checked;
    // Each dialect keeps its own canonical unchecked form: the note shorthand writes a bare `[]`
    // (what continueList emits), GFM writes `[ ]` (what the spec requires).
    lines[i] = NOTE_TASK_RE.test(lines[i])
      ? lines[i].replace(NOTE_TASK_RE, (_all, ind, _st, gap, rest) =>
          next ? `${ind}[x]${gap}${rest}` : `${ind}[]${gap}${rest}`,
        )
      : lines[i].replace(GFM_TASK_RE, (_all, lead, _st, gap, rest) =>
          next ? `${lead}[x]${gap}${rest}` : `${lead}[ ]${gap}${rest}`,
        );
    return lines.join("\n");
  }
  return null;
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
  // "." not "-": the button is called Bullet, and "-" is now the dash point.
  bullet: ". ",
  checkbox: "[] ",
  heading: "# ",
  quote: "> ",
};

/** Length of the given kind's marker at the start of a line (incl. indent + trailing space), or 0. */
const MARKER_LEN_RE: Record<LineMarkerKind, RegExp> = {
  // Both spellings: "." is what the button writes, "*" is what the transform emits, so a note
  // round-tripped through the vault still reads as bulleted.
  bullet: /^(\s*)[.*] /,
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
