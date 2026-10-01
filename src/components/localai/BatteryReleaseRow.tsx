// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Select, SettingRow } from "../ui";
import { minutes, QUIET_MINUTES, withStored } from "./quietPeriods";
import { sectionLabel } from "./sections";
import type { ReleaseSettings } from "./useReleaseSettings";

/**
 * "On battery, hand the memory back" (#432) — the half of the battery story that needs no cloud.
 *
 * Whether PM stays local or moves to the cloud, a model left on the card keeps it drawing power, so
 * this is enabled for keyless and Local only setups too: they are exactly who stays local on battery.
 * It lives in On battery, beside the other battery decisions, but it writes the same stored release
 * settings as Model memory — which is why it takes the tab's `release` rather than reading its own.
 */
export function BatteryReleaseRow({
  release,
  desktop,
  configured,
}: {
  /** The tab's release settings (`useReleaseSettings`), shared with Model memory. */
  release: ReleaseSettings;
  /** PM found no battery on this machine — a positive AC reading, never a missing one. */
  desktop: boolean;
  /** An endpoint is saved. With none there is no server to hand memory back from. */
  configured: boolean;
}) {
  const { batteryIdle, residency, saveError, changeBatteryIdle } = release;
  const noUnloadRoute = !!residency?.no_unload_route;
  const off = !configured || batteryIdle === null || noUnloadRoute || desktop;
  const lifecycle = sectionLabel("sec-localai-lifecycle");

  return (
    <>
      <SettingRow label="On battery, hand the memory back" helpId="settings-localai-power">
        {(a11y) => (
          <Select
            {...a11y}
            value={batteryIdle == null ? "" : String(batteryIdle)}
            disabled={off}
            onChange={(e) => changeBatteryIdle(Number(e.target.value))}
          >
            {/* Not a value: a placeholder until PM has read what is stored. */}
            {batteryIdle == null && <option value="">—</option>}
            {withStored([0, ...QUIET_MINUTES], batteryIdle).map((m) => (
              <option key={m} value={m}>
                {m === 0 ? `As set under ${lifecycle}` : `After ${minutes(m)} without use`}
              </option>
            ))}
          </Select>
        )}
      </SettingRow>
      {/* Unfolded, every branch: each is why the row is off or what the stored value will do — gating
          facts, not prose. */}
      {desktop ? (
        <p className="text-xs text-ink4">
          PM didn't find a battery on this machine, so this never applies.
        </p>
      ) : noUnloadRoute ? (
        <p className="text-xs text-ink4">
          Your server can't unload a model on request, so this can't do anything with it — see{" "}
          {lifecycle}.
        </p>
      ) : batteryIdle == null ? null : batteryIdle > 0 ? (
        <p className="text-xs text-ink4">
          On battery, PM also hands the memory back once nothing has used the model for{" "}
          {minutes(batteryIdle)}, counting from no earlier than when you unplugged — so moving to
          the sofa keeps a model you were just using. The next message loads it again, which takes a
          few seconds. PM only releases models it loaded.
        </p>
      ) : (
        <p className="text-xs text-ink4">
          On battery, PM does whatever "Give the memory back" under {lifecycle} says. A graphics
          card holding a model keeps drawing power even while nothing is asking it anything.
        </p>
      )}
      {saveError?.field === "battery" && (
        <p className="text-xs text-st-due">
          {saveError.kind === "restored"
            ? "Couldn't save that. This shows what PM has stored."
            : "Couldn't save that, and PM couldn't read back what is stored."}
        </p>
      )}
    </>
  );
}
