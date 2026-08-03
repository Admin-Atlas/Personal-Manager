// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every place one document's file lives (#710/#711).
//
// Before #711 this component would have had nothing to say: a document was its location, so "where
// is this" was one line and the row already carried it. Now one file reached through two Google
// accounts, or as both a shared-drive item and something shared with you, is ONE document with two
// places — and the two differ in exactly the ways that matter when you are deciding whether to
// delete something: which account, which folder, which path, whether PM can still reach it.
//
// Two rules it exists to keep:
//
//   * **A single place renders nothing.** The provenance line beside it already says where the file
//     is, and "In 1 place" under it would be chrome restating the row. This only appears when there
//     is genuinely something the row cannot show.
//   * **The anchor is an ordering, not a verdict.** It is shown first because it is the id PM
//     assigned at birth, and that makes the list stable between sessions. It is NOT the good copy,
//     the primary, or the one to keep — a document is as reachable as its best place, and the anchor
//     is routinely the dead one.

import { useEffect, useState } from "react";

import { documentLocations } from "../lib/ipc";
import { formatDateTime } from "../lib/format";
import { provenanceParts } from "../lib/sourceLabel";
import type { DocumentLocation } from "../lib/types";
import { Collapsible } from "./ui";

/** What PM can currently do at one place, in the words a person would use. `null` for a healthy one
 *  — a badge on every row that mostly says "fine" trains people to stop reading badges. */
function reachability(state: DocumentLocation["state"]): string | null {
  switch (state) {
    case "unreachable":
      return "can’t reach it just now";
    case "source_missing":
      return "not there any more";
    default:
      return null;
  }
}

/** One place, as a line: where it is, which folder, and anything wrong with it. */
function PlaceRow({ place }: { place: DocumentLocation }) {
  // Reuses the document labeller rather than a second copy of the connectors' namespace rules —
  // `external_ref` stands in for `source_path`, which is what it is for a location.
  const parts = provenanceParts({
    source_id: place.source_id,
    source_parent_folder_name: place.source_parent_folder_name,
    source_path: place.external_ref,
  });
  const problem = reachability(place.state);
  return (
    <li className="border-t border-border py-1.5 first:border-t-0">
      <p className="break-words text-xs text-ink2">
        {parts.length > 0 ? parts.join(" · ") : place.source_id}
      </p>
      <p className="mt-0.5 text-xs text-ink4">
        {place.source_modified_at && <>Changed {formatDateTime(place.source_modified_at)}</>}
        {place.source_modified_at && problem && " · "}
        {problem && <span className="text-[var(--st-due)]">{problem}</span>}
      </p>
    </li>
  );
}

/**
 * The places one document's file lives, folded away until asked for.
 *
 * Fetched per document rather than carried on the `Document` row: the Documents list loads hundreds
 * of rows at a time and none of them need this, so putting it on the row would make every list query
 * pay for a join that two surfaces use.
 */
export function DocumentPlaces({ docId }: { docId: number }) {
  const [places, setPlaces] = useState<DocumentLocation[]>([]);

  useEffect(() => {
    let alive = true;
    documentLocations(docId)
      .then((p) => {
        if (alive) setPlaces(p);
      })
      // Silent: this is supplementary detail on a surface that already works without it, and an
      // error line here would be louder than the thing it is apologising for.
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [docId]);

  if (places.length < 2) return null;
  return (
    <Collapsible
      title={<span className="text-xs text-ink3">In {places.length} places</span>}
      defaultOpen={false}
      className="mt-2"
    >
      <ul className="mt-1">
        {places.map((place) => (
          <PlaceRow key={place.source_id} place={place} />
        ))}
      </ul>
    </Collapsible>
  );
}
