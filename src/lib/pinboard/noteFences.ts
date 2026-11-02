// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// --- fenced code (pure) ----------------------------------------------------------------------
//
// Everything in notesMarkdown rewrites lines by the marker they start with, and inside a fenced code
// block that is exactly the wrong thing to do: a line there is not a list item, it is a picture of
// one. The damage is not cosmetic. A `- ` line in a ```diff block is a REMOVED line, and rewriting
// it to `+ ` turns it into an added one — the snippet now says the opposite of what was pasted, and
// it says it in the vault copy too, because this output is what `ingestNote` files (F-52).
//
// The same blindness made a checkbox inside a fence tickable: `matchTask` saw `[] x` and counted it,
// but the renderer emits it as literal text with no `<input>`, so every real checkbox after it was
// off by one and a click ticked the wrong line.
//
// Its own module because note tables (noteTables) need the same mask, and notesMarkdown needs the
// tables in turn: one shared leaf keeps the two from importing each other.

/** An opening or closing fence: up to three spaces, then three-or-more backticks or tildes. */
const FENCE_RE = /^ {0,3}(`{3,}|~{3,})(.*)$/;

/**
 * Which lines are fenced code — the fence lines themselves included, since they are not content
 * either.
 *
 * CommonMark's rules, kept because a note's fences also have to mean the same thing to the renderer
 * downstream: a backtick fence's info string may not contain a backtick (that is what keeps inline
 * code from opening a block), only a fence of the SAME character and at least the same length
 * closes, and an unclosed fence runs to the end of the note rather than being abandoned.
 */
export function fenceMask(lines: readonly string[]): boolean[] {
  let open: { char: string; len: number } | null = null;
  return lines.map((line) => {
    const m = FENCE_RE.exec(line);
    if (!open) {
      if (!m || (m[1][0] === "`" && m[2].includes("`"))) return false;
      open = { char: m[1][0], len: m[1].length };
      return true;
    }
    if (m && m[1][0] === open.char && m[1].length >= open.len && m[2].trim() === "") open = null;
    return true;
  });
}
