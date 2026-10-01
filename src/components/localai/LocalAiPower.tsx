// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState, type ReactNode } from "react";

import { keepLocalOnBattery, setLocalPowerPolicy } from "../../lib/ipc";
import {
  blockedReason,
  consentLine,
  powerGate,
  powerOf,
  powerReadout,
  scopeNote,
  THRESHOLD_OPTIONS,
  type PowerGate,
} from "../../lib/powerRoute";
import type { LocalLlmStatus, PowerScope, PowerView } from "../../lib/types";
import { Button, RadioChoice, SectionInfo, SectionLabel, Select, SettingRow, Toggle } from "../ui";
import { PowerConsentAsk } from "../PowerConsentStrip";
import { sectionLabel } from "./sections";

/** The three What moves options, in the order shown — each with who it suits, so the choice is
 *  made against a reason rather than a label. */
const SCOPES: ReadonlyArray<{ value: PowerScope; label: string; detail: string }> = [
  {
    value: "both",
    label: "Chat and background work",
    detail:
      "Everything that runs on your local model moves to the cloud while the battery is low. Suits you if battery life matters more to you than keeping requests on this machine.",
  },
  {
    value: "chat",
    label: "Chat only",
    detail:
      "Your conversations move; background work stays where it is. Suits you if background work already runs on the cloud, or if you'd rather the filing, titles, summaries and learning PM does — from your files and your chats — never leave this machine.",
  },
  {
    value: "background",
    label: "Background work only",
    detail:
      "Filing, titles, summaries and learning move; the replies in your conversations stay on your local model. Titles, summaries and learning are written from your chats, so the text they read moves with them. Suits you if most of your battery goes on the work PM does in the background, and you'd rather your replies keep coming from your own model. Background work runs on its own schedule, often while you're not looking.",
  },
];

/** The offered levels, plus a stored one that isn't among them (in order), so the Select can show
 *  what is really stored rather than silently snapping to a neighbour. */
function thresholdOptions(current: number | null): number[] {
  const options: number[] = [...THRESHOLD_OPTIONS];
  if (current != null && !options.includes(current)) {
    options.push(current);
    options.sort((a, b) => a - b);
  }
  return options;
}

/**
 * "On battery" (#432) — whether PM moves work off the local model while a laptop's battery is low.
 *
 * Everything it says comes from the status's `power` object, which Rust computes with the same
 * functions routing uses; this section words it and never re-derives a level, a band or a time.
 * While the status hasn't arrived, every control is disabled and none shows a value — a 60% presented
 * as the user's choice before PM has read it would be a default dressed up as a fact.
 *
 * Writes are optimistic and the next snapshot is the truth: every command pings the status, so a
 * fresh snapshot lands within a moment and replaces whatever was assumed. A rejected write drops
 * the assumption at once and says why, rather than leaving a control showing a value PM never stored.
 */
export function LocalAiPower({
  status,
  configured,
  anyLocalRoleWithModel,
  onError,
}: {
  status: LocalLlmStatus | null;
  configured: boolean;
  anyLocalRoleWithModel: boolean;
  onError: (message: string | null) => void;
}) {
  const [optimistic, setOptimistic] = useState<{
    threshold?: number;
    roles?: PowerScope;
    keepLocal?: boolean;
  }>({});
  // Every fresh snapshot is the truth.
  useEffect(() => setOptimistic({}), [status]);

  const gate = powerGate(configured, anyLocalRoleWithModel, status);
  const power = status ? powerOf(status) : null;
  const ready = gate === "ready";

  const threshold = optimistic.threshold ?? power?.threshold ?? null;
  const roles = optimistic.roles ?? power?.roles ?? null;
  const keepLocal = optimistic.keepLocal ?? power?.keep_local ?? false;

  function write(patch: typeof optimistic, send: () => Promise<void>) {
    setOptimistic((o) => ({ ...o, ...patch }));
    void send().catch((e: unknown) => {
      setOptimistic({});
      onError(String(e));
    });
  }

  return (
    <div
      id="sec-localai-power"
      data-settings-section
      data-help="settings-localai-power"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel>{sectionLabel("sec-localai-power")}</SectionLabel>

      {/* Unfolded: why the controls below are dead is a gating hint, and the doctrine never folds
          those. The controls themselves stay on screen, disabled, so what this section would do is
          visible before it can do it. */}
      {gate !== "ready" && gate !== "loading" && <GateBlock gate={gate} power={power} />}
      {/* Said, so the disabled controls below — the switch can only look "off" — aren't read as
          what is set. */}
      {gate === "loading" && <p className="mt-2 text-xs text-ink4">Reading your power settings…</p>}

      {/* Unfolded: a status readout. */}
      {ready && status && power && (
        <>
          <p className="mt-2 text-xs text-ink4">
            {powerReadout(power, status.chat_loaded, status.background_loaded)}
          </p>
          {power.consent_needed && (
            // The same question as the app-wide strip, which the Settings overlay covers.
            <div className="mt-2 flex flex-wrap items-start justify-between gap-3 text-sm text-ink2">
              <PowerConsentAsk power={power} />
            </div>
          )}
        </>
      )}

      <div className="mt-3 space-y-3">
        <SettingRow label="Switch to the cloud at" helpId="settings-localai-power">
          {(a11y) => (
            <Select
              {...a11y}
              value={threshold == null ? "" : String(threshold)}
              disabled={!ready}
              onChange={(e) => {
                const t = Number(e.target.value);
                write({ threshold: t }, () => setLocalPowerPolicy({ threshold: t }));
              }}
            >
              {/* Not a value: a placeholder so nothing is presented as stored before PM knows. */}
              {threshold == null && <option value="">—</option>}
              {thresholdOptions(threshold).map((n) => (
                <option key={n} value={n}>
                  {n === 0 ? "Never" : `${n}%`}
                </option>
              ))}
            </Select>
          )}
        </SettingRow>
        {/* Unfolded: what the chosen level will actually do is a gating fact, not prose. Worded from
            the stored values, so it never describes a level the backend hasn't accepted. */}
        {power && (
          <p className="text-xs text-ink4">
            {power.threshold > 0
              ? `When the battery falls to ${power.threshold}%, PM sends new requests to your cloud model. It comes back to your local model about a minute after you plug in, or if the battery climbs back to ${power.return_at}%. A change has to last a minute before PM acts on it, so unplugging to move the laptop changes nothing.`
              : "PM never switches because of the battery. Your local model is used on battery and on mains alike."}
          </p>
        )}

        <fieldset className="mt-3">
          <legend className="text-xs font-medium text-ink3">What moves</legend>
          {SCOPES.map((s) => (
            <RadioChoice
              key={s.value}
              name="power-scope"
              value={s.value}
              current={roles}
              onSelect={(r) => write({ roles: r }, () => setLocalPowerPolicy({ roles: r }))}
              label={s.label}
              detail={s.detail}
              note={power ? scopeNote(s.value, power) : undefined}
              disabled={!ready}
            />
          ))}
        </fieldset>

        <SettingRow label="Keep using local until I quit PM" helpId="settings-localai-power">
          {(a11y) => (
            // Enabled on mains too, so it can be armed before unplugging and cleared after.
            // Before the status arrives a switch can only look "off"; the title says that isn't an
            // answer, so a default isn't read as the user's state.
            <Toggle
              {...a11y}
              checked={keepLocal}
              disabled={!ready}
              title={gate === "loading" ? "PM hasn't read this yet." : undefined}
              onChange={(next) => write({ keepLocal: next }, () => keepLocalOnBattery(next))}
            />
          )}
        </SettingRow>
        <p className="text-xs text-ink4">
          For when you're on battery and offline, or simply need your local model. It isn't saved:
          it lasts until you quit PM. With the tray icon on, closing the window doesn't quit PM, so
          it stays on.
        </p>

        {power && (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-xs text-ink4">{consentLine(power)}</p>
            {power.consent != null && (
              // Deliberately NOT gated on `ready`: taking back permission for data to leave the
              // machine must stay possible whatever else about the setup has changed since.
              <Button
                variant="tertiary"
                size="sm"
                onClick={() =>
                  void setLocalPowerPolicy({ consent: "none" }).catch((e: unknown) =>
                    onError(String(e)),
                  )
                }
              >
                Withdraw
              </Button>
            )}
          </div>
        )}
      </div>

      <SectionInfo title="What this changes, and what it doesn't">
        <p>
          Running a model on a dedicated graphics card takes a lot of power. Sending a request to
          the cloud instead reduces power use by not running inference on your GPU. It doesn't stop
          the card drawing power altogether: while your server holds the model, the card stays
          awake. "On battery, hand the memory back" under Holding the graphics card lets PM release
          it after a quiet spell. Stopping your model server saves the most — that's yours to do; PM
          never stops it.
        </p>
        <p>
          PM decides where a request goes at the moment it sends it. A reply that has started always
          finishes where it started, and a long job such as filing a big import changes course only
          between batches.
        </p>
        <p>
          Only roles set to Local, fall back to cloud ever move. Local only means the cloud is never
          used for that role, on battery or not.
        </p>
        <p>
          Requests sent to the cloud are billed to your OpenRouter key like any other, and count
          towards Usage & cost under your cloud model.
        </p>
        <p>Desktops, and machines where PM can't read a battery, count as always plugged in.</p>
      </SectionInfo>
    </div>
  );
}

/** Why the section can't act here, worded for the one reason that applies. */
function GateBlock({
  gate,
  power,
}: {
  gate: Exclude<PowerGate, "ready" | "loading">;
  power: PowerView | null;
}) {
  const title = (text: string) => <p className="mt-2 text-sm text-ink2">{text}</p>;
  const body = (text: ReactNode) => <p className="mt-1 text-xs text-ink4">{text}</p>;
  switch (gate) {
    case "cloud_only":
      return (
        <>
          {title("Switch to cloud on battery — unavailable")}
          {body(
            "This setting decides when PM uses a local model instead of the cloud. You're currently using OpenRouter for everything, so there's nothing to switch between. If you set up a local model in Local AI, this becomes available.",
          )}
        </>
      );
    case "no_key":
      return (
        <>
          {title("Switch to cloud on battery — unavailable")}
          {body(
            "Running a local model on a dedicated GPU uses significant power. This setting can send requests to a cloud provider while you're on battery instead to save power. There are currently no cloud providers set up, but don't worry, PM works without it!",
          )}
          {body("Linking an OpenRouter key would make this available.")}
        </>
      );
    case "key_unreadable":
      return (
        <>
          {title("Switch to cloud on battery — unavailable right now")}
          {body(
            "PM can't read your saved keys at the moment, so it can't tell whether a cloud provider is set up. It stays on your local model until it can.",
          )}
        </>
      );
    case "not_with_roles": {
      const reasons = power
        ? (["chat", "background"] as const).flatMap((role) => {
            const blocked = role === "chat" ? power.chat.blocked : power.background.blocked;
            return blocked ? [`${blockedReason(role, blocked, power)}.`] : [];
          })
        : [];
      return (
        <>
          {title("Switch to cloud on battery — not used with your roles")}
          {body(
            <>
              {reasons.join(" ")} To let PM switch while you're on battery, set a role to Local,
              fall back to cloud under Assign roles above.
            </>,
          )}
        </>
      );
    }
    case "no_battery":
      return (
        <p className="mt-2 text-xs text-ink4">
          No battery found, so PM treats this machine as always plugged in. Nothing here changes how
          it works.
        </p>
      );
  }
}
