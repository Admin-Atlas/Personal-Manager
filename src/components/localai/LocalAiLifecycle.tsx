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
import type { LocalGpuResidency, PowerView } from "../../lib/types";
import { Button, SectionInfo, SectionLabel, Select, SettingRow } from "../ui";
import { TrayIconRow } from "../settings/TrayIconRow";

/** The quiet periods offered, shared by "Quiet period" and the on-battery release. */
const QUIET_MINUTES = [1, 2, 5, 10, 15, 30, 60];

/** "1 minute" / "5 minutes". */
function minutes(n: number): string {
  return n === 1 ? "1 minute" : `${n} minutes`;
}

/** The quiet periods plus a stored one that isn't among them (in order), so the Select shows what
 *  is really stored rather than snapping to a neighbour. */
function withStored(options: readonly number[], stored: number | null): number[] {
  const out = [...options];
  if (stored != null && !out.includes(stored)) {
    out.push(stored);
    out.sort((a, b) => a - b);
  }
  return out;
}

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
export function LocalAiLifecycle({
  configured,
  power,
}: {
  configured: boolean;
  /** The On battery readout from the status, or null while it isn't known — which counts as "not
   *  known to be a desktop", so the battery row stays usable. */
  power: PowerView | null;
}) {
  // null until the stored policy is read, like the battery row below: a picker showing "Leave it to
  // my server" before PM has looked would present a default as the user's choice, and would go on
  // presenting it if the read failed.
  const [policy, setPolicy] = useState<string | null>(null);
  const [idleMinutes, setIdleMinutes] = useState(5);
  // null until the stored value is read, and left null if the read fails: the row is disabled then,
  // rather than presenting "off" as though PM had said so.
  const [batteryIdle, setBatteryIdle] = useState<number | null>(null);
  // "restored": the write failed and the pickers show what is really stored. "unknown": the write
  // failed and so did reading it back, so the pickers show nothing.
  const [saveError, setSaveError] = useState<"restored" | "unknown" | null>(null);
  const [residency, setResidency] = useState<LocalGpuResidency | null>(null);
  const [releasing, setReleasing] = useState(false);
  const [freed, setFreed] = useState<number | null>(null);

  const refresh = useCallback(() => {
    void localGpuResidency()
      .then(setResidency)
      .catch(() => setResidency(null));
  }, []);

  // `afterFailedSave`: this read is checking what a failed write left behind. If it fails too, PM
  // knows neither what it tried to store nor what is stored, so both pickers go back to unknown —
  // leaving the unsaved choice on screen beside "this shows what PM has stored" would be a lie.
  const readStored = useCallback((afterFailedSave = false) => {
    void getLocalReleasePolicy()
      .then((s) => {
        setPolicy(s.policy);
        setIdleMinutes(s.idle_minutes);
        setBatteryIdle(s.battery_idle_minutes ?? null);
        if (afterFailedSave) setSaveError("restored");
      })
      .catch(() => {
        if (afterFailedSave) {
          setPolicy(null);
          setBatteryIdle(null);
          setSaveError("unknown");
        }
        /* otherwise the pickers simply stay unknown and disabled */
      });
  }, []);

  useEffect(() => {
    readStored();
    refresh();
  }, [readStored, refresh]);

  /** A write failed: show what PM really has stored, and say so. This used to be swallowed, which
   *  left the picker showing a choice that was never saved — the setting would quietly not apply. */
  function restore() {
    readStored(true);
  }

  function change(nextPolicy: string, nextMinutes: number) {
    setPolicy(nextPolicy);
    setIdleMinutes(nextMinutes);
    setFreed(null);
    void setLocalReleasePolicy(nextPolicy, nextMinutes).then(() => setSaveError(null), restore);
  }

  function changeBatteryIdle(nextMinutes: number) {
    setBatteryIdle(nextMinutes);
    void setLocalReleasePolicy(null, undefined, nextMinutes).then(
      () => setSaveError(null),
      restore,
    );
  }

  // A machine PM found no battery on. Only a positive reading counts: no power readout yet is not
  // evidence of a desktop.
  const desktop = power != null && !power.has_battery && power.source === "ac";
  const batteryOff = batteryIdle === null || !!residency?.no_unload_route || desktop;

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

  const chosen = policy == null ? null : (POLICIES.find((p) => p.value === policy) ?? POLICIES[0]);
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
                  value={policy ?? ""}
                  disabled={policy == null}
                  onChange={(e) => change(e.target.value, idleMinutes)}
                >
                  {/* Not a value: a placeholder until PM has read what is stored. */}
                  {policy == null && <option value="">—</option>}
                  {POLICIES.map((p) => (
                    <option key={p.value} value={p.value}>
                      {p.label}
                    </option>
                  ))}
                </Select>
              )}
            </SettingRow>
            {/* Unfolded: what the chosen option will actually do is a gating fact, not prose. */}
            {chosen && <p className="text-xs text-ink4">{chosen.when}</p>}
            {/* So "Leave it to my server — PM changes nothing" is never contradicted by the row
                below doing something on battery. */}
            {batteryIdle != null && batteryIdle > 0 && !desktop && (
              <p className="text-xs text-ink4">
                Except on battery: there, PM also hands the memory back after {minutes(batteryIdle)}{" "}
                without use, as set below.
              </p>
            )}

            {policy === "idle" && (
              <SettingRow label="Quiet period" helpId="settings-localai-lifecycle">
                {(a11y) => (
                  <Select
                    {...a11y}
                    value={String(idleMinutes)}
                    onChange={(e) => change(policy, Number(e.target.value))}
                  >
                    {QUIET_MINUTES.map((m) => (
                      <option key={m} value={m}>
                        {minutes(m)}
                      </option>
                    ))}
                  </Select>
                )}
              </SettingRow>
            )}

            {/* The On battery policy's other half (#432): whether PM stays local or moves to the
                cloud, a model left on the card keeps it drawing power. Enabled for keyless and
                Local only setups too — they are exactly who stays local on battery. */}
            <SettingRow
              label="On battery, hand the memory back"
              helpId="settings-localai-lifecycle"
            >
              {(a11y) => (
                <Select
                  {...a11y}
                  value={batteryIdle == null ? "" : String(batteryIdle)}
                  disabled={batteryOff}
                  onChange={(e) => changeBatteryIdle(Number(e.target.value))}
                >
                  {/* Not a value: a placeholder until PM has read what is stored. */}
                  {batteryIdle == null && <option value="">—</option>}
                  {withStored([0, ...QUIET_MINUTES], batteryIdle).map((m) => (
                    <option key={m} value={m}>
                      {m === 0 ? "As set above" : `After ${minutes(m)} without use`}
                    </option>
                  ))}
                </Select>
              )}
            </SettingRow>
            {desktop ? (
              <p className="text-xs text-ink4">
                PM didn't find a battery on this machine, so this never applies.
              </p>
            ) : batteryIdle == null ? null : batteryIdle > 0 ? (
              <p className="text-xs text-ink4">
                On battery, PM also hands the memory back once nothing has used the model for{" "}
                {minutes(batteryIdle)}, counting from no earlier than when you unplugged — so moving
                to the sofa keeps a model you were just using. The next message loads it again,
                which takes a few seconds. PM only releases models it loaded.
              </p>
            ) : (
              <p className="text-xs text-ink4">
                On battery, PM does whatever "Give the memory back" says. A graphics card holding a
                model keeps drawing power even while nothing is asking it anything.
              </p>
            )}
            {saveError && (
              <p className="text-xs text-st-due">
                {saveError === "restored"
                  ? "Couldn't save that. This shows what PM has stored."
                  : "Couldn't save that, and PM couldn't read back what is stored."}
              </p>
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
