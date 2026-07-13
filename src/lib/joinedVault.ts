// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one-shot "just joined a shared vault" flag (issue #337), following the mapPrefs
// pattern for shared localStorage keys. Set right before the post-join reload; read by
// onboarding-adjacent surfaces (the Connectors tab banner) to explain, once, what stays
// personal on this account. localStorage lives in the per-Windows-user webview profile,
// so the flag can never leak to the vault owner's side. Dismissing clears it for good.

const JUST_JOINED_VAULT_KEY = "pm:justJoinedVault";

/** Mark this profile as having just joined a shared vault (called before the reload). */
export function markJustJoinedVault() {
  localStorage.setItem(JUST_JOINED_VAULT_KEY, "1");
}

/** Whether the one-shot joined-vault explanation is still owed on this profile. */
export function justJoinedVault(): boolean {
  return localStorage.getItem(JUST_JOINED_VAULT_KEY) === "1";
}

/** Clear the flag (the user dismissed the explanation). */
export function clearJustJoinedVault() {
  localStorage.removeItem(JUST_JOINED_VAULT_KEY);
}
