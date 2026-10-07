// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import {
  getLocalReleasePolicy,
  localGpuResidency,
  releaseLocalGpu,
  setLocalReleasePolicy,
} from "../../lib/ipc";
import type { LocalGpuResidency, LocalLlmStatus } from "../../lib/types";

/** Which of the two stored release settings a write was for. */
export type ReleaseField = "policy" | "battery";

/** A write that failed, and which setting it was for. "restored": the pickers show what is really
 *  stored. "unknown": reading it back failed too, so the pickers show nothing. */
export type ReleaseSaveError = { field: ReleaseField; kind: "restored" | "unknown" };

/** The stored release settings, the live residency readout, and the writes — what
 *  `useReleaseSettings` hands down. */
export interface ReleaseSettings {
  /** "server" | "on-exit" | "idle", or null until the stored one is read. */
  policy: string | null;
  idleMinutes: number;
  /** "On battery, hand the memory back", in minutes (0 = as the policy says), or null until read. */
  batteryIdle: number | null;
  saveError: ReleaseSaveError | null;
  /** undefined until the first read, null when PM couldn't ask. */
  residency: LocalGpuResidency | null | undefined;
  releasing: boolean;
  /** How many models the last Release now freed, or null. */
  freed: number | null;
  /** Re-read what the server is holding. */
  refresh: () => void;
  change: (nextPolicy: string, nextMinutes: number) => void;
  changeBatteryIdle: (nextMinutes: number) => void;
  release: () => Promise<void>;
}

/**
 * Giving the graphics card back (#786 item 8): the stored policy, the on-battery release (#432),
 * what the server is holding right now, and Release now.
 *
 * A hook rather than one section's state, so that every section showing these settings reads one
 * copy: two copies of "what PM has stored" would be two answers.
 *
 * `status` is the tab's live status, and it is what keeps the residency readout honest. What the
 * server holds changes without anything here being touched — a test loads a model, a reply ends, a
 * quiet period expires — and the status says so (loaded, released). So the readout is
 * re-read whenever those change, rather than going on reporting "the graphics card is free" over a
 * model the last test just loaded. `enabled: false` makes the hook inert, for a section that was
 * handed the tab's instance and only holds its own because hooks can't be conditional.
 */
export function useReleaseSettings({
  status,
  enabled = true,
}: {
  status: LocalLlmStatus | null;
  enabled?: boolean;
}): ReleaseSettings {
  // null until the stored policy is read, like the battery row below: a picker showing "Leave it to
  // my server" before PM has looked would present a default as the user's choice, and would go on
  // presenting it if the read failed.
  const [policy, setPolicy] = useState<string | null>(null);
  const [idleMinutes, setIdleMinutes] = useState(5);
  // null until the stored value is read, and left null if the read fails: the row is disabled then,
  // rather than presenting "off" as though PM had said so.
  const [batteryIdle, setBatteryIdle] = useState<number | null>(null);
  const [saveError, setSaveError] = useState<ReleaseSaveError | null>(null);
  const [residency, setResidency] = useState<LocalGpuResidency | null | undefined>(undefined);
  const [releasing, setReleasing] = useState(false);
  const [freed, setFreed] = useState<number | null>(null);

  const refresh = useCallback(() => {
    void localGpuResidency()
      .then(setResidency)
      .catch(() => setResidency(null));
  }, []);

  // `failed`: this read is checking what a failed write to that field left behind. If it fails too,
  // PM knows neither what it tried to store nor what is stored, so both pickers go back to unknown —
  // leaving the unsaved choice on screen beside "this shows what PM has stored" would be a lie.
  const readStored = useCallback((failed?: ReleaseField) => {
    void getLocalReleasePolicy()
      .then((s) => {
        setPolicy(s.policy);
        setIdleMinutes(s.idle_minutes);
        setBatteryIdle(s.battery_idle_minutes ?? null);
        if (failed) setSaveError({ field: failed, kind: "restored" });
      })
      .catch(() => {
        if (failed) {
          setPolicy(null);
          setBatteryIdle(null);
          setSaveError({ field: failed, kind: "unknown" });
        }
        /* otherwise the pickers simply stay unknown and disabled */
      });
  }, []);

  useEffect(() => {
    if (!enabled) return;
    readStored();
    refresh();
  }, [enabled, readStored, refresh]);

  // Everything in the status that says the server's memory just changed hands. Not the whole
  // status: it is re-sent on every push, and most of it (the window, the battery) says nothing
  // about what is loaded. Not "answering" either: that flips at the start and end of every local
  // call, and a residency read is a request to the user's server — two per reply, for a change
  // `*_loaded` already reports when the call that loaded the model completes.
  const fingerprint = status
    ? `${status.chat_loaded}|${status.background_loaded}|${status.chat_released}|${status.background_released}`
    : null;
  const lastFingerprint = useRef(fingerprint);
  useEffect(() => {
    if (lastFingerprint.current === fingerprint) return;
    lastFingerprint.current = fingerprint;
    if (enabled) refresh();
  }, [fingerprint, enabled, refresh]);

  /** A write failed: show what PM really has stored, and say so. This used to be swallowed, which
   *  left the picker showing a choice that was never saved — the setting would quietly not apply. */
  function restore(field: ReleaseField) {
    readStored(field);
  }

  function change(nextPolicy: string, nextMinutes: number) {
    setPolicy(nextPolicy);
    setIdleMinutes(nextMinutes);
    setFreed(null);
    void setLocalReleasePolicy(nextPolicy, nextMinutes).then(
      () => setSaveError(null),
      () => restore("policy"),
    );
  }

  function changeBatteryIdle(nextMinutes: number) {
    setBatteryIdle(nextMinutes);
    void setLocalReleasePolicy(null, undefined, nextMinutes).then(
      () => setSaveError(null),
      () => restore("battery"),
    );
  }

  async function release() {
    setReleasing(true);
    setFreed(null);
    try {
      setFreed(await releaseLocalGpu());
    } catch {
      setFreed(null);
    } finally {
      setReleasing(false);
      refresh();
    }
  }

  return {
    policy,
    idleMinutes,
    batteryIdle,
    saveError,
    residency,
    releasing,
    freed,
    refresh,
    change,
    changeBatteryIdle,
    release,
  };
}
