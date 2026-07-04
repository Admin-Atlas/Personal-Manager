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
