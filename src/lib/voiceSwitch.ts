// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";

import { getVoiceSwitch, setVoiceSwitch } from "./ipc";
import type { VoiceSwitch } from "./types";

// The voice switch (Mods spec D8) for the webview. The switch itself is a backend setting, enforced
// in Rust (`voice_switch.rs`: `transcribe_audio` refuses while it is off); this is only how the
// Composer's mic learns where it stands and hears when it moves. It moves from Settings → Storage,
// which is a modal over the app, so a chat underneath has to hear the change rather than re-read on
// its next mount.

/** Fired after the switch changes, with the new `VoiceSwitch` as its detail. */
export const VOICE_SWITCH_EVENT = "pm:voice-switch";

/** Tell every reader of the switch where it now stands. */
export function announceVoiceSwitch(next: VoiceSwitch): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<VoiceSwitch>(VOICE_SWITCH_EVENT, { detail: next }));
}

/** Re-read the switch and announce it. Rejects with the backend's error. */
export async function refreshVoiceSwitch(): Promise<VoiceSwitch> {
  const next = await getVoiceSwitch();
  announceVoiceSwitch(next);
  return next;
}

/** Switch voice on (which installs the speech engine first) or off (and, with `removeEngine`,
 *  remove the engine and the speech model), then tell every surface. A failure is re-read before
 *  it is passed on: switching off writes the switch before it removes anything, so a removal that
 *  failed has still turned voice off, and the mic must hear that. */
export async function setVoiceEnabled(on: boolean, removeEngine = false): Promise<VoiceSwitch> {
  try {
    const next = await setVoiceSwitch(on, removeEngine);
    announceVoiceSwitch(next);
    return next;
  } catch (e) {
    await refreshVoiceSwitch().catch(() => {
      /* the original error is the one worth showing */
    });
    throw e;
  }
}

/**
 * The switch for a component: `null` until the backend has answered (or if it couldn't), then the
 * stored answer, kept current by every announcement. Read fresh on each mount, because the store it
 * comes from can change under the app (a vault switch, a restore).
 */
export function useVoiceSwitch(): VoiceSwitch | null {
  const [sw, setSw] = useState<VoiceSwitch | null>(null);
  // An announcement that lands while the first read is in flight is newer than that read's answer,
  // which must then not overwrite it.
  const announced = useRef(false);
  useEffect(() => {
    let alive = true;
    const onSwitch = (e: Event) => {
      announced.current = true;
      setSw((e as CustomEvent<VoiceSwitch>).detail);
    };
    window.addEventListener(VOICE_SWITCH_EVENT, onSwitch);
    getVoiceSwitch()
      .then((s) => {
        if (alive && !announced.current) setSw(s);
      })
      .catch(() => {
        /* stays null: unknown, which hides the mic */
      });
    return () => {
      alive = false;
      window.removeEventListener(VOICE_SWITCH_EVENT, onSwitch);
    };
  }, []);
  return sw;
}

/** Whether the mic should be shown. Unknown is hidden: a mic the backend may refuse would invite a
 *  recording that can't be transcribed. */
export function voiceShown(sw: VoiceSwitch | null): boolean {
  return sw?.on === true;
}
