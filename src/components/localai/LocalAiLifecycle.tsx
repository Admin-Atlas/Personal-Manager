// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";

import { formatGib } from "../../lib/format";
import {
  getLocalReleasePolicy,
  localGpuResidency,
  releaseLocalGpu,
  setLocalReleasePolicy,
} from "../../lib/ipc";
import type { LocalGpuResidency } from "../../lib/types";
import { Button, SectionInfo, SectionLabel, Select, SettingRow } from "../ui";
import { TrayIconRow } from "../settings/TrayIconRow";

/** How the three policies are worded, and — the part users actually need — when each one suits. */
const POLICIES: ReadonlyArray<{ value: string; label: string; when: string }> = [
  {
    value: "server",
    label: "Leave it to my server",
    when: "PM changes nothing. Your server decides how long to keep a model in memory, exactly as it does now. The right choice if you have already configured that yourself, or if the machine is a desktop that is not short of memory.",
  },
  {
    value: "on-exit",
    label: "When I quit PM",
    when: "The model stays loaded the whole time PM is open — no reloading between messages — and the memory comes back when you quit. With the tray icon on, closing the window does not quit PM, so the model stays: the session ends when the app does, not when the window does. This needs a normal quit; if the machine shuts PM down for you, as it does when you log out, there is no chance to hand anything back.",
  },
  {
    value: "idle",
    label: "After a quiet period",
    when: "The model is released once nothing has used it for a while, and again when you quit. Best on a laptop, or any machine where something else wants the graphics card. The cost is a few seconds the next time you use it, while the model loads again.",
  },
];

/**
 * Giving the graphics card back (#786 item 8).
 *
 * The one mechanism PM uses is an explicit unload. It never sets a keep-alive on its own requests,
 * because measurement showed a single request carrying one reprograms that server for the rest of its
 * life — every later request inherits it, including requests from other programs — which would
 * silently overwrite a setting the user chose. PM runs its own timer instead and leaves the server's
 * configuration alone.
 */
export function LocalAiLifecycle({ configured }: { configured: boolean }) {
  const [policy, setPolicy] = useState("server");
  const [idleMinutes, setIdleMinutes] = useState(5);
  const [residency, setResidency] = useState<LocalGpuResidency | null>(null);
  const [releasing, setReleasing] = useState(false);
  const [freed, setFreed] = useState<number | null>(null);

  const refresh = useCallback(() => {
    void localGpuResidency()
      .then(setResidency)
      .catch(() => setResidency(null));
  }, []);

  useEffect(() => {
    void getLocalReleasePolicy()
      .then((s) => {
        setPolicy(s.policy);
        setIdleMinutes(s.idle_minutes);
      })
      .catch(() => {
        /* leave the defaults — the section still renders */
      });
    refresh();
  }, [refresh]);

  function change(nextPolicy: string, nextMinutes: number) {
    setPolicy(nextPolicy);
    setIdleMinutes(nextMinutes);
    setFreed(null);
    void setLocalReleasePolicy(nextPolicy, nextMinutes).catch(() => {
      /* the next read corrects it; a failed write must not wedge the picker */
    });
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

  const chosen = POLICIES.find((p) => p.value === policy) ?? POLICIES[0];
  const resident = residency?.resident ?? null;
  const releasable = (resident ?? []).filter((m) => m.pm_loaded);

  return (
    <div
      id="sec-localai-lifecycle"
      data-settings-section
      data-help="settings-localai-lifecycle"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel>Holding the graphics card</SectionLabel>
      {!configured ? (
        <p className="mt-2 text-xs text-ink4">
          Connect an endpoint above and PM can tell you what is loaded, and hand the memory back
          when you are not using it.
        </p>
      ) : (
        <>
          {/* Unfolded: this is a status readout, and the doctrine folds prose but never those. */}
          <p className="mt-2 text-xs text-ink4">
            {resident === null ? (
              "PM couldn't ask your server what it has loaded — either it isn't answering, or it doesn't report that."
            ) : resident.length === 0 ? (
              "Nothing is loaded right now, so the graphics card is free."
            ) : (
              <>
                Your server is holding{" "}
                <span className="text-ink2">{resident.map((m) => m.model).join(", ")}</span>
                {residency?.vram_gb != null && (
                  <>
                    {" "}
                    — at least{" "}
                    <span className="text-ink2">
                      {formatGib(resident.reduce((n, m) => n + m.size_vram_gb, 0))}
                    </span>{" "}
                    of your {formatGib(residency.vram_gb)} card, and in practice somewhat more,
                    since a server doesn&rsquo;t count its own working memory
                  </>
                )}
                .{" "}
                {releasable.length === 0
                  ? "PM didn't load it, so PM won't unload it — that one is yours to manage."
                  : "PM loaded it, so PM can hand it back."}
              </>
            )}
          </p>

          {residency?.no_unload_route && (
            // Unfolded: a gating fact. Offering a picker that silently does nothing would be worse
            // than the absence of the feature.
            <p className="mt-1 text-xs text-st-due">
              This server has no way to unload a model on request, so none of the options below can
              do anything with it. llama-server keeps its model for as long as it is running, and LM
              Studio has no unload command — stopping the server is the only way to get the memory
              back. Ollama can do it.
            </p>
          )}

          {residency != null && residency.dgpu_displays.length > 0 && (
            // Surfaced, never acted on. Someone plugging in a monitor is quite likely sitting down
            // to work — releasing their model at that exact moment would be a scheduler acting on a
            // signal nobody agreed to.
            <p className="mt-1 text-xs text-ink4">
              An external display ({residency.dgpu_displays.join(", ")}) is also using this card. PM
              won&rsquo;t change anything because of that — it&rsquo;s usually a few hundred
              megabytes, and plugging in a screen normally means you are about to do more, not less.
            </p>
          )}

          <div className="mt-3 space-y-3">
            <SettingRow label="Give the memory back" helpId="settings-localai-lifecycle">
              {(a11y) => (
                <Select
                  {...a11y}
                  value={policy}
                  onChange={(e) => change(e.target.value, idleMinutes)}
                >
                  {POLICIES.map((p) => (
                    <option key={p.value} value={p.value}>
                      {p.label}
                    </option>
                  ))}
                </Select>
              )}
            </SettingRow>
            {/* Unfolded: what the chosen option will actually do is a gating fact, not prose. */}
            <p className="text-xs text-ink4">{chosen.when}</p>

            {policy === "idle" && (
              <SettingRow label="Quiet period" helpId="settings-localai-lifecycle">
                {(a11y) => (
                  <Select
                    {...a11y}
                    value={String(idleMinutes)}
                    onChange={(e) => change(policy, Number(e.target.value))}
                  >
                    {[1, 2, 5, 10, 15, 30, 60].map((m) => (
                      <option key={m} value={m}>
                        {m === 1 ? "1 minute" : `${m} minutes`}
                      </option>
                    ))}
                  </Select>
                )}
              </SettingRow>
            )}

            <TrayIconRow helpId="settings-tray-icon" />
            {/* The tray decides what "quit" means, so it belongs beside the policy that turns on it
                rather than one tab away. */}
            <p className="text-xs text-ink4">
              With this on, closing the window leaves PM running in the background — so a model
              stays loaded until you actually quit.
            </p>

            <div className="flex flex-wrap items-center gap-2">
              <Button
                variant="secondary"
                size="sm"
                onClick={() => void release()}
                disabled={releasing || releasable.length === 0}
              >
                {releasing ? "Releasing…" : "Release now"}
              </Button>
              {freed != null && (
                <span className="text-xs text-ink4">
                  {freed === 0
                    ? "Nothing to release."
                    : freed === 1
                      ? "Released one model."
                      : `Released ${freed} models.`}
                </span>
              )}
            </div>
          </div>
        </>
      )}

      <SectionInfo title="What PM does, and what it leaves alone">
        <p>
          PM only ever unloads a model <span className="text-ink2">it</span> loaded. One you started
          yourself in a terminal stays exactly where you put it.
        </p>
        <p>
          It also never changes how long your server keeps models by itself. Asking for that once
          would reprogram the server for the rest of its life — every later request from every
          program would inherit it — so PM runs its own timer and sends a plain unload instead.
        </p>
        <p>
          Releasing is never treated as a sign your server is healthy or broken. It is housekeeping,
          and it stays out of the reliability picture in both directions.
        </p>
      </SectionInfo>
    </div>
  );
}
