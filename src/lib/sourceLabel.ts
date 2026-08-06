// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a document actually came from, in the words a person would use.
//
// `source_type` alone collapses every connector to one sentence — "Not stored here — indexed from a
// connected account" — which is true of a file in one Google account, a file in a second Google
// account, a file someone shared with you, a shared drive, OneDrive and a tracked local folder
// alike. On the duplicates surface that made the two sides of a pair render byte-identically: same
// title, same origin sentence, same project, same date. There was nothing to choose between them,
// on the one screen in the app that asks you to delete one of your own documents.
//
// Everything needed was already on the row and simply not read. This decodes `source_id`, whose
// shape is set by the connectors:
//
//   gdrive:<email>:<fileId>          a file in that Google account's own Drive
//   gdrive:swm:<rootId>:<fileId>     shared with you — account-independent by design
//   gdrive:sd:<driveId>:<fileId>     a shared drive
//   onedrive:<email>:<itemId>        OneDrive
//   local:<folderKey>:<fileId>       a tracked folder on this device
//
// The `swm:`/`sd:` arms MUST be tested before the email arm, exactly as `drive::account_of` does:
// those namespaces carry no owning account, so a naive `split(':')` would report "swm" as an email
// address.

import type { Document } from "./types";

/**
 * A short provenance label — "Google Drive · you@example.com", "Google Drive · shared with you" —
 * or `null` when the document is stored in the vault and `source_type` already says everything.
 */
export function sourceLabel(doc: Pick<Document, "source_id">): string | null {
  const id = doc.source_id;
  if (!id) return null;

  const drive = id.startsWith("gdrive:") ? id.slice("gdrive:".length) : null;
  if (drive !== null) {
    if (drive.startsWith("swm:")) return "Google Drive · shared with you";
    if (drive.startsWith("sd:")) return "Google Drive · a shared drive";
    const email = drive.split(":")[0];
    return email ? `Google Drive · ${email}` : "Google Drive";
  }

  const onedrive = id.startsWith("onedrive:") ? id.slice("onedrive:".length) : null;
  if (onedrive !== null) {
    const email = onedrive.split(":")[0];
    return email ? `OneDrive · ${email}` : "OneDrive";
  }

  if (id.startsWith("local:")) return "This device";
  return null;
}

/**
 * The most specific place-name available for a document, for a surface that has to tell two
 * otherwise-identical rows apart: the provenance label, then the folder it sits in.
 *
 * Returns the parts rather than a joined string so a caller can style or wrap them; empty when
 * there is nothing beyond what `source_type` already conveys.
 */
export function provenanceParts(
  doc: Pick<Document, "source_id" | "source_parent_folder_name" | "source_path">,
): string[] {
  const parts: string[] = [];
  const label = sourceLabel(doc);
  if (label) parts.push(label);
  // The containing folder is often the ONLY thing that differs between two copies of one file, so
  // it earns its place ahead of the full path, which is usually too long to read in a card.
  if (doc.source_parent_folder_name) parts.push(doc.source_parent_folder_name);
  else if (doc.source_path) parts.push(doc.source_path);
  return parts;
}

/**
 * The full path or URL a document can actually be reached at — the one line that answers "where did
 * this come from", for BOTH ingest routes.
 *
 * The two routes store it in different columns and neither is ever populated for the other, which is
 * why a caller cannot just read one: a stored document has `source_path` and a null `external_ref`,
 * an indexed one has `external_ref` and a structurally-null `source_path`. That asymmetry is why the
 * Documents table showed a path under local files and nothing at all under connector files — the
 * data was there under the other name.
 *
 * `external_ref` is a path for some connectors and a URL for others (the tracked local folder stores
 * the absolute path; Drive stores its `webViewLink`, OneDrive its `webUrl`), which is exactly what
 * "the full access path" means for each of them — so this deliberately does not try to normalise the
 * two into one shape.
 *
 * NOTE this is not reachable through `provenanceParts`, which falls back to `source_path` and so can
 * never see `external_ref` for a connector document.
 */
export function documentLocation(
  doc: Pick<Document, "source_type" | "source_path" | "external_ref">,
): string | null {
  return doc.source_type === "index_only" ? doc.external_ref : doc.source_path;
}

/**
 * Where a document lives, coarsely — the axis the Documents table's Source column sorts on.
 *
 * `source_type` alone CANNOT answer this, which is the trap worth writing down: a file in a tracked
 * folder on this machine is `index_only` too (PM indexes it by pointer and reads the body off disk
 * on demand), so "is it local" is a question about the `source_id` namespace, not about the type.
 * Hence `sourceLabel`, which already owns those namespace rules, decides the origin here rather than
 * a second prefix parser drifting from it.
 *
 * Reachability outranks origin: a document whose source is gone is a different kind of thing from a
 * Drive document, and someone sorting this column is usually looking for exactly those. The two
 * problem states stay apart for the reason the backend keeps them apart — an expired token means
 * "ask again later", a missing source means "it is gone".
 */
export type SourceGroup = "vault" | "device" | "drive" | "onedrive" | "unreachable" | "missing";

/** Ascending order of the Source column: what PM holds, then this machine, then the clouds, then the
 *  two kinds of trouble. Descending flips it, so one click either way reaches the broken rows. */
const SOURCE_GROUP_RANK: Record<SourceGroup, number> = {
  vault: 0,
  device: 1,
  drive: 2,
  onedrive: 3,
  unreachable: 4,
  missing: 5,
};

export function sourceGroup(doc: Pick<Document, "source_id" | "source_state">): SourceGroup {
  if (doc.source_state === "source_missing") return "missing";
  if (doc.source_state === "unreachable") return "unreachable";
  const id = doc.source_id;
  if (id == null) return "vault";
  if (id.startsWith("gdrive:")) return "drive";
  if (id.startsWith("onedrive:")) return "onedrive";
  if (id.startsWith("local:")) return "device";
  // An id in a namespace this build doesn't know is still a pointer to something outside the vault,
  // so it must not rank as "held here" — group it with the clouds rather than inventing a seventh
  // bucket that no column value would explain.
  return "drive";
}

/** Sort position of a document on the source axis. */
export function sourceRank(doc: Pick<Document, "source_id" | "source_state">): number {
  return SOURCE_GROUP_RANK[sourceGroup(doc)];
}

/** What the Source column reads: where the file is, plus what is wrong with it when something is.
 *
 *  The problem is appended rather than replacing the origin, because a column you sorted by has to
 *  explain its own order — a row that sorted to the bottom while reading "Google Drive" looks like a
 *  bug. `SourceBadge` beside the title says the same thing in its own words; this is the sortable,
 *  scannable form of it. */
export function sourceSummary(doc: Pick<Document, "source_id" | "source_state">): string {
  const origin = sourceLabel(doc) ?? "In your vault";
  switch (doc.source_state) {
    case "source_missing":
      return `${origin} · not there any more`;
    case "unreachable":
      return `${origin} · can’t reach it`;
    default:
      return origin;
  }
}
