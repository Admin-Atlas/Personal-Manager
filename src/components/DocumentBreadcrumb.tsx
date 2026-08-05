// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a document sits, as folders rather than as a URL (#736).
//
// The line this replaces was `https://docs.google.com/document/d/1aB…/edit`, which is the correct
// answer to "how do I open this" and no answer at all to "where is this". A Drive URL contains no
// folder, by design — the id is the file's identity and its location is a separate lookup — so a
// person reading a list of documents could not tell two files called "Notes" apart, which is the
// thing the column was added for.
//
// Rendering rules, both of which exist so a breadcrumb never over-claims:
//
//   * **The last crumb is a folder, never the file.** Drive's own header ends at the containing
//     folder and so does this; repeating the title on the line under the title is noise.
//   * **A short trail is not a truncated one.** PM shows what it can actually see. On a file shared
//     with you the folders above the share boundary are invisible to your account, so the crumb list
//     starts where your visibility does — and never fabricates the ancestors it cannot read.

import { documentBreadcrumb, documentLocation } from "../lib/sourceLabel";
import type { Document } from "../lib/types";

/** The separator between crumbs — the same character Drive and Finder use, so it reads as a path
 *  rather than as a sentence. Rendered `aria-hidden` with the crumbs joined into one string for
 *  assistive tech, since a screen reader announcing "chevron" between every folder is worse than
 *  useless. */
const SEP = "›";

export interface DocumentBreadcrumbProps {
  doc: Pick<
    Document,
    | "source_id"
    | "source_folder_path"
    | "source_parent_folder_name"
    | "source_type"
    | "source_path"
    | "external_ref"
  >;
  /** Extra classes for the line — callers own the colour and the size, since the reader's header and
   *  a dense table row want different ones. */
  className?: string;
}

/**
 * Where one document is: its folder trail, or — for a document with no trail to give — the plain
 * path it came from.
 *
 * The fallback is not a hedge, it is the other half of the answer. A **vault** document was imported
 * once from a file that PM does not go on watching, so it has no live folders to name; what it has
 * is where it came from, and deleting that line in the name of a breadcrumb would take away
 * information #734 had just added. Everything a connector tracks takes the crumbs; everything else
 * takes its path.
 *
 * A document with neither renders nothing at all, deliberately: "Location unknown" under every chat
 * and every photo would be chrome that never becomes useful.
 */
export function DocumentBreadcrumb({ doc, className }: DocumentBreadcrumbProps) {
  const crumbs = documentBreadcrumb(doc);
  if (crumbs.length === 0) {
    const path = documentLocation(doc);
    if (!path) return null;
    return (
      <p className={`truncate text-xs text-ink4 ${className ?? ""}`} title={path}>
        {path}
      </p>
    );
  }
  const full = crumbs.join(` ${SEP} `);
  return (
    <p className={`truncate text-xs text-ink4 ${className ?? ""}`} title={full}>
      {crumbs.map((crumb, i) => (
        // Index keys: a trail is a positional list of plain strings with no identity of its own, and
        // two sibling folders may legitimately share a name.
        <span key={i}>
          {i > 0 && (
            <span className="px-1 text-ink4/60" aria-hidden>
              {SEP}
            </span>
          )}
          {crumb}
        </span>
      ))}
    </p>
  );
}
