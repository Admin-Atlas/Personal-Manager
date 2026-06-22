// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The launch lock screen for the optional biometric app-lock (a soft UI gate — the store
// is already decrypted; this only withholds the window until the OS verifies the user).
// It auto-prompts once on mount, lets the user retry, and — only when the device can't
// verify at all — offers an honest escape so a broken/removed sensor can't trap the user.

import { useCallback, useEffect, useRef, useState } from "react";
import { unlockApp } from "../lib/ipc";
import { Button } from "./ui";

/** verifying = OS prompt up; canceled = user dismissed/failed (device can verify, just retry);
 *  error = the verifier couldn't run (offer the escape). */
type Phase = "verifying" | "canceled" | "error";

export function LockScreen({ onUnlocked }: { onUnlocked: () => void }) {
  const [phase, setPhase] = useState<Phase>("verifying");
  const [detail, setDetail] = useState<string | null>(null);
  // Stop overlapping prompts from a double-click or StrictMode's double-mounted effect.
  const inFlight = useRef(false);

  const attempt = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setPhase("verifying");
    setDetail(null);
    try {
      const ok = await unlockApp();
      if (ok) {
        onUnlocked();
        return;
      }
      setPhase("canceled");
    } catch (e) {
      setPhase("error");
      setDetail(String(e));
    } finally {
      inFlight.current = false;
    }
  }, [onUnlocked]);

  // Prompt automatically the first time the lock screen appears.
  useEffect(() => {
    void attempt();
  }, [attempt]);

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* A simple padlock glyph — no icon dep. */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect
            x="4"
            y="10"
            width="16"
            height="10"
            rx="2"
            stroke="currentColor"
            strokeWidth="1.6"
          />
          <path
            d="M8 10V7a4 4 0 1 1 8 0v3"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      </div>

      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">PM is locked</h1>
        <p className="mt-1 max-w-xs text-sm text-ink4">
          {phase === "verifying"
            ? "Waiting for you to verify…"
            : phase === "canceled"
              ? "Verification was cancelled. Try again to continue."
              : "Couldn't verify on this device."}
        </p>
      </div>

      {phase === "error" && detail && (
        <p
          className="max-w-xs rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {detail}
        </p>
      )}

      <div className="flex flex-col items-center gap-2">
        <Button variant="primary" onClick={() => void attempt()} disabled={phase === "verifying"}>
          {phase === "verifying" ? "Verifying…" : "Unlock"}
        </Button>

        {/* Escape hatch only when the device genuinely can't verify — the lock guards the
            window, not your encrypted data, so a broken sensor must not lock you out. */}
        {phase === "error" && (
          <button
            type="button"
            onClick={onUnlocked}
            className="font-ui text-xs text-ink4 underline underline-offset-2 hover:text-ink2"
          >
            Open without verifying
          </button>
        )}
      </div>
    </div>
  );
}
