// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device toggle: whether a dropped photo is copied into the vault or merely referenced where it
// lies. Default OFF — copying spends disk, so it is opt-in.
//
// This has to persist. The Documents tab is rendered conditionally, so leaving it unmounts the view
// and every useState resets; the box came back unticked AND, more damagingly, a photo dropped after
// a tab round-trip was silently not copied — a standing preference behaving like a one-shot.
//
// localStorage rather than a backend Setting: the value is already threaded per call into
// `ingest_paths`, no Rust code reads it as state, and it is a statement about THIS machine's disk.
// Mirrors reviewPrefs.

export const COPY_PHOTOS_KEY = "pm.documents.copyPhotosToVault";

/** Whether to keep a vault copy of dropped photos. Absent (fresh install) = off. */
export function readCopyPhotosToVault(): boolean {
  try {
    return localStorage.getItem(COPY_PHOTOS_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeCopyPhotosToVault(on: boolean): void {
  try {
    localStorage.setItem(COPY_PHOTOS_KEY, String(on));
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}

// Whether the one-time "PM can look for duplicates" nudge has been dismissed (#282).
//
// localStorage, not a vault Setting, and the distinction matters: the duplicate check being ON is a
// statement about this LIBRARY (it belongs to the vault, and travels with it), whereas having seen
// the suggestion is a statement about THIS PERSON at THIS MACHINE. Storing the dismissal in the vault
// would re-show the nudge to a colleague opening a shared vault who had already dismissed it, and
// hide it from the same user on a second machine — both backwards.
export const DUPLICATE_NUDGE_KEY = "pm.documents.duplicateNudgeSeen";

/** Whether the duplicate-check suggestion has already been shown and dismissed. */
export function readDuplicateNudgeSeen(): boolean {
  try {
    return localStorage.getItem(DUPLICATE_NUDGE_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeDuplicateNudgeSeen(): void {
  try {
    localStorage.setItem(DUPLICATE_NUDGE_KEY, "true");
  } catch {
    /* best-effort — worst case the suggestion appears once more */
  }
}
