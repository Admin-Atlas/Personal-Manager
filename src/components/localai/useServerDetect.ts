// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

import { probeLocalLlmPorts } from "../../lib/ipc";
import type { DetectedEndpoint } from "../../lib/types";

/** How often PM looks again while nothing is connected. */
const EVERY_MS = 30_000;
/** Coming back to the window looks again, but not more often than this. */
const FOCUS_THROTTLE_MS = 5_000;

/** The local servers PM can find, and a way to look again — what `useServerDetect` hands down. */
export interface ServerDetect {
  /** What the last look found, or null before one has answered (and whenever something is
   *  connected — once connected, the tab reads the status, not the probe). */
  detected: DetectedEndpoint[] | null;
  detecting: boolean;
  /** Look now. Resolves to what it found; a look already running is shared, not doubled. */
  detect: () => Promise<DetectedEndpoint[]>;
}

/**
 * Looks for a model server on this computer while none is connected: on mount, every half minute,
 * and when the window regains focus — someone who has just installed and started Ollama comes back to
 * PM, and should find it noticed rather than having to say so.
 *
 * The tab's, so the start card and Model server read one answer: two probes would be two answers,
 * a moment apart. A failed probe is "nothing found", never an error — the ports are the user's, and
 * nothing answering on them is the ordinary state of a fresh install.
 */
export function useServerDetect(configured: boolean): ServerDetect {
  const [detected, setDetected] = useState<DetectedEndpoint[] | null>(null);
  const [detecting, setDetecting] = useState(false);
  const pending = useRef<Promise<DetectedEndpoint[]> | null>(null);
  const lastAt = useRef(0);
  // Read when a look lands, so one that was sent before connecting can't fill the list after it.
  const configuredRef = useRef(configured);
  useEffect(() => {
    configuredRef.current = configured;
  }, [configured]);

  const detect = useCallback((): Promise<DetectedEndpoint[]> => {
    if (pending.current) return pending.current;
    lastAt.current = Date.now();
    setDetecting(true);
    const run = (async () => {
      try {
        const found = (await probeLocalLlmPorts().catch(() => [])) ?? [];
        if (!configuredRef.current) setDetected(found);
        return found;
      } finally {
        pending.current = null;
        setDetecting(false);
      }
    })();
    pending.current = run;
    return run;
  }, []);

  useEffect(() => {
    if (configured) {
      setDetected(null);
      return;
    }
    // Disconnecting lands here too, so PM looks again straight away.
    void detect();
    const id = setInterval(() => void detect(), EVERY_MS);
    const onFocus = () => {
      if (Date.now() - lastAt.current >= FOCUS_THROTTLE_MS) void detect();
    };
    window.addEventListener("focus", onFocus);
    return () => {
      clearInterval(id);
      window.removeEventListener("focus", onFocus);
    };
  }, [configured, detect]);

  return { detected, detecting, detect };
}
