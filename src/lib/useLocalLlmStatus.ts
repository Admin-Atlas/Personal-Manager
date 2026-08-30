// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { localLlmStatus, onLocalLlmStatus } from "./ipc";
import { subscribeUntilCleanup } from "./subscribe";
import type { LocalLlmStatus } from "./types";

/**
 * How long to gather backend pings before refetching.
 *
 * The slot now announces the start AND the end of every local call, so a run of background jobs
 * emits two events apiece. Each one is a fact worth having and none of them is worth a separate
 * round trip: the display is describing states that last seconds, and a quarter of a second late is
 * invisible. Trailing rather than leading, so the fetch that lands always describes the settled
 * state rather than one the burst has already moved past.
 */
const PING_COALESCE_MS = 250;

/**
 * How often to re-ask while a local endpoint is configured and the window is on screen.
 *
 * Everything PM causes arrives as an event; this is for the one thing it does not cause. A server
 * unloads a model on its own schedule — Ollama's default is five idle minutes — and says nothing
 * when it does, so without a tick the footer would keep reporting a model that left the card
 * whenever the user was doing anything other than talking to it, which is exactly when it leaves.
 *
 * What it costs, stated plainly rather than waved away. The backend debounces the actual probe to
 * `HEALTH_PROBE_DEBOUNCE` (30 s) behind a single global token, so the tick can never spam anything
 * however fast it runs — but a user sitting in Chat with the Local AI tab closed previously made NO
 * periodic requests at all, and now makes about three a minute to their own server: one
 * `/v1/models`, and `/slots` + `/api/ps` when a role has a model bound. Against a loopback server
 * that is nothing; it is still an addition, and the reason it is worth making is that the
 * alternative is a footer that states a fact it stopped checking.
 */
const LIVE_POLL_MS = 20_000;

/**
 * The live local-endpoint status for the provider surfaces (the sidebar model footer and the
 * composer ProviderChip). One instance is mounted in App and passed down (the "subscribe once"
 * rule), so every surface shares a single fetch + listener rather than each polling.
 *
 * It fetches once on mount, refetches (coalesced) whenever the backend pings `local-llm://status`
 * — a call starting or finishing, a cooldown opening or closing, the endpoint being reconfigured,
 * a model being released — schedules one extra refetch at a cooldown deadline (which ends on a
 * timer with no event), and ticks slowly while an endpoint is configured and the window is visible.
 * Returns `null` until the first fetch resolves; never throws — a failed refetch keeps the
 * last-known snapshot.
 *
 * `ready` is the app-scope gate every other background poll already sits behind: with the vault
 * shut there is no settings row to read, so the tick would be a guaranteed error every 20 s. The
 * one-shot fetch is deliberately NOT gated — the surfaces have to render something on first paint,
 * and a single failed read costs nothing.
 */
export function useLocalLlmStatus(ready: boolean): LocalLlmStatus | null {
  const [status, setStatus] = useState<LocalLlmStatus | null>(null);
  // Guards a refetch that resolves after unmount. RE-ARMED at the top of the owning effect:
  // StrictMode's mount → unmount → mount would otherwise leave it false for the second mount, and
  // every guarded write would be silently dropped.
  const aliveRef = useRef(true);
  const cooldownTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const coalesceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refetch = useCallback(() => {
    localLlmStatus()
      .then((s) => {
        if (!aliveRef.current) return;
        setStatus(s);
        if (cooldownTimer.current) {
          clearTimeout(cooldownTimer.current);
          cooldownTimer.current = null;
        }
        // A cooldown lifts by a timer, not an event — schedule the one refetch that catches it
        // (a second past the deadline, so the backend has already cleared it).
        if (s.in_cooldown && s.cooldown_remaining_s > 0) {
          cooldownTimer.current = setTimeout(refetch, (s.cooldown_remaining_s + 1) * 1000);
        }
      })
      .catch(() => {
        /* keep the last-known snapshot — a transient read failure isn't a state change */
      });
  }, []);

  useEffect(() => {
    aliveRef.current = true;

    const ping = () => {
      if (coalesceTimer.current) return;
      coalesceTimer.current = setTimeout(() => {
        coalesceTimer.current = null;
        refetch();
      }, PING_COALESCE_MS);
    };

    refetch();
    const off = subscribeUntilCleanup(() => onLocalLlmStatus(ping));

    return () => {
      aliveRef.current = false;
      if (cooldownTimer.current) {
        clearTimeout(cooldownTimer.current);
        cooldownTimer.current = null;
      }
      if (coalesceTimer.current) {
        clearTimeout(coalesceTimer.current);
        coalesceTimer.current = null;
      }
      off();
    };
  }, [refetch]);

  // Read once more the moment the app becomes ready.
  //
  // The mount fetch runs above every gate in App — the vault curtain, the unlock screen — so on a
  // passphrase vault it fires while the store is still shut, `local_llm_status` fails on its first
  // DB read, and the snapshot stays null. Without this the footer then stayed blank for the whole
  // session: the tick below is gated on `configured`, which can only come FROM a successful read,
  // so the one failure locked the other out. Nothing else refetches until a call happens.
  useEffect(() => {
    if (ready) refetch();
  }, [ready, refetch]);

  // The slow tick. Armed only for someone who actually has a local endpoint (a cloud-only user
  // polls nothing at all), and only while the window is on screen — with the tray icon on, closing
  // the window merely hides it, so an ungated interval would keep asking the user's server about a
  // footer nobody can see.
  const configured = status?.configured ?? false;
  useEffect(() => {
    if (!ready || !configured) return;
    const tick = () => {
      if (document.visibilityState !== "hidden") refetch();
    };
    const id = setInterval(tick, LIVE_POLL_MS);
    // Coming back to the window is the moment the display is most likely to be wrong, and the one
    // moment it is certainly being looked at.
    document.addEventListener("visibilitychange", tick);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [ready, configured, refetch]);

  return status;
}
