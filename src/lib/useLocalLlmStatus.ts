// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import { localLlmStatus, onLocalLlmStatus } from "./ipc";
import type { LocalLlmStatus } from "./types";

/**
 * The live local-endpoint status for the chat provider surfaces (the sidebar "Local" line and the
 * composer ProviderChip). One instance is mounted in App and passed down (the "subscribe once"
 * rule), so the two surfaces share a single fetch + listener rather than each polling.
 *
 * It fetches once on mount, refetches whenever the backend pings `local-llm://status` (a call
 * succeeded/failed — opening or closing a cooldown — or the endpoint was (re)configured/cleared),
 * and, because a cooldown ENDS on a timer with no event, schedules one extra refetch at the
 * cooldown deadline so "resting" clears on its own. Returns `null` until the first fetch resolves;
 * never throws — a failed refetch keeps the last-known snapshot.
 */
export function useLocalLlmStatus(): LocalLlmStatus | null {
  const [status, setStatus] = useState<LocalLlmStatus | null>(null);
  // Guards a refetch that resolves after unmount, and holds the single cooldown-expiry timer.
  const aliveRef = useRef(true);
  const cooldownTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    aliveRef.current = true;

    const clearTimer = () => {
      if (cooldownTimer.current) {
        clearTimeout(cooldownTimer.current);
        cooldownTimer.current = null;
      }
    };

    const refetch = () => {
      localLlmStatus()
        .then((s) => {
          if (!aliveRef.current) return;
          setStatus(s);
          clearTimer();
          // A cooldown lifts by a timer, not an event — schedule the one refetch that catches it
          // (a second past the deadline, so the backend has already cleared it).
          if (s.in_cooldown && s.cooldown_remaining_s > 0) {
            cooldownTimer.current = setTimeout(refetch, (s.cooldown_remaining_s + 1) * 1000);
          }
        })
        .catch(() => {
          /* keep the last-known snapshot — a transient read failure isn't a state change */
        });
    };

    refetch();
    let unlisten: (() => void) | undefined;
    void onLocalLlmStatus(refetch).then((u) => {
      if (aliveRef.current) unlisten = u;
      else u(); // unmounted before the listener attached — drop it immediately
    });

    return () => {
      aliveRef.current = false;
      clearTimer();
      unlisten?.();
    };
  }, []);

  return status;
}
