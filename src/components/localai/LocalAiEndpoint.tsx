// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";

import {
  checkLocalLlmEndpoint,
  clearLocalLlmEndpoint,
  clearLocalLlmToken,
  setLocalLlmEndpoint,
  setLocalLlmToken,
} from "../../lib/ipc";
import type {
  DetectedEndpoint,
  EndpointCheck,
  LocalLlmConfig,
  LocalLlmStatus,
} from "../../lib/types";
import { runnerGuides, tuningFor } from "../../lib/workbenchGuide";
import { withCode } from "../withCode";
import { TokenChip } from "./fitDisplay";
import { TUNING_ID, TUNING_TITLE } from "./locate";
import { runnerOf } from "./readiness";
import { RunnerGuideCard } from "./RunnerGuideCard";
import { SectionLink } from "./SectionLink";
import { sectionHelp, sectionLabel } from "./sections";
import {
  Button,
  Callout,
  Collapsible,
  ConfirmDialog,
  Input,
  SectionInfo,
  SectionLabel,
  useFieldA11y,
} from "../ui";

/**
 * "Model server" — the address, the token, and what PM will and won't send there.
 *
 * It owns its own form state (the typed URL, the typed token, the last check, the in-flight flags)
 * because none of it means anything outside this section: a half-typed address is not a fact about
 * the tab. What it reports upward is only what other sections read — that the stored config
 * changed, and that something went wrong — plus `onEndpointChanged`, which exists because a test
 * result in the roles section proves a MODEL on a SERVER and the server has just moved.
 *
 * What the port probe found is the tab's (`useServerDetect`), so this list and the start card's step
 * 1 are one answer. While step 1 is showing the install guide, this section points there rather than
 * keeping a second copy; whenever it isn't — a server was found, or one is connected — the
 * comparison here is the guide, so there is always one on the page.
 */
export function LocalAiEndpoint({
  config,
  status,
  configured,
  onReload,
  onError,
  onEndpointChanged,
  error,
  tuningOpen,
  onTuningOpenChange,
  detected,
  onDetect,
  guideInStart,
  pickContext,
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
  /** Something in this section went wrong, said here rather than at the top of the tab. */
  error?: string | null;
  /** The "Settings PM's numbers assume" fold, held by the tab so a pointer elsewhere can open it.
   *  Omitted, the fold keeps its own state. */
  tuningOpen?: boolean;
  onTuningOpenChange?: (open: boolean) => void;
  /** What the tab's port probe found while nothing is connected, or null before it has answered. */
  detected: DetectedEndpoint[] | null;
  /** Look again now, resolving to what was found. */
  onDetect: () => Promise<DetectedEndpoint[]>;
  /** The start card's step 1 is showing the install guide right now. */
  guideInStart: boolean;
  /** The context PM sized its pick for, so the steps here set that number — or null with no pick. */
  pickContext: number | null;
}) {
  const [urlInput, setUrlInput] = useState("");
  // Whether this section's own Auto-detect has been pressed: the probe also runs by itself, and
  // "nothing answered" is the answer to a question — said once someone has asked it here, rather
  // than as a second copy of what the start card's first step already says.
  const [asked, setAsked] = useState(false);
  const [checking, setChecking] = useState(false);
  const [check, setCheck] = useState<EndpointCheck | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmForgetToken, setConfirmForgetToken] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
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

  // The two settings PM sized its numbers for, worded for the server this is — or nothing, for a
  // server PM can't name by its port.
  const runner = runnerOf(storedUrl);
  const tuning = tuningFor(runner, undefined, pickContext);
  // The comparison is the guide whenever step 1 isn't: once a server is found, step 1 is a Connect
  // button, and someone with only LM Studio running who wants Ollama had no install steps anywhere.
  const compare = configured || (detected?.length ?? 0) > 0;

  async function autodetect() {
    onError(null);
    setAsked(true);
    const found = await onDetect();
    if (found.length === 1) setUrlInput(found[0].url);
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
      setAsked(false);
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
      data-help={sectionHelp("sec-localai-endpoint")}
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel action={configured && <StatusChip status={status} />}>
        {sectionLabel("sec-localai-endpoint")}
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
      {error && <Callout className="mt-2">{error}</Callout>}

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
              the comparison below says, for each runner, whether it starts with your machine or you
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
            {/* Asks first: it forgets more than the address — the token and both jobs' models go
                with it, and what each job does afterwards depends on its routing. */}
            <Button variant="tertiary" onClick={() => setConfirmDisconnect(true)}>
              Disconnect…
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
          <ConfirmDialog
            open={confirmDisconnect}
            title="Disconnect from your model server?"
            danger
            confirmLabel="Disconnect"
            onConfirm={() => {
              setConfirmDisconnect(false);
              void disconnect();
            }}
            onClose={() => setConfirmDisconnect(false)}
          >
            PM forgets this server's address, its saved token, and the models you gave chat and
            background work. Your server and the models in it aren't touched. Afterwards, a job set
            to Local only has nothing to answer with until you connect again, and one set to Local,
            fall back to cloud uses your cloud model if you have one. To change the address or
            token, disconnect and connect again.
          </ConfirmDialog>
        </div>
      ) : (
        <div className="mt-2 space-y-3">
          {detected && detected.length > 0 && (
            <div className="text-xs">
              <p className="text-ink4">Found on this computer:</p>
              <ul className="mt-1 space-y-1">
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
            </div>
          )}
          {/* Only while it's true: once a server is found, step 1 is a Connect button and the guide
              is the comparison below this form. */}
          {guideInStart && (
            <p className="text-xs text-ink4">
              Don't have one yet? Step 1 under <SectionLink to="sec-localai-start" /> has the
              install steps for this computer.
            </p>
          )}
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="secondary" onClick={() => void autodetect()}>
              Auto-detect a local server
            </Button>
            <span className="text-xs text-ink4">
              Looks for Ollama, LM Studio, and llama-server on this machine.
            </span>
          </div>
          {asked && detected && detected.length === 0 && (
            <p className="text-xs text-ink4">
              Nothing answered on port 11434, 1234 or 8080. If you've already installed one, it's
              most likely not running — step 1 under <SectionLink to="sec-localai-start" /> says,
              for each server, whether it starts on its own. You can also type an address yourself,
              if your server is on a different port or another machine.
            </p>
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
            {/* Secondary: the tab's one primary is the step it is on, and connecting from this form
                is the by-hand route to the same write. */}
            <Button
              variant="secondary"
              onClick={() => void saveEndpoint()}
              disabled={saving || !urlInput.trim()}
            >
              {saving ? "Connecting…" : "Connect"}
            </Button>
          </div>
        </div>
      )}

      {/* The settings PM's numbers assume, for the server this is. PM sizes for a longer context than
          most servers start with, and on most graphics-card setups for a compressed cache — a model
          PM says fits only fits that way once the server is set the same way, and nothing said so
          before this. Folded (it is instructions), and held by the tab so a pointer can open it. */}
      {configured && runner && tuning && (
        <div id={TUNING_ID} className="mt-3">
          <Collapsible
            title={TUNING_TITLE}
            defaultOpen={false}
            open={tuningOpen}
            onOpenChange={onTuningOpenChange}
          >
            <div className="mt-1 text-xs text-ink4">
              <p>
                PM sizes models for a longer context than most servers start with, and on most
                graphics-card setups for a compressed (q8_0) cache. These are the {runner} settings
                for both:
              </p>
              <ul className="mt-1.5 list-disc space-y-1 pl-4">
                <li>{withCode(tuning.context)}</li>
                <li>{withCode(tuning.cache)}</li>
              </ul>
            </div>
          </Collapsible>
        </div>
      )}

      {/* Whenever the start card's first step isn't the guide: once connected ("was one of the
          others a better choice for me?" is a question you mostly ask AFTER trying one), and once a
          server is found but not yet connected. Before that, step 1 is the guide — one guide on the
          page, not two that could disagree. */}
      {compare && (
        <Collapsible title="Compare the three local servers" defaultOpen={false}>
          <RunnerInstall connected={configured} pickContext={pickContext} />
        </Collapsible>
      )}

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
  return <TokenChip token={token}>{label}</TokenChip>;
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
          to. Connect it anyway: with Ollama, PM can then download one into it for you.
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
 *  all three, so "which one?" is a question PM creates and ought to answer.
 *
 *  `connected`: each guide ends with the step for someone who has to disconnect first, since PM
 *  doesn't look for another server while one is connected. */
function RunnerInstall({
  connected,
  pickContext,
}: {
  connected: boolean;
  pickContext: number | null;
}) {
  const guides = runnerGuides(undefined, pickContext);
  return (
    <div className="mt-1 text-xs text-ink4">
      <p>
        PM works with any of these three. It doesn't bundle or install one — you pick and install it
        yourself, and it fetches the model weights, not PM. They differ in how much of an app comes
        with them and how you get models; all three end up serving the same models to PM.
      </p>
      <div className="mt-3 space-y-3">
        {guides.map((g) => (
          <RunnerGuideCard key={g.name} guide={g} connected={connected} />
        ))}
      </div>
    </div>
  );
}
