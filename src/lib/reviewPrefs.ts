// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device toggle: whether the Review tab asks the AI to propose a project, tags and importance for
// each item, or the user files everything by hand. Default OFF — a fresh install has no model set up,
// and the AI is an enhancement, never a requirement; the Review banner nudges you to turn it on (which
// really helps when importing a lot). Shared by the Review banner button and the Settings → AI & Models
// control, so the default lives here once (mirrors focusPrefs). A frontend gate with no backend
// consumer, so it lives in localStorage — never a backend Setting.

export const REVIEW_AI_KEY = "pm.review.aiSuggestions";

/** Whether Review should ask the model for suggestions. Absent (fresh install) = off. */
export function readReviewAiEnabled(): boolean {
  try {
    return localStorage.getItem(REVIEW_AI_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeReviewAiEnabled(enabled: boolean): void {
  try {
    localStorage.setItem(REVIEW_AI_KEY, String(enabled));
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}
