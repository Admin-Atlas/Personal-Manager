// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Inline `[n]` citation markers, made to survive a Markdown parse.
//
// A grounded answer arrives as one string with the model's citations written inline as [1], [2], …
// PM used to make those clickable by splitting the raw text on /(\[\d+\])/ into React nodes — which
// works for plain text but cannot survive rendering the answer AS Markdown: a document chopped into
// fragments loses its block structure, so lists, tables and code fences would break apart at every
// marker.
//
// Rewriting each marker into the in-page link `[[n]](#pm-cite-n)` keeps the answer ONE Markdown
// document instead. The link text stays the literal `[n]`, `markdown.tsx`'s `safeUrl` already allows
// `#`-prefixed hrefs, and `rehype-external-links` leaves it alone (it only rewrites http/https), so
// the anchor never gets `target="_blank"` and App's OS-browser link interceptor — which requires
// both that target and an http(s) href — can't hijack the click. `Bubble` delegates the click back
// to the same `jumpToSource` the old buttons called, and a real `<a href>` is keyboard-operable for
// free (Enter fires a click).

/** The href scheme for an in-page citation link. Not a real anchor target — nothing has this id; it
 *  is a marker `Bubble`'s click delegate matches on, and the scroll is done imperatively. */
export const CITE_HREF_PREFIX = "#pm-cite-";

const MARKER = /\[(\d+)\]/g;
/** A fence line opens or closes a code block; markers inside one are code, not citations. */
const FENCE = /^\s*(?:```|~~~)/;
/** Split pattern for inline code spans — the capture group keeps them in the output, at odd indices. */
const INLINE_CODE = /(`+[^`]*`+)/;

/**
 * Rewrite every in-range `[n]` marker in `content` to a `#pm-cite-n` Markdown link, leaving fenced
 * and inline code untouched. A marker outside 1..`count` is left as literal text, exactly as the old
 * node-splitting version did — the model sometimes cites a source that didn't survive the gate.
 *
 * Pure and total: returns `content` unchanged when there are no citations to link.
 */
export function linkCitations(content: string, count: number): string {
  if (count < 1 || !content.includes("[")) return content;

  let fenced = false;
  return content
    .split("\n")
    .map((line) => {
      if (FENCE.test(line)) {
        fenced = !fenced;
        return line;
      }
      if (fenced) return line;
      return line
        .split(INLINE_CODE)
        .map((part, i) =>
          i % 2 === 1
            ? part
            : part.replace(MARKER, (whole, digits: string) => {
                const n = Number(digits);
                return n >= 1 && n <= count ? `[${whole}](${CITE_HREF_PREFIX}${n})` : whole;
              }),
        )
        .join("");
    })
    .join("\n");
}

/** The source number a clicked href points at, or null when it isn't one of ours (or is out of
 *  range — a hand-crafted `#pm-cite-99` in a model answer must not scroll to a source that
 *  doesn't exist). */
export function citationTarget(href: string | null | undefined, count: number): number | null {
  if (!href || !href.startsWith(CITE_HREF_PREFIX)) return null;
  const n = Number(href.slice(CITE_HREF_PREFIX.length));
  return Number.isInteger(n) && n >= 1 && n <= count ? n : null;
}
