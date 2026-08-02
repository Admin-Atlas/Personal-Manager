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

// Whether the rebuild Activity fold is open.
//
// The Documents tab is a branch of App's view ternary inside an ErrorBoundary keyed on the view, so
// LEAVING THE TAB UNMOUNTS IT. `Collapsible` keeps its own `useState(defaultOpen)` when it isn't
// given `open`/`onOpenChange`, and that state dies with the mount — so a fold the user had closed
// reseeded open on every return. Its own header comment already says the fix: state that must
// outlive the mount is passed in, not left to the primitive.
//
// localStorage rather than a backend Setting, per the same rule as the two prefs above: a fold is a
// statement about this person at this machine, not about the library.
export const ACTIVITY_OPEN_KEY = "pm.documents.activityOpen";

/** Whether the Activity fold is open — `null` when the user has never chosen, so the caller keeps
 *  ownership of the default rather than having `false` mean both "closed" and "unset". */
export function readActivityOpen(): boolean | null {
  try {
    const raw = localStorage.getItem(ACTIVITY_OPEN_KEY);
    // Only the two values this ever writes count as a choice. Anything else — a hand-edited key, a
    // half-written value — is "never chosen", not "closed": defaulting a junk read to closed would
    // silently hide the Activity list, which is the one thing on this card the user came to see.
    if (raw === "true") return true;
    if (raw === "false") return false;
    return null;
  } catch {
    return null;
  }
}

export function writeActivityOpen(open: boolean): void {
  try {
    localStorage.setItem(ACTIVITY_OPEN_KEY, String(open));
  } catch {
    /* best-effort — a private-mode / quota failure just means the fold won't persist */
  }
}
