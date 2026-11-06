// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The decisions behind a Settings → Storage row that carries a feature switch (voice input's, Mods
// spec D8), kept pure so the combinations of switch and engine state are tested without rendering.

import type { StorageComponent, StorageReport } from "./types";

/** One button a switch row offers. */
export type SwitchAction = "turn-on" | "install" | "install-again" | "turn-off" | "remove";

/** The buttons a row with a feature switch offers, in order. While on, the engine goes through Turn
 *  off (which offers to remove it), beside a repair when it is missing or stale; while off, Turn on
 *  installs it, and Remove frees the space an engine left behind. A row without a switch: none. */
export function switchActions(c: StorageComponent): SwitchAction[] {
  if (c.switch === null) return [];
  if (c.switch) {
    if (c.status === "installable") return ["install", "turn-off"];
    if (c.status === "stale") return ["install-again", "turn-off"];
    return ["turn-off"];
  }
  if (c.status === "removable" || c.status === "stale") return ["turn-on", "remove"];
  return ["turn-on"];
}

/** What switching voice off can remove: the engine (when it is on disk) and the speech model's
 *  weights (their own row), with the bytes that frees. An engine that isn't installed carries an
 *  install estimate, which frees nothing. */
export function voiceRemovable(report: StorageReport): { any: boolean; bytes: number } {
  let bytes = 0;
  let any = false;
  for (const c of report.components) {
    const engine = c.id === "voice" && (c.status === "removable" || c.status === "stale");
    if (engine || c.id === "whisper") {
      any = true;
      bytes += c.size_bytes;
    }
  }
  return { any, bytes };
}
