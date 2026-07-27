// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The `@tag` grammar (#276), as pure functions.
//
// Writing `@marketing` in a chat message PINS that tag for that one query: in a project chat it
// widens retrieval to also reach the tag's documents, and in a global chat it narrows an otherwise
// unscoped search down to them. The scope decision itself is made in Rust, from the message that was
// actually sent — the text is the record of what was asked, so the two cannot disagree.
//
// What lives here is the frontend's half: finding the token the caret is sitting in (so the composer
// can offer completions) and finding the mentions in a sent message (so they can be highlighted).
// Both must agree with `tags::parse_mentions` in Rust, which is why the rules are written once here
// and tested on both sides:
//
//   - a mention starts at an `@` that begins a word — `bob@example.com` mentions nothing;
//   - a bare mention runs to the next whitespace, minus any trailing sentence punctuation;
//   - a name containing a space is quoted: `@"Atlas, Inc."` — without that form the project names
//     #275 went out of its way to allow would be exactly the ones that could never be pinned;
//   - matching is case-insensitive on a trimmed, ASCII-lowercased form.
//
// Nothing here decides what IS a tag. A candidate is only a mention once it matches the registry —
// so a stray `@` in prose pins nothing and is not highlighted.

import type { TagSummary } from "./types";

/** The same normalisation `tags::normalize` applies in Rust: trimmed, ASCII-lowercased. */
export function normalizeTag(name: string): string {
  return name.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
}

const TRAILING = /[.,;:!?)\]"]+$/;

/** Does the character before an `@` allow it to begin a mention? */
function startsWord(before: string | undefined): boolean {
  return before === undefined || /\s/.test(before) || before === "(";
}

/** The mention beginning at `at` (the index of the `@`): its name and the index just past it. */
function readMention(text: string, at: number): { name: string; next: number } {
  const start = at + 1;
  if (text[start] === '"') {
    const close = text.indexOf('"', start + 1);
    // An unclosed quote is one the user is still typing, not a mention.
    if (close === -1) return { name: "", next: start };
    return { name: text.slice(start + 1, close), next: close + 1 };
  }
  let end = start;
  while (end < text.length && !/\s/.test(text[end])) end++;
  const name = text.slice(start, end).replace(TRAILING, "");
  return { name, next: start + name.length };
}

/** Every `@mention` candidate in `text`, in order, deduplicated case-insensitively. */
export function parseMentions(text: string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== "@" || !startsWord(i > 0 ? text[i - 1] : undefined)) continue;
    const { name, next } = readMention(text, i);
    const key = normalizeTag(name);
    if (name && !seen.has(key)) {
      seen.add(key);
      out.push(name);
    }
    i = Math.max(next - 1, i);
  }
  return out;
}

/** The mention token the caret is inside, if any — what the composer offers completions for. */
export interface MentionAtCaret {
  /** Index of the `@`. */
  start: number;
  /** Index just past the token (the caret itself). */
  end: number;
  /** What has been typed after the `@`, possibly empty. */
  query: string;
  /** True when the user opened a quote, so completing must re-quote the name. */
  quoted: boolean;
}

/**
 * Find the mention being typed at `caret`, or `null`.
 *
 * Only ever looks BACKWARDS from the caret, so the suggestion list tracks what is being typed rather
 * than a completed mention elsewhere in the message. Whitespace ends the search — except inside an
 * open quote, where a space is part of the name being typed and is exactly why the quote is there.
 */
export function mentionAtCaret(text: string, caret: number): MentionAtCaret | null {
  const openQuote = text.lastIndexOf('"', caret - 1);
  if (openQuote > 0 && text[openQuote - 1] === "@" && startsWord(text[openQuote - 2])) {
    // Only if the quote is still open at the caret.
    if (text.indexOf('"', openQuote + 1) >= caret || text.indexOf('"', openQuote + 1) === -1) {
      return {
        start: openQuote - 1,
        end: caret,
        query: text.slice(openQuote + 1, caret),
        quoted: true,
      };
    }
  }
  for (let i = caret - 1; i >= 0; i--) {
    const ch = text[i];
    if (/\s/.test(ch)) return null;
    if (ch === "@") {
      if (!startsWord(i > 0 ? text[i - 1] : undefined)) return null;
      return { start: i, end: caret, query: text.slice(i + 1, caret), quoted: false };
    }
  }
  return null;
}

/**
 * Replace the mention at `at` with `name`, returning the new text and where the caret should land.
 *
 * A name containing whitespace is quoted, because that is the only form the parser reads as one
 * mention. A trailing space closes the token so the next word starts fresh.
 */
export function completeMention(
  text: string,
  at: MentionAtCaret,
  name: string,
): { text: string; caret: number } {
  const inserted = /\s/.test(name) ? `@"${name}" ` : `@${name} `;
  return {
    text: text.slice(0, at.start) + inserted + text.slice(at.end),
    caret: at.start + inserted.length,
  };
}

/** A run of text, flagged if it is a mention of a KNOWN tag. */
export interface MentionSegment {
  text: string;
  /** The canonical registry name when this run is a real tag; absent for ordinary text. */
  tag?: string;
}

/**
 * Split `text` into runs so a rendered message can emphasise the mentions that actually resolved.
 *
 * Only mentions matching a known tag are marked, and that distinction is the whole value of showing
 * this: it is how someone sees that `@marketing` reached their Marketing files and `@markting` did
 * not. A message with no resolved mentions comes back as a single run, so the common case renders
 * exactly as it always did.
 */
export function splitMentions(text: string, known: readonly string[]): MentionSegment[] {
  const byNorm = new Map(known.map((k) => [normalizeTag(k), k]));
  if (byNorm.size === 0) return [{ text }];

  const out: MentionSegment[] = [];
  let plain = "";
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "@" && startsWord(i > 0 ? text[i - 1] : undefined)) {
      const { name, next } = readMention(text, i);
      const hit = name ? byNorm.get(normalizeTag(name)) : undefined;
      if (hit) {
        if (plain) {
          out.push({ text: plain });
          plain = "";
        }
        out.push({ text: text.slice(i, next), tag: hit });
        i = next - 1;
        continue;
      }
    }
    plain += text[i];
  }
  if (plain) out.push({ text: plain });
  return out;
}

/** How many suggestions to show. Enough to be useful, few enough to stay a glance. */
const MAX_SHOWN = 6;

/** The tags matching `query`, best first. Exported for its test — it is the whole ranking rule. */
export function matchTags(tags: readonly TagSummary[], query: string): TagSummary[] {
  const q = normalizeTag(query);
  const scored = tags
    .map((t) => ({ tag: t, at: normalizeTag(t.name).indexOf(q) }))
    .filter((s) => q === "" || s.at >= 0);
  // A prefix match beats a match in the middle; then the tag people actually use; then a stable
  // alphabetical tiebreak so the list never reshuffles between renders.
  scored.sort(
    (a, b) =>
      a.at - b.at || b.tag.documents - a.tag.documents || a.tag.name.localeCompare(b.tag.name),
  );
  return scored.slice(0, MAX_SHOWN).map((s) => s.tag);
}
