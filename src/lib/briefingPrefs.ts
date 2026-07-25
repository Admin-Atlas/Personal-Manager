// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where the user has asked to see the daily briefing, beyond the Focus tab. Display-only with no
// backend consumer, so it lives in localStorage — never a backend Setting (mirrors focusPrefs).
//
// BOTH default to off: the Focus card is the shipped behaviour, and turning on an extra surface is a
// deliberate choice rather than something an update does to you.
//
// Unlike the other pref modules these are read by surfaces that stay MOUNTED while Settings is open
// (Settings renders as an overlay over the live app), so a plain read-at-mount would leave the
// toggle looking broken until the user navigated away and back. `subscribeBriefingPrefs` closes
// that gap: the writers announce a change and the surfaces re-read. The app already has a
// `pm:settings-changed` convention for exactly this, so this reuses that event name rather than
// inventing a second one.

const SIDEBAR_KEY = "pm.briefing.sidebar";
const WINDOW_KEY = "pm.briefing.window";

/** The app-wide "a setting changed" signal (dispatched by Settings; listened to by live surfaces). */
const CHANGED_EVENT = "pm:settings-changed";

function readFlag(key: string): boolean {
  try {
    return localStorage.getItem(key) === "true";
  } catch {
    return false;
  }
}

function writeFlag(key: string, on: boolean): void {
  try {
    localStorage.setItem(key, String(on));
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
  // Announce even if the write failed: the in-memory state of every mounted surface should still
  // follow the user's click for this session.
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

/** Show the briefing pinned in the left sidebar, above the model row and What's New / Settings. */
export function readBriefingInSidebar(): boolean {
  return readFlag(SIDEBAR_KEY);
}
export function writeBriefingInSidebar(on: boolean): void {
  writeFlag(SIDEBAR_KEY, on);
}

/** Show the briefing as a floating panel that stays put across every tab. */
export function readBriefingWindow(): boolean {
  return readFlag(WINDOW_KEY);
}
export function writeBriefingWindow(on: boolean): void {
  writeFlag(WINDOW_KEY, on);
}

/** True when both surfaces are at their out-of-the-box default (off) — drives Settings' Reset. */
export function briefingPrefsAreDefault(): boolean {
  return !readBriefingInSidebar() && !readBriefingWindow();
}

export function resetBriefingPrefs(): void {
  writeBriefingInSidebar(false);
  writeBriefingWindow(false);
}

/**
 * Re-run `onChange` whenever a briefing-surface pref is written, so a surface that is already
 * mounted (the sidebar, the floating panel) follows a Settings toggle immediately instead of
 * waiting for a remount. Returns the unsubscribe.
 */
export function subscribeBriefingPrefs(onChange: () => void): () => void {
  window.addEventListener(CHANGED_EVENT, onChange);
  return () => window.removeEventListener(CHANGED_EVENT, onChange);
}
