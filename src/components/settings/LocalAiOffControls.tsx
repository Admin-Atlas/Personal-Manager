// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";

import { shortModel } from "../../lib/format";
import { clearLocalLlmEndpoint, setLocalLlmRouting } from "../../lib/ipc";
import { refreshLocalAiSwitch, setLocalAiEnabled } from "../../lib/localAiSwitch";
import type { LocalAiSwitch } from "../../lib/types";
import { Button, Callout, ConfirmDialog, SettingRow } from "../ui";

// What AI & Models shows while Local AI is switched off (D7). The Local AI tab, and with it the
// switch and every routing control, is gone then, so the two things someone needs from it live
// here for as long as it is off, and only then ("one control, one home": on, they are the tab's):
//
// - `LocalAiPickPrompt`: Bobby's "tell them to pick a model". A role that was routed to a local
//   model has nothing to answer it, and is offered the cloud model, which is the only kind there is
//   to pick while off. It is the one control besides Assign roles that writes routing.
// - `LocalAiOffRow`: the way back on, and "Forget server and token", which switching off keeps
//   (so switching back on is one click) and which must still be reachable.

/** The words every surface uses for a role with nothing to answer it, Bobby's verbatim. Mirrors
 *  `local_ai_switch::LOCAL_AI_OFF_MESSAGE`, which the backend's own refusals carry; a test reads
 *  the Rust source to keep the two the same. */
export const LOCAL_AI_OFF_MESSAGE =
  "Local AI is off. Pick a model in AI & Models to keep using AI.";

const ROLES = [
  { role: "chat", label: "Chat", what: "Nothing answers your chats." },
  { role: "background", label: "Background", what: "Nothing does PM's background work." },
] as const;

export function LocalAiPickPrompt({
  sw,
  chatModel,
  backgroundModel,
  onError,
}: {
  sw: LocalAiSwitch;
  /** The first cloud model of each role, as the editors above list them, so the button names what
   *  will answer. `null` while the lists load or when a role's list is empty. */
  chatModel: string | null;
  backgroundModel: string | null;
  onError: (message: string | null) => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const needs = ROLES.filter(({ role }) =>
    role === "chat" ? sw.chat_needs_model : sw.background_needs_model,
  );
  if (sw.on || needs.length === 0) return null;

  async function pickCloud(role: "chat" | "background") {
    onError(null);
    setBusy(role);
    try {
      await setLocalLlmRouting(role, "cloud");
      await refreshLocalAiSwitch();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <Callout tone="warning" body="ink" live data-help="settings-localai-pick">
      <p className="font-medium">{LOCAL_AI_OFF_MESSAGE}</p>
      <ul className="mt-2 space-y-2">
        {needs.map(({ role, label, what }) => {
          const model = role === "chat" ? chatModel : backgroundModel;
          return (
            <li key={role} className="flex flex-wrap items-center justify-between gap-2">
              <span className="min-w-0">
                <span className="font-medium">{label}.</span> {what}
              </span>
              <Button
                size="sm"
                variant="secondary"
                disabled={busy !== null}
                onClick={() => void pickCloud(role)}
              >
                {busy === role
                  ? "Saving…"
                  : model
                    ? `Use ${shortModel(model)}`
                    : `Use the cloud ${label.toLowerCase()} model`}
              </Button>
            </li>
          );
        })}
      </ul>
      <p className="mt-2 text-xs text-ink4">
        The cloud models are the ones listed below. To use your own model instead, turn Local AI
        back on.
      </p>
    </Callout>
  );
}

export function LocalAiOffRow({
  sw,
  onTurnedOn,
  onError,
}: {
  sw: LocalAiSwitch;
  /** Switched on: the Local AI tab is back, and this takes the reader to it. */
  onTurnedOn?: () => void;
  onError: (message: string | null) => void;
}) {
  const [turningOn, setTurningOn] = useState(false);
  const [confirmForget, setConfirmForget] = useState(false);
  const [forgetting, setForgetting] = useState(false);

  async function turnOn() {
    onError(null);
    setTurningOn(true);
    try {
      await setLocalAiEnabled(true);
      onTurnedOn?.();
    } catch (e) {
      onError(String(e));
    } finally {
      setTurningOn(false);
    }
  }

  async function forget() {
    onError(null);
    setForgetting(true);
    try {
      await clearLocalLlmEndpoint();
      await refreshLocalAiSwitch();
      setConfirmForget(false);
    } catch (e) {
      onError(String(e));
      setConfirmForget(false);
    } finally {
      setForgetting(false);
    }
  }

  return (
    <>
      <SettingRow
        label="Local AI: Off"
        description={
          sw.server_saved
            ? "Run AI on your own computer with Ollama, LM Studio or llama-server. PM kept your server's address and token, so turning it on reconnects at once. You'll choose your models again."
            : "Run AI on your own computer with Ollama, LM Studio or llama-server. Turning it on brings back the Local AI tab, where you connect your server."
        }
        helpId="settings-localai-switch-row"
      >
        {/* Named by its own words, not the row's label: "Local AI: Off, button" would announce the
            state and not the action. The row's description still describes it. */}
        {(a11y) => (
          <Button
            aria-describedby={a11y["aria-describedby"]}
            size="sm"
            variant="secondary"
            disabled={turningOn}
            onClick={() => void turnOn()}
          >
            {turningOn ? "Turning on…" : "Turn on"}
          </Button>
        )}
      </SettingRow>
      {sw.server_saved && (
        <div className="mt-2">
          <Button size="sm" variant="tertiary" onClick={() => setConfirmForget(true)}>
            Forget server and token
          </Button>
        </div>
      )}
      <ConfirmDialog
        open={confirmForget}
        title="Forget your model server?"
        confirmLabel="Forget"
        danger
        busy={forgetting}
        onConfirm={() => void forget()}
        onClose={() => setConfirmForget(false)}
      >
        <p>
          PM forgets your server&apos;s address and its access token. Your models stay on your
          server. To use Local AI again, you&apos;ll connect your server afresh.
        </p>
      </ConfirmDialog>
    </>
  );
}
