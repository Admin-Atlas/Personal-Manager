// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where the user has asked to see the daily briefing, beyond the Focus tab. Display-only with no
// backend consumer, so these live in localStorage — never a backend Setting (mirrors focusPrefs).
//
// The one exception is the TRAY icon, which is deliberately NOT here: Rust reads it at boot to
// decide the icon's visibility and whether closing the main window quits or hides, so it belongs in
// the backend `settings` table. A pref with a backend consumer goes to the backend; this file is for
// the ones only the webview cares about.
//
// Everything defaults to OFF. The Focus card is the shipped behaviour and an update should never
// start putting the briefing somewhere new on its own.
//
// These are read by surfaces that stay MOUNTED while Settings is open (Settings renders as an
// overlay over the live app), so a plain read-at-mount would leave a toggle looking broken until the
// user navigated away and back. `subscribeBriefingPrefs` closes that gap, reusing the app's existing
// `pm:settings-changed` convention rather than inventing a second signal.

const SIDEBAR_KEY = "pm.briefing.sidebar";
const FLOAT_KEY = "pm.briefing.float";
/** Pre-3.77 key: a boolean "show the in-app floating panel". Read once for migration. */
const LEGACY_WINDOW_KEY = "pm.briefing.window";

const CHANGED_EVENT = "pm:settings-changed";

/**
 * How the briefing floats, if at all.
 *
 * - `off` — not floating (the default).
 * - `inApp` — a panel inside PM's own window. Light: no second webview, no extra memory.
 * - `onTop` — a real always-on-top OS window that floats over other applications too. Costs a
 *   second webview, which is real memory on a low-RAM machine, so it is opt-in rather than the
 *   default shape of "floating".
 */
export type BriefingFloat = "off" | "inApp" | "onTop";

const FLOATS: readonly BriefingFloat[] = ["off", "inApp", "onTop"];

function announce(): void {
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

/** Show the briefing pinned in the left sidebar, above the model row and What's New / Settings. */
export function readBriefingInSidebar(): boolean {
  try {
    return localStorage.getItem(SIDEBAR_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeBriefingInSidebar(on: boolean): void {
  try {
    localStorage.setItem(SIDEBAR_KEY, String(on));
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
  // Announce even if the write failed: every mounted surface should still follow the click for
  // this session.
  announce();
}

/** The float mode, migrating the pre-3.77 boolean ("was the in-app panel on?") on first read. */
export function readBriefingFloat(): BriefingFloat {
  try {
    const raw = localStorage.getItem(FLOAT_KEY);
    if (raw && (FLOATS as readonly string[]).includes(raw)) return raw as BriefingFloat;
    // Anyone who had the in-app panel switched on keeps exactly what they had.
    if (localStorage.getItem(LEGACY_WINDOW_KEY) === "true") return "inApp";
  } catch {
    /* fall through to the default */
  }
  return "off";
}

export function writeBriefingFloat(mode: BriefingFloat): void {
  try {
    localStorage.setItem(FLOAT_KEY, mode);
  } catch {
    /* best-effort */
  }
  announce();
}

/** True when the briefing shows nowhere but the Focus tab — its out-of-the-box state. */
export function briefingPrefsAreDefault(): boolean {
  return !readBriefingInSidebar() && readBriefingFloat() === "off";
}

export function resetBriefingPrefs(): void {
  writeBriefingInSidebar(false);
  writeBriefingFloat("off");
}

/**
 * Re-run `onChange` whenever a briefing-surface pref is written, so a surface that is already
 * mounted follows a Settings toggle immediately instead of waiting for a remount. Returns the
 * unsubscribe.
 */
export function subscribeBriefingPrefs(onChange: () => void): () => void {
  window.addEventListener(CHANGED_EVENT, onChange);
  return () => window.removeEventListener(CHANGED_EVENT, onChange);
}
