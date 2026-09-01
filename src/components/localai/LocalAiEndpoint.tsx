// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";

import {
  checkLocalLlmEndpoint,
  clearLocalLlmEndpoint,
  clearLocalLlmToken,
  probeLocalLlmPorts,
  setLocalLlmEndpoint,
  setLocalLlmToken,
} from "../../lib/ipc";
import type {
  DetectedEndpoint,
  EndpointCheck,
  LocalLlmConfig,
  LocalLlmStatus,
} from "../../lib/types";
import { runnerGuides } from "../../lib/workbenchGuide";
import {
  Button,
  Collapsible,
  ConfirmDialog,
  Input,
  SectionInfo,
  SectionLabel,
  useFieldA11y,
} from "../ui";

/**
 * "Connect an endpoint" — the address, the token, and what PM will and won't send there.
 *
 * It owns its own form state (the typed URL, the typed token, the last check, the in-flight flags)
 * because none of it means anything outside this section: a half-typed address is not a fact about
 * the tab. What it reports upward is only what other sections read — that the stored config
 * changed, and that something went wrong — plus `onEndpointChanged`, which exists because a test
 * result in the roles section proves a MODEL on a SERVER and the server has just moved.
 */
export function LocalAiEndpoint({
  config,
  status,
  configured,
  onReload,
  onError,
  onEndpointChanged,
}: {
  config: LocalLlmConfig | null;
  status: LocalLlmStatus | null;
  configured: boolean;
  /** Re-read the stored config (and the served-model list) into the tab. */
  onReload: () => Promise<void>;
  /** Show an error in the tab, or clear it with `null`. */
  onError: (message: string | null) => void;
  /** The stored endpoint or its token changed, so anything proved against the old one is stale. */
  onEndpointChanged: () => void;
}) {
  const [urlInput, setUrlInput] = useState("");
  const [detected, setDetected] = useState<DetectedEndpoint[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [check, setCheck] = useState<EndpointCheck | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmForgetToken, setConfirmForgetToken] = useState(false);
  // The endpoint form's two label/control pairs. Both labels sat above their Input naming nothing,
  // so the fields announced as the placeholder ("http://localhost:11434", "bearer token").
  const urlField = useFieldA11y();
  const tokenField = useFieldA11y();

  // Seed the field from the stored address, and only while the user has not typed one. A blind
  // assignment would overwrite what someone is halfway through typing every time the config
  // reloads, which it does after every save and every completed download.
  const storedUrl = config?.base_url ?? null;
  useEffect(() => {
    setUrlInput((u) => u || storedUrl || "");
  }, [storedUrl]);

  async function autodetect() {
    onError(null);
    try {
      const found = await probeLocalLlmPorts();
      setDetected(found);
      if (found.length === 1) setUrlInput(found[0].url);
    } catch (e) {
      onError(String(e));
    }
  }

  async function runCheck() {
    if (!urlInput.trim()) return;
    setChecking(true);
    setCheck(null);
    onError(null);
    try {
      setCheck(await checkLocalLlmEndpoint(urlInput.trim(), tokenInput.trim() || undefined));
    } catch (e) {
      onError(String(e));
    } finally {
      setChecking(false);
    }
  }

  async function saveEndpoint() {
    if (!urlInput.trim()) return;
    setSaving(true);
    onError(null);
    try {
      // URL FIRST, token second. `setLocalLlmEndpoint` refuses a public cleartext address, and
      // writing the token before that refusal left a bearer token in the OS keychain with no
      // endpoint to belong to — invisible, since `has_token` only renders once one is configured.
      const normalized = await setLocalLlmEndpoint(urlInput.trim());
      if (tokenInput.trim()) await setLocalLlmToken(tokenInput.trim());
      setUrlInput(normalized);
      setTokenInput("");
      setCheck(null);
      onEndpointChanged();
      await onReload();
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  }

  /** Forget just the bearer token, keeping the endpoint and both role assignments. Disconnect is
   *  the only other way to clear it, and that also wipes the base URL and both models. */
  async function forgetToken() {
    onError(null);
    try {
      await clearLocalLlmToken();
      // The token is part of what a pass proved: without it the same server may now refuse.
      onEndpointChanged();
      // Nothing else refreshes `config`, so without this the "(with a saved token)" line stays on
      // screen after the token is gone.
      await onReload();
    } catch (e) {
      onError(String(e));
    }
  }

  async function disconnect() {
    onError(null);
    try {
      await clearLocalLlmEndpoint();
      setCheck(null);
      setDetected(null);
      onEndpointChanged();
      await onReload();
    } catch (e) {
      onError(String(e));
    }
  }

  return (
    <div
      id="sec-localai-endpoint"
      data-settings-section
      data-help="settings-localai-endpoint"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel action={configured && <StatusChip status={status} />}>
        Connect an endpoint
      </SectionLabel>
      {/* Which runners PM supports, stated up front and in BOTH states — this is a gating fact
          (what you need to have installed), not prose to fold away. */}
      <p className="mt-1.5 text-xs text-ink4">
        Works with <span className="text-ink2">Ollama</span> (port 11434),{" "}
        <span className="text-ink2">LM Studio</span> (1234) and{" "}
        <span className="text-ink2">llama-server</span> (8080) — and any other server that speaks
        the OpenAI API, at whatever address you give it. PM connects to a server{" "}
        <span className="text-ink2">you</span> run; it never bundles, installs, or starts one.
      </p>

      {configured ? (
        <div className="mt-2">
          <p className="text-xs text-ink4">
            Connected to <span className="break-all text-ink2">{config?.base_url}</span>
            {config?.has_token ? " (with a saved token)" : ""}.
          </p>
          {/* Unfolded: the chip beside the section label says "Unreachable" or "Cooling down" and
              then stops, which is a dead end — the two states have different causes and different
              things to do about them, and neither was said anywhere. */}
          {status != null && !status.reachable && !status.in_cooldown && (
            <p className="mt-1 text-xs text-ink4">
              PM can't reach it at the moment. The usual cause is that the server isn't running —
              the guide below says, for each runner, whether it starts with your machine or you
              start it each session. PM keeps checking, so this clears on its own once it's back.
            </p>
          )}
          {status?.in_cooldown && (
            <p className="mt-1 text-xs text-ink4">
              PM is resting the connection after several failures in a row, so it isn't hammering a
              server that's struggling. It retries by itself — nothing to do unless it keeps coming
              back, in which case the server is the place to look.
            </p>
          )}
          <div className="mt-2 flex flex-wrap items-center gap-2">
            <Button variant="tertiary" onClick={() => void disconnect()}>
              Disconnect
            </Button>
            {config?.has_token && (
              <Button variant="tertiary" onClick={() => setConfirmForgetToken(true)}>
                Forget token
              </Button>
            )}
          </div>
          <ConfirmDialog
            open={confirmForgetToken}
            title="Forget the saved token?"
            danger
            confirmLabel="Forget it"
            onConfirm={() => {
              setConfirmForgetToken(false);
              void forgetToken();
            }}
            onClose={() => setConfirmForgetToken(false)}
          >
            PM deletes the bearer token for this endpoint from your keychain. The address and your
            role assignments stay as they are. If the server requires a token, PM won't be able to
            reach it until you connect again with a new one.
          </ConfirmDialog>
        </div>
      ) : (
        <div className="mt-2 space-y-3">
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="secondary" onClick={() => void autodetect()}>
              Auto-detect a local server
            </Button>
            <span className="text-xs text-ink4">
              Looks for Ollama, LM Studio, and llama-server on this machine.
            </span>
          </div>
          {detected && (
            <div className="text-xs">
              {detected.length === 0 ? (
                <p className="text-ink4">
                  Nothing answered on port 11434, 1234 or 8080. If you've already installed one,
                  it's most likely not running — check the guide below for whether yours starts on
                  its own. Otherwise install one and auto-detect again. You can also type an address
                  yourself, if your server is on a different port or another machine.
                </p>
              ) : (
                <ul className="space-y-1">
                  {detected.map((d) => (
                    <li key={d.url}>
                      <button
                        type="button"
                        onClick={() => setUrlInput(d.url)}
                        className="text-accent-text underline hover:brightness-110"
                      >
                        {d.label} — {d.url}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
          <div>
            <label {...urlField.labelProps} className="block text-sm font-medium text-ink2">
              Endpoint URL
            </label>
            <Input
              {...urlField.controlProps}
              value={urlInput}
              onChange={(e) => {
                setUrlInput(e.target.value);
                setCheck(null); // a prior check is stale once the URL changes
              }}
              placeholder="http://localhost:11434"
              className="mt-1"
            />
          </div>
          <div>
            <label {...tokenField.labelProps} className="block text-sm font-medium text-ink2">
              Token <span className="text-ink4">(optional — only for a remote endpoint)</span>
            </label>
            <Input
              {...tokenField.controlProps}
              type="password"
              autoComplete="off"
              value={tokenInput}
              onChange={(e) => {
                setTokenInput(e.target.value);
                setCheck(null);
              }}
              placeholder="bearer token"
              className="mt-1"
            />
          </div>
          {check && <EndpointCheckResult check={check} />}
          <div className="flex flex-wrap gap-2">
            <Button
              variant="secondary"
              onClick={() => void runCheck()}
              disabled={checking || !urlInput.trim()}
            >
              {checking ? "Checking…" : "Check"}
            </Button>
            <Button
              variant="primary"
              onClick={() => void saveEndpoint()}
              disabled={saving || !urlInput.trim()}
            >
              {saving ? "Connecting…" : "Connect"}
            </Button>
          </div>
        </div>
      )}

      {/* Outside the `configured` branch on purpose. It used to live inside the not-yet-connected
          half, so the moment you connected anything the comparison vanished — and "was one of the
          others a better choice for me?" is a question you mostly ask AFTER trying one. */}
      <Collapsible
        title={configured ? "Compare the three local servers" : "Don't have a local server yet?"}
        defaultOpen={false}
      >
        <RunnerInstall />
      </Collapsible>

      <SectionInfo title="What leaves your device">
        <p>
          A server on <span className="text-ink2">this machine</span> (localhost / 127.0.0.1) keeps
          everything on your device — nothing leaves it, which is even stronger than the
          zero-retention promise PM makes to cloud providers.
        </p>
        <p>
          A <span className="text-ink2">remote</span> endpoint (another computer, your LAN, or a
          Tailscale address) means your chats are sent to that server — PM can't vouch for what it
          does with them. PM refuses to send a token and your chats in the clear to a public
          address, and warns when a server is exposed on your network.
        </p>
        <p>
          PM never downloads model weights itself. Your runner (Ollama / LM Studio) fetches them
          from wherever it's configured to — the Ollama registry or Hugging Face.
        </p>
      </SectionInfo>
    </div>
  );
}

function StatusChip({ status }: { status: LocalLlmStatus | null }) {
  // The cooldown seconds arrive up to 30s stale (the poll cadence) and then sat FROZEN until the
  // next poll — a countdown that doesn't count. Anchor it to when this status landed and tick it
  // down locally; each poll re-anchors. At zero the chip says what is actually happening (the next
  // call retries) instead of holding a dead number.
  const receivedAt = useRef(Date.now());
  const lastStatus = useRef<LocalLlmStatus | null>(status);
  if (lastStatus.current !== status) {
    lastStatus.current = status;
    receivedAt.current = Date.now();
  }
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!status?.in_cooldown) return;
    const id = setInterval(() => setTick((n) => n + 1), 1000);
    return () => clearInterval(id);
  }, [status]);
  let label = "Checking…";
  let token = "--ink4";
  if (status) {
    if (status.in_cooldown) {
      const elapsed = Math.floor((Date.now() - receivedAt.current) / 1000);
      const remaining = Math.max(0, (status.cooldown_remaining_s ?? 0) - elapsed);
      label = remaining > 0 ? `Cooling down (${remaining}s)` : "Ready to retry";
      token = "--st-look";
    } else if (status.reachable) {
      label = "Connected";
      token = "--st-quick";
    } else {
      label = "Unreachable";
      token = "--st-due";
    }
  }
  return (
    <span
      className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium"
      style={{
        color: `var(${token})`,
        background: `color-mix(in oklab, var(${token}) 15%, transparent)`,
      }}
    >
      {label}
    </span>
  );
}

function EndpointCheckResult({ check }: { check: EndpointCheck }) {
  const bad = check.scheme_verdict === "refused_public_cleartext" || !check.reachable;
  // A server that answers but serves nothing is not a pass, and the fall-through token is the
  // green one — which reads as "you're set" at the exact moment there is still a download to do.
  // Unreachable before #790, so nothing here had to account for it.
  const empty = check.reachable && check.models.length === 0;
  const warn =
    empty ||
    check.posture !== "loopback" ||
    check.exposed_on_network ||
    check.scheme_verdict === "warn_unencrypted";
  const token = bad ? "--st-due" : warn ? "--st-look" : "--st-quick";
  return (
    <div
      className="rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
      style={{
        borderColor: `color-mix(in oklab, var(${token}) 45%, transparent)`,
        background: `color-mix(in oklab, var(${token}) 12%, transparent)`,
        color: "var(--ink2)",
      }}
    >
      <p className="font-medium" style={{ color: `var(${token})` }}>
        {check.reachable ? `Reachable · ${check.models.length} model(s)` : "Not reachable"}
        {check.posture !== "loopback" ? ` · ${check.posture}` : ""}
      </p>
      {empty && (
        <p className="mt-1">
          It's running, but there are no models in it yet — so there is nothing for PM to send work
          to. Download one into it, then check again.
        </p>
      )}
      {check.posture !== "loopback" && check.scheme_verdict !== "refused_public_cleartext" && (
        <p className="mt-1">
          This is a remote server — your chats will be sent to it. PM can't vouch for what it does
          with them.
        </p>
      )}
      {check.message && <p className="mt-1">{check.message}</p>}
    </div>
  );
}

/** All three runners, with the choice explained before the instructions.
 *
 *  This used to be the Ollama guide plus one sentence conceding the other two exist, which left a
 *  user who had never installed any of them with no way to tell them apart — and PM auto-detects
 *  all three, so "which one?" is a question PM creates and ought to answer. */
function RunnerInstall() {
  const guides = runnerGuides();
  return (
    <div className="mt-1 text-xs text-ink4">
      <p>
        PM works with any of these three. It doesn't bundle or install one — you pick and install it
        yourself, and it fetches the model weights, not PM. They differ in how much of an app comes
        with them and how you get models; all three end up serving the same models to PM.
      </p>
      <div className="mt-3 space-y-3">
        {guides.map((g) => (
          <div key={g.name} className="rounded-[var(--radius-sm)] border border-border p-2.5">
            <div className="flex flex-wrap items-baseline gap-x-2">
              <span className="text-sm text-ink2">{g.name}</span>
              <span className="font-mono text-[0.625rem] text-ink4">port {g.port}</span>
            </div>
            <p className="mt-0.5">{g.summary}</p>
            <p className="mt-1.5 text-ink3">{g.bestFor}</p>
            <p className="mt-1">
              <span className="text-ink3">Models:</span> {g.models}
            </p>
            {/* Unfolded, never a caret: a hardware exclusion and "does this stay running?" are
                gating facts, and the settings doctrine folds prose but not those. Lifecycle sits
                immediately before the steps because it is what decides whether the steps are a
                one-time setup or something you redo every session. */}
            {g.caveat && <p className="mt-1 text-ink3">Worth knowing: {g.caveat}</p>}
            <p className="mt-1 text-ink3">
              <span className="text-ink3">Staying running:</span> {g.lifecycle}
            </p>
            <ol className="ml-4 mt-1.5 list-decimal space-y-1">
              {g.steps.map((s, i) => (
                <li key={i}>{s}</li>
              ))}
            </ol>
          </div>
        ))}
      </div>
    </div>
  );
}
