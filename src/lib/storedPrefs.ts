// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The three UI-preference blobs that hold USER CONTENT, kept in the encrypted `settings` table
// instead of the webview's localStorage.
//
// WHY THESE THREE AND NOT THE REST. The per-project milestone sort is keyed by PROJECT NAME; the
// hidden-calendar set is a list of calendar ids, which for Google/Outlook is routinely the account's
// email address; the backup dismissals were one key per destination+account, with the cloud account
// in the KEY NAME. All three sat in the WebView2/WebKit localStorage — a plaintext LevelDB beside the
// SQLCipher store that exists to hold exactly this, absent from a `.pmbackup` (pack.rs archives the
// DB snapshot and the vault, not the webview profile), and described in the "remove PM data" panel
// as "on-device interface preferences". View state — view mode, panel sizes, theme, fold state, the
// calendar cursor — is genuine chrome and deliberately STAYS in localStorage.
//
// THE TRICK THAT KEEPS THE DIFF SMALL: every read site is a synchronous `useState` initialiser or an
// effect body, and `getPref` is async. So this module owns a module-level cache, hydrated once per
// session, and the three pref modules keep their existing SYNCHRONOUS signatures on top of it.
//
// ORDERING RULES, all load-bearing:
//   - Hydration must run AFTER the store opens. `AppState::conn()` errors while the vault is locked,
//     so `getPref` rejects at boot on a passphrase vault; App drives this from the paths where the
//     store is known open, and a failed attempt leaves `hydrated` false so the next one retries.
//   - Writes before hydration are DROPPED, never persisted. The one thing that writes before a user
//     touches anything is a component's mount effect firing with its DEFAULT (ProjectView's
//     `showCompleted` does exactly that), which would otherwise stamp `true` over a stored `false`.
//     This is the same hazard useSidebarSplit's "we never blind-write the mirror on mount" avoids.
//   - The localStorage copy is removed only in the `setPref` RESOLVE path. If the write rejects, that
//     copy is still the ONLY copy and must survive so the next boot retries the adoption.
//
// NO MIGRATION IS INVOLVED. `settings` is a key/value table that has existed since v1; adding a key
// is not DDL. The keys are allowlisted backend-side in `settings.rs::WEBVIEW_PREFS`.

import { getPref, setPref } from "./ipc";

/** The three store keys. Each is its own row: `project_ui` is rewritten whole by useSidebarSplit on
 *  every divider drag, so anything folded in there is silently deleted on the next drag. */
export type StoredPrefKey = "milestone_ui" | "calendar_ui" | "backup_ui";

const KEYS: readonly StoredPrefKey[] = ["milestone_ui", "calendar_ui", "backup_ui"];

/** A stored blob is JSON we wrote; every FIELD is still validated by the module that reads it, so a
 *  hand-edited or newer-build row degrades to that module's fallback rather than throwing. */
export type PrefBlob = Record<string, unknown>;

const EMPTY: PrefBlob = Object.freeze({});

const cache: Record<StoredPrefKey, PrefBlob> = {
  milestone_ui: EMPTY,
  calendar_ui: EMPTY,
  backup_ui: EMPTY,
};

let hydrated = false;
let inflight: Promise<void> | null = null;

/** Whether the session's blobs have been read from the store yet. Reads before this point see the
 *  defaults and writes are dropped — see the ordering rules above. */
export function storedPrefsHydrated(): boolean {
  return hydrated;
}

/** Synchronous cache read. Empty (so the caller's own defaults apply) until hydration lands. */
export function readStored(key: StoredPrefKey): PrefBlob {
  return cache[key];
}

/** Patch a blob and persist it. Fire-and-forget: a rejected write is best-effort exactly like the
 *  localStorage `try/catch` it replaces, and must never throw into a click handler. */
export function writeStored(key: StoredPrefKey, patch: PrefBlob): void {
  if (!hydrated) return; // never persist a pre-hydration default over a stored value
  const next = { ...cache[key], ...patch };
  cache[key] = next;
  setPref(key, JSON.stringify(next)).catch(() => {
    /* best-effort — the cache keeps this session consistent */
  });
}

/**
 * Read all three blobs from the store once per session, adopting any pre-upgrade localStorage copy.
 *
 * Idempotent and one-shot: a second call while the first is in flight shares its promise, and a call
 * after it resolved is a no-op — StrictMode's double-invoked effect must not double-read. A REJECTED
 * attempt (the store is not open) resets, so a later call retries.
 */
export function hydrateStoredPrefs(): Promise<void> {
  if (hydrated) return Promise.resolve();
  if (!inflight) {
    inflight = Promise.all(KEYS.map(hydrateKey))
      .then(() => {
        hydrated = true;
        inflight = null;
      })
      .catch((e: unknown) => {
        inflight = null;
        throw e;
      });
  }
  return inflight;
}

/** TEST ONLY — drop the session state so each case starts from a cold module. */
export function __resetStoredPrefs(): void {
  hydrated = false;
  inflight = null;
  for (const k of KEYS) cache[k] = EMPTY;
}

async function hydrateKey(key: StoredPrefKey): Promise<void> {
  // A rejection here is the "store not open" signal and propagates, so the whole hydration retries.
  const raw = await getPref(key);
  if (raw != null) {
    // Store wins. The only way both copies exist is that adoption already ran, and every write since
    // then went to the store.
    cache[key] = parseBlob(raw);
    return;
  }
  const found = ADOPT[key]();
  if (!found) return; // nothing to carry over — a fresh profile, or a restored one already migrated
  cache[key] = found.blob;
  try {
    await setPref(key, JSON.stringify(found.blob));
  } catch {
    // The localStorage copy is still the only durable one — leave it exactly where it is and let the
    // next boot re-adopt. The cache is seeded either way, so this session behaves normally.
    return;
  }
  for (const lsKey of found.from) removeLs(lsKey);
}

function parseBlob(raw: string): PrefBlob {
  try {
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as PrefBlob;
  } catch {
    /* a corrupt row reads as absent, so the caller's defaults apply */
  }
  return EMPTY;
}

// --- one-time adoption of the pre-upgrade localStorage copies -------------------------------------
//
// Each adopter returns the blob to store plus the exact localStorage keys that become redundant once
// it lands. The VALUES are carried across verbatim (the reading module validates them, and always
// did) — in particular `pm.milestones.sort` may hold either the per-project map or the older bare
// `{key,dir}` that applied everywhere, and both shapes must survive the move or someone's
// deliberately chosen sort resets on upgrade.

interface Adopted {
  blob: PrefBlob;
  from: readonly string[];
}

const MS_SORT_LS = "pm.milestones.sort";
const MS_SHOW_COMPLETED_LS = "pm.milestones.showCompleted";
const CAL_HIDDEN_LS = "pm.calendar.hidden";
/** The composite that followed this prefix — `<destination>.<account ?? "">` — IS the stored id, so
 *  the account may be empty and may itself contain dots (it is an email address). Nothing splits it. */
const BACKUP_DISMISSED_LS_PREFIX = "pm.backup.reconcileDismissed.";

const ADOPT: Record<StoredPrefKey, () => Adopted | null> = {
  milestone_ui: () => {
    const from: string[] = [];
    const blob: PrefBlob = {};
    const sortRaw = readLs(MS_SORT_LS);
    if (sortRaw !== null) {
      from.push(MS_SORT_LS);
      try {
        blob.sort = JSON.parse(sortRaw);
      } catch {
        /* unparseable — drop it and let the default apply, exactly as reading it did */
      }
    }
    const showRaw = readLs(MS_SHOW_COMPLETED_LS);
    if (showRaw !== null) {
      from.push(MS_SHOW_COMPLETED_LS);
      blob.showCompleted = showRaw !== "false";
    }
    return from.length ? { blob, from } : null;
  },

  calendar_ui: () => {
    const raw = readLs(CAL_HIDDEN_LS);
    if (raw === null) return null;
    let hidden: unknown;
    try {
      hidden = JSON.parse(raw);
    } catch {
      hidden = [];
    }
    return { blob: { hidden }, from: [CAL_HIDDEN_LS] };
  },

  backup_ui: () => {
    // One key per destination+account, dynamically named, so this is the only way to find them all.
    // Snapshot the names first: removing while iterating `localStorage.key(i)` reindexes the store.
    const from: string[] = [];
    const dismissed: string[] = [];
    try {
      for (let i = 0; i < localStorage.length; i += 1) {
        const name = localStorage.key(i);
        if (name === null || !name.startsWith(BACKUP_DISMISSED_LS_PREFIX)) continue;
        from.push(name);
        if (localStorage.getItem(name) === "true") {
          dismissed.push(name.slice(BACKUP_DISMISSED_LS_PREFIX.length));
        }
      }
    } catch {
      return null; // localStorage unavailable (private mode / disabled)
    }
    return from.length ? { blob: { reconcileDismissed: dismissed }, from } : null;
  },
};

function readLs(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function removeLs(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* best-effort — a stale copy is inert now that the store is authoritative */
  }
}
