// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Saved ✓" signal behind the Settings footer.
//
// Settings writes on change — there is no batch to confirm — so the footer used to carry a standing
// line of prose ("Changes are saved as you make them"). Honest, but passive: it says the same thing
// whether or not anything just happened. This turns it into a real acknowledgement.
//
// THE ROT PROBLEM AND WHY THIS SHAPE: a tick fed by a hand-maintained list of settings commands is
// worse than no tick, because the failure mode is silence on a save that DID happen — the user
// learns the indicator can't be trusted. So this is not a list of commands to announce. It is a
// small deny-list of `set_*` commands that write user CONTENT rather than settings, and everything
// else matching the prefix announces. A new setting command is therefore announced the day it is
// added, with no one having to remember. The deny-list can only ever cause a spurious tick, never a
// missing one — and it is only *listened* to while the Settings overlay is open, where none of the
// denied commands are reachable anyway.
//
// The announce point is `ipc.ts`'s single internal `invoke`, for the same reason the VaultError
// normalisation lives there: no caller can be missed by a per-wrapper list.

/** Fired after a settings-shaped command completes successfully. */
export const SETTING_SAVED_EVENT = "pm:setting-saved";

/** `set_*` commands that write user CONTENT, not a preference. None of these is reachable from the
 *  Settings overlay; they are listed so the predicate stays honest if it is ever read elsewhere. */
const CONTENT_WRITES: ReadonlySet<string> = new Set([
  "set_document_metadata",
  "set_project_metadata",
  "set_conversation_project",
  "set_milestone_event",
  "set_milestone_state",
  "set_milestone_status",
  // A view cache PM maintains for itself — the user never "changed a setting" by panning the map.
  "set_project_layout",
]);

/** Whether a command name is a settings write worth acknowledging. Pure, so the boundary is tested
 *  rather than asserted. */
export function isSettingWrite(cmd: string): boolean {
  return cmd.startsWith("set_") && !CONTENT_WRITES.has(cmd);
}

/** Announce a completed settings write. Guarded for non-DOM contexts (the pure test env). */
export function announceSettingSaved(): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new Event(SETTING_SAVED_EVENT));
}

/** Subscribe to settings writes; returns an unsubscriber.
 *
 *  Also listens for `pm:settings-changed`, the app's existing cross-surface signal, so the
 *  localStorage-backed preferences (theme, Depth, the calendar and focus prefs) — which never touch
 *  the backend and so never reach `invoke` — acknowledge too. Those are the majority of the General
 *  tab, and a tick that ignored them would be exactly the untrustworthy indicator this is trying
 *  not to be. */
export function onSettingSaved(fn: () => void): () => void {
  window.addEventListener(SETTING_SAVED_EVENT, fn);
  window.addEventListener("pm:settings-changed", fn);
  return () => {
    window.removeEventListener(SETTING_SAVED_EVENT, fn);
    window.removeEventListener("pm:settings-changed", fn);
  };
}
