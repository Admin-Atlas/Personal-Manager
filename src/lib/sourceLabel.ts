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
