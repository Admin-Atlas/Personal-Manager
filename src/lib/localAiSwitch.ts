// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";

import { getLocalAiSwitch, setLocalAiSwitch } from "./ipc";
import type { LocalAiSwitch } from "./types";

// The Local AI switch (D7) for the webview. The switch itself is a backend setting, enforced in
// Rust (`local_ai_switch.rs`); this is only how the surfaces that show or hide Local AI learn where
// it stands and hear when it moves. Three of them change at once when it does: the Settings rail
// (the tab and its dot), AI & Models (its row and the "pick a model" pickers) and the Local AI tab
// (its switch), so a write here is announced to all of them rather than each re-reading on its own.

/** Fired after the switch changes, with the new `LocalAiSwitch` as its detail. */
export const LOCAL_AI_SWITCH_EVENT = "pm:local-ai-switch";

/** Tell every reader of the switch where it now stands. */
export function announceLocalAiSwitch(next: LocalAiSwitch): void {
  if (typeof window === "undefined") return;
  window.dispatchEvent(new CustomEvent<LocalAiSwitch>(LOCAL_AI_SWITCH_EVENT, { detail: next }));
}

/** Switch Local AI on or off, then tell every surface. Rejects with the backend's error, having
 *  announced nothing: a switch that failed has not moved. */
export async function setLocalAiEnabled(on: boolean): Promise<LocalAiSwitch> {
  const next = await setLocalAiSwitch(on);
  announceLocalAiSwitch(next);
  return next;
}

/** Re-read the switch and announce it, for a write that changes what it reports without moving it
 *  (a role given the cloud model, a server forgotten). Rejects with the backend's error. */
export async function refreshLocalAiSwitch(): Promise<LocalAiSwitch> {
  const next = await getLocalAiSwitch();
  announceLocalAiSwitch(next);
  return next;
}

/**
 * The switch for a component: `null` until the backend has answered (or if it couldn't), then the
 * stored answer, kept current by every announcement. Read fresh on each mount rather than cached
 * across them, because the store it comes from can change under the app (a vault switch, a
 * restore), and a cached answer from the last one is exactly what must not be shown.
 *
 * `null` is not "off". A surface that shows Local AI only when it is on treats it as hidden; a
 * surface that would act on "off" (moving the reader off a tab that vanished) waits for an answer.
 */
export function useLocalAiSwitch(): LocalAiSwitch | null {
  const [sw, setSw] = useState<LocalAiSwitch | null>(null);
  // An announcement that lands while the first read is in flight is newer than that read's answer,
  // which must then not overwrite it.
  const announced = useRef(false);
  useEffect(() => {
    let alive = true;
    const onSwitch = (e: Event) => {
      announced.current = true;
      setSw((e as CustomEvent<LocalAiSwitch>).detail);
    };
    window.addEventListener(LOCAL_AI_SWITCH_EVENT, onSwitch);
    getLocalAiSwitch()
      .then((s) => {
        if (alive && !announced.current) setSw(s);
      })
      .catch(() => {
        /* stays null: unknown, which every reader treats as "don't show Local AI" */
      });
    return () => {
      alive = false;
      window.removeEventListener(LOCAL_AI_SWITCH_EVENT, onSwitch);
    };
  }, []);
  return sw;
}

/** Whether Local AI should be shown, for the surfaces that vanish while it is off. Unknown is
 *  hidden: showing a Workbench that may be switched off would invite a click the backend refuses. */
export function localAiShown(sw: LocalAiSwitch | null): boolean {
  return sw?.on === true;
}
