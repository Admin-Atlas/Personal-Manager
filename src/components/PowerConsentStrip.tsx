// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { keepLocalOnBattery, setLocalPowerPolicy } from "../lib/ipc";
import { consentText, powerOf, scopeOf } from "../lib/powerRoute";
import type { LocalLlmStatus, PowerView } from "../lib/types";
import { Button, Callout } from "./ui";

/**
 * The one-time On battery question (#432), as an app-wide strip.
 *
 * Asked before the FIRST power reroute of any price, not only a paid one: a free cloud model still
 * takes what you send off this machine. Until it is answered PM stays on the local model, so nothing
 * is billed while the window is hidden to the tray or nobody is at the keyboard.
 *
 * Non-modal and never focused, so a stray Enter in the composer can't answer it and Escape can't
 * fight Settings or the palette over it. There is no ✕ on purpose: every way out is an answer, which
 * is what clears the backend's `consent_needed` — a dismiss would leave the question standing and the
 * strip would simply come back. Derived from App's one status snapshot, so it needs no listener of
 * its own and is already waiting when the window is reopened from the tray.
 */
export function PowerConsentStrip({ status }: { status: LocalLlmStatus | null }) {
  if (!status) return null;
  const power = powerOf(status);
  if (!power.consent_needed) return null;
  return (
    <Callout
      tone="info"
      variant="strip"
      size="md"
      body="ink"
      live
      className="flex flex-wrap items-start justify-between gap-3"
    >
      <PowerConsentAsk power={power} />
    </Callout>
  );
}

/**
 * The question and its three answers, shared by the strip and Settings → Local AI → On battery
 * (the Settings overlay covers the strip, so the section asks inline too). Renders two siblings —
 * the words and the buttons — for the caller's own flex row to lay out.
 *
 * The safe answer comes first. "Keep using local" is the in-memory override, so PM asks again after
 * a restart; "Never switch" is the persistent no, stored as threshold 0.
 */
export function PowerConsentAsk({ power }: { power: PowerView }) {
  // "sent" stays disabled until a snapshot arrives AFTER the answer: every one of these commands
  // pings the status, and that snapshot either unmounts this or, if it still asks, hands the buttons
  // back. Re-enabling on the resolve alone would offer a second answer to a question already gone.
  const [busy, setBusy] = useState<"sending" | "sent" | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    setBusy((b) => (b === "sent" ? null : b));
  }, [power]);

  function answer(send: () => Promise<void>) {
    setBusy("sending");
    setFailed(false);
    void send().then(
      () => setBusy("sent"),
      () => {
        setBusy(null);
        setFailed(true);
      },
    );
  }

  const asked = scopeOf(
    power.chat.route === "needs_consent",
    power.background.route === "needs_consent",
  );
  const disabled = busy !== null;
  return (
    <>
      <div className="min-w-0 flex-1 space-y-1">
        <p>{consentText(power)}</p>
        <p className="text-xs">
          PM asks this once. If something else becomes able to move later, it asks once about that
          too. "Keep using local" lasts until you quit PM, and PM asks again next time. You can
          change any of this in Settings → Local AI → On battery.
        </p>
        {failed && <p className="text-xs text-st-due">Couldn't save that. Try again.</p>}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="secondary"
          size="sm"
          disabled={disabled}
          onClick={() => answer(() => keepLocalOnBattery(true))}
        >
          Keep using local
        </Button>
        <Button
          variant="primary"
          size="sm"
          disabled={disabled || asked == null}
          // The yes is about exactly the roles the question named, and covers nothing else.
          onClick={() => {
            if (asked) answer(() => setLocalPowerPolicy({ consent: asked }));
          }}
        >
          Use the cloud on battery
        </Button>
        <Button
          variant="tertiary"
          size="sm"
          disabled={disabled}
          onClick={() => answer(() => setLocalPowerPolicy({ threshold: 0 }))}
        >
          Never switch on battery
        </Button>
      </div>
    </>
  );
}
