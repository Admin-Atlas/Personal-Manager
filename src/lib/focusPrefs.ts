// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device layout pref for the Focus tab, shared by the Focus header toggle and the Settings →
// General control (both read/write the same key, so the default lives here once). "split" (the
// default) puts the briefing / actions / agenda beside the project list on a wide screen; "vertical"
// keeps the single stacked column. Display-only with no backend consumer, so it lives in
// localStorage — never a backend Setting (mirrors mapPrefs).

export type FocusLayout = "split" | "vertical";

export const FOCUS_LAYOUT_KEY = "pm.focus.layout";

/** The stored layout; anything but "vertical" (including absent) is the split default. */
export function readFocusLayout(): FocusLayout {
  try {
    return localStorage.getItem(FOCUS_LAYOUT_KEY) === "vertical" ? "vertical" : "split";
  } catch {
    return "split";
  }
}

export function writeFocusLayout(layout: FocusLayout): void {
  try {
    localStorage.setItem(FOCUS_LAYOUT_KEY, layout);
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}
