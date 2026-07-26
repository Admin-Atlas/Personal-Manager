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
