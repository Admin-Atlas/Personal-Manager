// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";

import {
  checkLocalLlmEndpoint,
  clearLocalLlmEndpoint,
  clearLocalLlmToken,
  dismissLocalBetterFit,
  getLocalLlmConfig,
  listLocalLlmModels,
  localBetterFitNotice,
  localHardwareScan,
  localLlmStatus,
  localModelRecommendations,
  probeLocalLlmPorts,
  acceptLocalModelTerms,
  pullLocalModel,
  setLocalLlmEndpoint,
  setLocalLlmRoleModel,
  setLocalLlmRouting,
  setLocalLlmToken,
  setLocalModelRescanCadence,
  setLocalModelScanDir,
} from "../../lib/ipc";
import type {
  DetectedEndpoint,
  EndpointCheck,
  LocalBetterFit,
  LocalDiskSource,
  LocalFitResult,
  LocalFitVerdict,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalOnDiskModel,
  LocalRecommendation,
  LocalRecommendations,
  LocalRescanCadence,
  LocalServedModel,
  PullProgress,
} from "../../lib/types";
import { ollamaGuide } from "../../lib/workbenchGuide";
import { Button, Collapsible, ConfirmDialog, Input, SectionInfo, Select } from "../ui";

/** The Local AI tab (#296): read this machine's hardware, size a curated model catalog against it,
 *  and turn on the local-endpoint provider (#297) — connect a local server, assign it to the chat /
 *  background roles, with cloud fallback. Self-contained and immediate-persist; errors surface inline.
 *  Frontend-only over existing backend commands, plus the one streaming Ollama pull. */
export function LocalAiSettings({ onBetterFitChange }: { onBetterFitChange?: () => void } = {}) {
  const [recs, setRecs] = useState<LocalRecommendations | null>(null);
  const [betterFit, setBetterFit] = useState<LocalBetterFit | null>(null);
  const [loading, setLoading] = useState(true);
  const [rescanning, setRescanning] = useState(false);
  const [config, setConfig] = useState<LocalLlmConfig | null>(null);
  const [status, setStatus] = useState<LocalLlmStatus | null>(null);
  const [served, setServed] = useState<LocalServedModel[]>([]);
  const [error, setError] = useState<string | null>(null);

  // Endpoint form.
  const [urlInput, setUrlInput] = useState("");
  const [detected, setDetected] = useState<DetectedEndpoint[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [check, setCheck] = useState<EndpointCheck | null>(null);
  const [tokenInput, setTokenInput] = useState("");
  const [saving, setSaving] = useState(false);
  const [confirmForgetToken, setConfirmForgetToken] = useState(false);

  // Model pull (Ollama only).
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProg, setPullProg] = useState<PullProgress | null>(null);
  /** The model whose licence terms are being shown, or null when no dialog is open. */
  const [termsFor, setTermsFor] = useState<LocalRecommendation | null>(null);

  const configured = !!config?.base_url;
  // Whether the connected endpoint is an Ollama server (the only runner with a one-click pull API).
  // Heuristic: Ollama's default port. A non-Ollama endpoint gets a copy-paste command instead.
  const isOllama = !!config?.base_url?.includes(":11434");

  async function reloadConfig() {
    const cfg = await getLocalLlmConfig();
    setConfig(cfg);
    if (cfg.base_url) {
      setUrlInput((u) => u || cfg.base_url || "");
      listLocalLlmModels()
        .then(setServed)
        .catch(() => setServed([]));
    } else {
      setServed([]);
    }
  }

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      // Load config and recommendations INDEPENDENTLY: config drives the endpoint/roles UI, while
      // recommendations are a best-effort readout — a hardware-scan failure must not blank a
      // genuinely-configured endpoint (so you can still see its status, disconnect, or reassign).
      try {
        const cfg = await getLocalLlmConfig();
        if (cancelled) return;
        setConfig(cfg);
        if (cfg.base_url) {
          setUrlInput(cfg.base_url);
          listLocalLlmModels()
            .then((m) => !cancelled && setServed(m))
            .catch(() => {});
        }
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
      try {
        const r = await localModelRecommendations();
        if (!cancelled) setRecs(r);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
      // The better-fit suggestion (#437) is a separate, best-effort read: it must never blank the
      // rest of the tab, and there being nothing to suggest is the common case.
      try {
        const b = await localBetterFitNotice();
        if (!cancelled) setBetterFit(b);
      } catch {
        /* a suggestion is a nicety — stay quiet if it can't be computed */
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Poll the live status while an endpoint is configured (the backend debounces the actual probe to
  // once / 30s, so this can't hammer the user's server).
  useEffect(() => {
    if (!configured) {
      setStatus(null);
      return;
    }
    let cancelled = false;
    const tick = () =>
      localLlmStatus()
        .then((s) => !cancelled && setStatus(s))
        .catch(() => {});
    void tick();
    const id = setInterval(tick, 30000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [configured]);

  /** Acknowledge the better-fit suggestion: it stays quiet until the cadence says to look again.
   *  Clears the dot here and, via the callback, in the sidebar and the settings nav. */
  async function dismissBetterFit() {
    setBetterFit(null);
    try {
      await dismissLocalBetterFit();
    } catch (e) {
      setError(String(e));
    }
    onBetterFitChange?.();
  }

  /** Change how often PM re-checks. `manual` turns the notice off without hiding the way back. */
  async function changeCadence(cadence: string) {
    setRecs((r) => (r ? { ...r, cadence } : r));
    try {
      await setLocalModelRescanCadence(cadence as LocalRescanCadence);
      if (cadence === "manual") setBetterFit(null);
    } catch (e) {
      setError(String(e));
    }
    onBetterFitChange?.();
  }

  /** Point the on-disk crawl at an extra folder (or change the one it uses), then reload. */
  async function pickScanFolder() {
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked !== "string") return;
    await applyScanDir(picked);
  }

  async function clearScanFolder() {
    await applyScanDir(null);
  }

  async function applyScanDir(dir: string | null) {
    setError(null);
    try {
      await setLocalModelScanDir(dir);
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError(String(e));
    }
  }

  async function rescan() {
    setRescanning(true);
    setError(null);
    try {
      await localHardwareScan(true);
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError(String(e));
    } finally {
      setRescanning(false);
    }
  }

  async function autodetect() {
    setError(null);
    try {
      const found = await probeLocalLlmPorts();
      setDetected(found);
      if (found.length === 1) setUrlInput(found[0].url);
    } catch (e) {
      setError(String(e));
    }
  }

  async function runCheck() {
    if (!urlInput.trim()) return;
    setChecking(true);
    setCheck(null);
    setError(null);
    try {
      setCheck(await checkLocalLlmEndpoint(urlInput.trim(), tokenInput.trim() || undefined));
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  }

  async function saveEndpoint() {
    if (!urlInput.trim()) return;
    setSaving(true);
    setError(null);
    try {
      // URL FIRST, token second. `setLocalLlmEndpoint` refuses a public cleartext address, and
      // writing the token before that refusal left a bearer token in the OS keychain with no
      // endpoint to belong to — invisible, since `has_token` only renders once one is configured.
      const normalized = await setLocalLlmEndpoint(urlInput.trim());
      if (tokenInput.trim()) await setLocalLlmToken(tokenInput.trim());
      setUrlInput(normalized);
      setTokenInput("");
      setCheck(null);
      await reloadConfig();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  /** Forget just the bearer token, keeping the endpoint and both role assignments. Disconnect is
   *  the only other way to clear it, and that also wipes the base URL and both models. */
  async function forgetToken() {
    setError(null);
    try {
      await clearLocalLlmToken();
      // Nothing else refreshes `config`, so without this the "(with a saved token)" line stays on
      // screen after the token is gone.
      await reloadConfig();
    } catch (e) {
      setError(String(e));
    }
  }

  async function disconnect() {
    setError(null);
    try {
      await clearLocalLlmEndpoint();
      setCheck(null);
      setDetected(null);
      await reloadConfig();
    } catch (e) {
      setError(String(e));
    }
  }

  function changeRoleModel(role: "chat" | "background", model: string) {
    setConfig((c) => (c ? { ...c, [`${role}_model`]: model || null } : c));
    void setLocalLlmRoleModel(role, model).catch((e) => setError(String(e)));
  }

  function changeRouting(role: "chat" | "background", pref: string) {
    setConfig((c) => (c ? { ...c, [`${role}_routing`]: pref } : c));
    void setLocalLlmRouting(role, pref as "cloud" | "local" | "local-then-cloud").catch((e) =>
      setError(String(e)),
    );
  }

  /** Download, once the terms behind this model have been shown and accepted (if they need to be).
   *
   *  Restricted-licence models — Gemma, Llama, the largest Qwen — carry publisher terms rather than
   *  an open-source licence, so PM shows them first. Acceptance is remembered per LICENCE, so
   *  reading the Gemma Terms once covers every Gemma. Open-licence models are never interrupted.
   *
   *  This is disclosure, not enforcement: the download is the user's own Ollama fetching the weights
   *  from the publisher, and they could run `ollama pull` without PM at all. */
  function requestPull(rec: LocalRecommendation) {
    const needsTerms = !rec.licence.open && !(recs?.terms_accepted ?? []).includes(rec.licence.id);
    if (needsTerms) {
      setTermsFor(rec);
      return;
    }
    void pull(rec);
  }

  async function acceptTermsAndPull() {
    const rec = termsFor;
    if (!rec) return;
    setTermsFor(null);
    try {
      const accepted = await acceptLocalModelTerms(rec.licence.id);
      setRecs((r) => (r ? { ...r, terms_accepted: accepted } : r));
    } catch (e) {
      // The acceptance failed to persist, so the next download of this licence asks again. That is
      // the safe direction: never start the download on the back of a record that wasn't written.
      setError(String(e));
      return;
    }
    await pull(rec);
  }

  async function pull(rec: LocalRecommendation) {
    const tag = ollamaTag(rec.install.ollama);
    if (!tag) return;
    setPulling(rec.repo);
    setPullProg(null);
    setError(null);
    try {
      await pullLocalModel(tag, setPullProg);
      await reloadConfig(); // the model now shows as served / installed
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError(String(e));
    } finally {
      setPulling(null);
      setPullProg(null);
    }
  }

  const installedRepos = new Set(
    (recs?.installed ?? []).map((m) => m.matched_repo).filter(Boolean),
  );

  return (
    <>
      {error && (
        <div
          className="mt-4 rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 45%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
            color: "var(--st-due)",
          }}
        >
          {error}
        </div>
      )}

      {/* A better-fitting model is available (#437). A passive strip at the top of the tab — the
          quiet counterpart to the dots on the sidebar and the settings nav, and the thing they
          lead to. Never a modal, never a gate: dismissing it is always enough. */}
      {betterFit && (
        <div
          role="status"
          className="mt-4 flex flex-wrap items-center gap-2 rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in oklab, var(--accent) 40%, transparent)",
            background: "color-mix(in oklab, var(--accent) 10%, transparent)",
          }}
        >
          <span className="min-w-0 flex-1 text-ink2">
            <span className="text-ink">{betterFit.display_name}</span>{" "}
            {betterFit.already_downloaded
              ? "is already on this device and fits your machine better than"
              : "would fit your machine better than"}{" "}
            {betterFit.replaces}.
          </span>
          <Button
            variant="tertiary"
            onClick={() => void dismissBetterFit()}
            className="px-2 py-0.5 text-xs"
          >
            Dismiss
          </Button>
        </div>
      )}

      {/* ── Your machine ─────────────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-machine"
        data-settings-section
        data-help="settings-localai-machine"
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-center justify-between">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Your machine
          </label>
          <Button
            variant="tertiary"
            onClick={() => void rescan()}
            disabled={rescanning}
            className="px-2 py-0.5 text-xs"
          >
            {rescanning ? "Scanning…" : "Re-scan"}
          </Button>
        </div>
        {loading ? (
          <p className="mt-2 text-xs text-ink4">Scanning your hardware…</p>
        ) : recs ? (
          <HardwareReadout recs={recs} />
        ) : (
          <p className="mt-2 text-xs text-ink4">Couldn't read your hardware.</p>
        )}
        <SectionInfo title="How PM reads your machine">
          <p>
            PM checks your memory, processor, and graphics card entirely on this device — nothing is
            sent anywhere. It uses this only to work out which local models would run well, and how
            fast.
          </p>
        </SectionInfo>
      </div>

      {/* ── Recommended models ───────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-models"
        data-settings-section
        data-help="settings-localai-models"
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-baseline justify-between gap-2">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Recommended models
          </label>
          {!loading && recs && recs.curated.length > 0 && (
            <span className="shrink-0 text-[0.6875rem] text-ink4">
              {recs.curated.length} in the catalog
            </span>
          )}
        </div>
        <Collapsible title="What do these numbers mean?" defaultOpen={false} className="mt-2">
          <NumbersGuide />
        </Collapsible>
        {loading ? (
          <p className="mt-3 text-xs text-ink4">Sizing models against your machine…</p>
        ) : recs && recs.curated.length > 0 ? (
          <div className="mt-3 max-h-80 space-y-2 overflow-y-auto pr-1">
            {recs.curated.map((rec) => (
              <RecommendationCard
                key={rec.repo}
                rec={rec}
                installed={installedRepos.has(rec.repo)}
                canPull={configured && isOllama && !!ollamaTag(rec.install.ollama)}
                pulling={pulling === rec.repo}
                pullProg={pulling === rec.repo ? pullProg : null}
                onPull={() => requestPull(rec)}
                busy={pulling !== null}
              />
            ))}
          </div>
        ) : (
          <p className="mt-3 text-xs text-ink4">No catalog models to show.</p>
        )}
        <ConfirmDialog
          open={termsFor !== null}
          title={termsFor ? `${termsFor.display_name} is under the ${termsFor.licence.name}` : ""}
          confirmLabel="Accept and download"
          onConfirm={() => void acceptTermsAndPull()}
          onClose={() => setTermsFor(null)}
        >
          {termsFor && (
            <>
              <p>{termsFor.licence.summary}</p>
              <p className="mt-2">
                <a
                  href={termsFor.licence.url}
                  target="_blank"
                  rel="noreferrer noopener"
                  className="underline decoration-dotted underline-offset-2"
                >
                  Read the full terms
                </a>
                .
              </p>
              <p className="mt-2 text-ink4">
                PM doesn't download the weights — your own Ollama fetches them from the publisher,
                and PM can't enforce these terms either way. Accepting here records that you've read
                them. PM won't ask again for another model under the same licence.
              </p>
            </>
          )}
        </ConfirmDialog>
        <p className="mt-3 text-xs text-faint">
          Local models don't appear in Settings → AI &amp; Models → Usage &amp; cost — that ledger
          tracks only your paid cloud (OpenRouter) calls. Running a model on your own machine has no
          per-use cost to count.
        </p>
        {recs && (
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <label className="text-xs text-ink3" htmlFor="localai-cadence">
              Tell me when a better-fitting model appears
            </label>
            <Select
              id="localai-cadence"
              value={recs.cadence}
              onChange={(e) => void changeCadence(e.target.value)}
              className="w-auto text-xs"
            >
              <option value="on-catalog-update">When PM's model list is updated</option>
              <option value="weekly">Weekly</option>
              <option value="monthly">Monthly</option>
              <option value="manual">Never — I'll check myself</option>
            </Select>
          </div>
        )}
      </div>

      {/* ── Already downloaded ───────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-downloaded"
        data-settings-section
        data-help="settings-localai-downloaded"
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-baseline justify-between gap-2">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Already downloaded
          </label>
          {!loading && recs && recs.on_disk.length > 0 && (
            <span className="shrink-0 text-[0.6875rem] text-ink4">
              {recs.on_disk.length} on this device
            </span>
          )}
        </div>
        {loading ? (
          <p className="mt-2 text-xs text-ink4">Looking for downloaded models…</p>
        ) : recs ? (
          <DownloadedModels
            recs={recs}
            configured={configured}
            onPickFolder={pickScanFolder}
            onClearFolder={clearScanFolder}
          />
        ) : (
          <p className="mt-2 text-xs text-ink4">Couldn't check for downloaded models.</p>
        )}
        <SectionInfo title="Where PM looks, and what it reads">
          <p>
            PM checks the folders {SUPPORTED_RUNTIMES} keep their models in, so a model you've
            downloaded but aren't currently running still gets sized against your machine.
          </p>
          <p>
            It reads <span className="text-ink2">file names and sizes only</span> — never the
            contents of a model file — it writes nothing, and none of it leaves this device. Models
            it doesn't recognise are listed with an honest “can't estimate this” rather than a
            guess.
          </p>
        </SectionInfo>
      </div>

      {/* ── Connect an endpoint ──────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-endpoint"
        data-settings-section
        data-help="settings-localai-endpoint"
        className="mt-5 border-t border-border pt-4"
      >
        <div className="flex items-center justify-between">
          <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
            Connect an endpoint
          </label>
          {configured && <StatusChip status={status} />}
        </div>
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
                    No local server found. Install one below, then auto-detect again.
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
              <label className="block text-sm font-medium text-ink2">Endpoint URL</label>
              <Input
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
              <label className="block text-sm font-medium text-ink2">
                Token <span className="text-ink4">(optional — only for a remote endpoint)</span>
              </label>
              <Input
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
            <Collapsible title="Don't have a local server yet?" defaultOpen={false}>
              <RunnerInstall />
            </Collapsible>
          </div>
        )}

        <SectionInfo title="What leaves your device">
          <p>
            A server on <span className="text-ink2">this machine</span> (localhost / 127.0.0.1)
            keeps everything on your device — nothing leaves it, which is even stronger than the
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

      {/* ── Assign roles ─────────────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-roles"
        data-settings-section
        data-help="settings-localai-roles"
        className="mt-5 border-t border-border pt-4"
      >
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Assign roles
        </label>
        {!configured ? (
          <p className="mt-2 text-xs text-ink4">
            Connect an endpoint above to route PM's chat or background work to a local model.
          </p>
        ) : (
          <div className="mt-3 space-y-4">
            <RoleRow
              label="Chat"
              hint="Answers your chats."
              model={config?.chat_model ?? ""}
              routing={config?.chat_routing ?? "cloud"}
              served={served}
              onModel={(m) => changeRoleModel("chat", m)}
              onRouting={(p) => changeRouting("chat", p)}
            />
            <RoleRow
              label="Background"
              hint="Sorting proposals, titles, summaries, and learning."
              model={config?.background_model ?? ""}
              routing={config?.background_routing ?? "cloud"}
              served={served}
              onModel={(m) => changeRoleModel("background", m)}
              onRouting={(p) => changeRouting("background", p)}
            />
            {served.some((m) => m.embedding) && (
              // Unfolded on purpose. The settings doctrine folds prose but never gating hints, and
              // "this one is listed but you can't pick it" is exactly a gating hint (same call as
              // the "already downloaded" list above).
              <p className="text-xs text-ink4">
                Embedding and reranking models are listed but can't be chosen — they turn text into
                numbers for search, and can't hold a conversation. PM uses its own for search.
              </p>
            )}
          </div>
        )}
        <SectionInfo title="How routing & fallback work">
          <p>
            <span className="text-ink2">Cloud</span> keeps using your OpenRouter model.{" "}
            <span className="text-ink2">Local only</span> uses the model you picked and fails if
            it's unreachable. <span className="text-ink2">Local, fall back to cloud</span> tries
            local first and quietly hands off to your cloud model only on a hard failure (an
            unreachable or broken server) — never to chase quality.
          </p>
        </SectionInfo>
      </div>
    </>
  );
}

// ── Already downloaded (#449) ─────────────────────────────────────────────────────────────────

/** The runners PM can find models for, named in one place so the copy can't drift from the crawl. */
const SUPPORTED_RUNTIMES = "Ollama, LM Studio and Hugging Face";

const DISK_SOURCE_LABEL: Record<LocalDiskSource, string> = {
  ollama: "Ollama",
  hugging_face: "Hugging Face",
  lm_studio: "LM Studio",
  folder: "Your folder",
};

/** Models found on disk that no endpoint is serving. Distinguishes "we looked and this runner has
 *  nothing" from "this runner isn't on this machine" — an empty list means different things.
 *
 *  Exported for its test: the "you can't pick these yet" line is a GATING hint, not prose, so the
 *  settings doctrine keeps it unfolded — a test pins it rather than trusting it not to drift. */
export function DownloadedModels({
  recs,
  configured,
  onPickFolder,
  onClearFolder,
}: {
  recs: LocalRecommendations;
  /** An endpoint is saved. Decides which half of the gating hint applies. */
  configured: boolean;
  onPickFolder: () => void;
  onClearFolder: () => void;
}) {
  const found = recs.disk_sources_present
    .filter((s) => s !== "folder")
    .map((s) => DISK_SOURCE_LABEL[s]);

  return (
    <div className="mt-2">
      {recs.on_disk.length === 0 ? (
        <p className="text-xs text-ink4">
          {found.length > 0
            ? `Found ${listJoin(found)} on this device, with nothing downloaded that isn't already being served.`
            : `No model folder found for ${SUPPORTED_RUNTIMES}. If your models live somewhere else, point PM at that folder below.`}
        </p>
      ) : (
        <>
          <p className="mb-2 text-xs text-ink4">
            {configured
              ? "None of these can be assigned yet — PM can only use a model your endpoint is actually serving. Load one in the app you downloaded it with and it appears under Assign roles above."
              : "This is what's on your device, not what PM can use yet. Connect an endpoint above, then load the model in the app you downloaded it with, and it appears under Assign roles."}
          </p>
          <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
            {recs.on_disk.map((m) => (
              <OnDiskCard key={`${m.source}:${m.path}:${m.name}`} model={m} />
            ))}
          </div>
          {found.length > 0 && (
            <p className="mt-2 text-xs text-faint">Found via {listJoin(found)}.</p>
          )}
        </>
      )}

      {recs.disk_truncated && (
        <p className="mt-2 text-xs text-ink4">
          PM stopped after the first few hundred models, so this list isn't everything on your
          device.
        </p>
      )}

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button variant="tertiary" onClick={onPickFolder} className="px-2 py-0.5 text-xs">
          {recs.scan_dir ? "Change folder…" : "Also look in a folder…"}
        </Button>
        {recs.scan_dir && (
          <>
            <span className="min-w-0 break-all text-xs text-ink4">{recs.scan_dir}</span>
            <Button variant="tertiary" onClick={onClearFolder} className="px-2 py-0.5 text-xs">
              Stop looking there
            </Button>
          </>
        )}
      </div>
    </div>
  );
}

function OnDiskCard({ model }: { model: LocalOnDiskModel }) {
  return (
    <div className="rounded-[var(--radius-sm)] border border-border px-3 py-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="min-w-0 break-all text-sm text-ink2">{model.name}</span>
        <FitBadge verdict={model.fit.verdict} />
      </div>
      <p className="mt-0.5 text-xs text-ink4">
        {DISK_SOURCE_LABEL[model.source]} · {fmtGb(model.size_gb)}
        {model.quant ? ` · ${model.quant}` : ""}
        {model.shards > 1 ? ` · ${model.shards} files` : ""}
      </p>
      <ConfigRow label="In system memory" fit={model.fit} />
      {model.fit.notes.map((n, i) => (
        <p key={i} className="mt-1 text-xs text-faint">
          {n}
        </p>
      ))}
    </div>
  );
}

/** "a, b and c" — the Oxford-free list join the rest of PM's copy uses. */
function listJoin(items: string[]): string {
  if (items.length <= 1) return items[0] ?? "";
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

// ── Small pieces ──────────────────────────────────────────────────────────────────────────────

const VERDICT: Record<LocalFitVerdict, { label: string; token: string }> = {
  comfortable: { label: "Comfortable", token: "--st-quick" },
  tight: { label: "Tight fit", token: "--st-look" },
  halved_context: { label: "Reduced context", token: "--st-look" },
  stay_on_cloud: { label: "Too big — stay on cloud", token: "--st-due" },
  unknown: { label: "Unknown", token: "--ink4" },
};

function FitBadge({ verdict }: { verdict: LocalFitVerdict }) {
  const v = VERDICT[verdict];
  return (
    <span
      className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium"
      style={{
        color: `var(${v.token})`,
        background: `color-mix(in oklab, var(${v.token}) 15%, transparent)`,
      }}
    >
      {v.label}
    </span>
  );
}

function fmtGb(n: number | null): string {
  return n == null ? "—" : `${n.toFixed(1)} GB`;
}

function fmtBytes(n: number | null | undefined): string {
  if (n == null) return "—";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(0)} MB`;
  return `${(n / 1e3).toFixed(0)} KB`;
}

/** The `ollama pull <tag>` install hint reduced to just the tag (for the pull API / a copy button). */
function ollamaTag(hint: string | null): string | null {
  if (!hint) return null;
  const m = hint.trim().match(/ollama\s+(?:pull|run)\s+(\S+)/i);
  return m ? m[1] : null;
}

function HardwareReadout({ recs }: { recs: LocalRecommendations }) {
  const h = recs.hardware;
  const rows: Array<[string, string]> = [
    ["Memory", `${fmtGb(h.available_ram_gb)} free of ${fmtGb(h.total_ram_gb)}`],
    [
      "Processor",
      `${h.cpu_brand ?? "—"}${h.cpu_cores ? ` · ${h.cpu_cores} cores` : ""}${h.cpu_threads ? ` / ${h.cpu_threads} threads` : ""}`,
    ],
    [
      "Graphics",
      h.gpu_name
        ? `${h.gpu_name}${h.vram_gb ? ` · ${fmtGb(h.vram_gb)}${h.unified_memory ? " unified" : " VRAM"}` : ""}${h.gpu_bandwidth_gbps ? ` · ~${h.gpu_bandwidth_gbps.toFixed(0)} GB/s` : ""}`
        : "No dedicated GPU detected",
    ],
    ["Free disk", fmtGb(h.disk_free_gb)],
  ];
  return (
    <div className="mt-3">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-ink4">{k}</dt>
            <dd className="text-ink2">{v}</dd>
          </div>
        ))}
      </dl>
      {h.is_wsl && (
        <p className="mt-1.5 text-xs text-faint">
          Running under WSL — GPU access depends on your WSL setup.
        </p>
      )}
      {h.notes.length > 0 && <p className="mt-1.5 text-xs text-faint">{h.notes.join(" ")}</p>}
      {h.vram_gb != null && !h.unified_memory && (
        <p className="mt-1.5 text-xs text-faint">
          Sized with ~{recs.reserve_gb.toFixed(0)} GB of RAM and ~{recs.gpu_reserve_gb.toFixed(0)}{" "}
          GB of GPU memory kept free.
        </p>
      )}
      {h.vram_gb != null && !h.unified_memory && h.gpu_bandwidth_gbps == null && (
        <p className="mt-1.5 text-xs text-faint">
          Speed estimates use a default graphics-memory bandwidth — this card's exact model wasn't
          recognised.
        </p>
      )}
    </div>
  );
}

function NumbersGuide() {
  const items: Array<[string, string]> = [
    [
      "Fit",
      "Whether the model runs comfortably, only with a shrunk context, or is too big for now.",
    ],
    [
      "Quant",
      "How much the weights are compressed. Lower (e.g. Q4) is smaller and faster; higher (Q6/Q8) is more faithful but heavier.",
    ],
    [
      "Context",
      "How much text the model can consider at once. PM shrinks this to fit your memory when it has to, down to a floor.",
    ],
    [
      "q8_0 KV",
      "The running memory for the conversation is sized at f16 by default. When a card shows “q8_0 KV”, PM compressed that cache (near-lossless) so the model keeps a longer context or a higher-quality quant instead of shrinking either.",
    ],
    [
      "Speed",
      "A rough tokens-per-second estimate for how fast replies stream on your machine — higher is snappier.",
    ],
    [
      "Memory",
      "About how much RAM (or VRAM) the model needs loaded. It must sit under what you have free, with headroom.",
    ],
    [
      "MoE (mixture of experts)",
      "A large model where only a few billion parameters fire per word. It runs at the speed of that small active part, but its whole weight still has to fit in memory — so a MoE is fast for its size, not lighter to load. Cards show both the total and the active size.",
    ],
    [
      "Two ways to run (with a graphics card)",
      "When your best-quality fit is larger than your graphics memory, PM also shows a faster config that fits inside your GPU — usually a smaller quant and a shorter context, but replies stream much quicker. Both are yours to choose; PM never switches for you.",
    ],
  ];
  return (
    <dl className="mt-1 space-y-1.5 text-xs">
      {items.map(([k, v]) => (
        <div key={k}>
          <dt className="inline font-medium text-ink2">{k}: </dt>
          <dd className="inline text-ink4">{v}</dd>
        </div>
      ))}
      <p className="pt-1 text-faint">
        Numbers are estimates. Memory assumes an f16 KV cache by default; where a card shows “q8_0
        KV”, PM sized it on a compressed (near-lossless) cache to keep a larger context or quant.
        Your real speed and memory depend on your runner and settings.
      </p>
    </dl>
  );
}

/** The per-config mono metric spans (quant · context · speed · memory), shared by the single- and
 *  two-config (Split) card layouts. */
function ConfigMetrics({ fit }: { fit: LocalFitResult }) {
  return (
    <>
      {fit.quant && <span>{fit.quant}</span>}
      {fit.context != null && <span>{(fit.context / 1024).toFixed(0)}k ctx</span>}
      {fit.kv === "q8_0" && <span>q8_0 KV</span>}
      {fit.est_tokens_per_sec != null && <span>~{fit.est_tokens_per_sec.toFixed(0)} tok/s</span>}
      {fit.est_memory_gb != null && <span>{fmtGb(fit.est_memory_gb)}</span>}
    </>
  );
}

/** One labelled config row in a Split card: the mono metrics (with a “q8_0 KV” chip when the cache was
 *  compressed) plus this config's situational caveat (system-RAM vs GPU, halved/tight). */
function ConfigRow({ label, fit }: { label: string; fit: LocalFitResult }) {
  const caveat = fit.notes.join(" ");
  return (
    <div>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="shrink-0 text-[0.625rem] font-medium text-ink3">{label}</span>
        <div className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
          <ConfigMetrics fit={fit} />
        </div>
      </div>
      {caveat && <p className="mt-0.5 text-[0.625rem] text-faint">{caveat}</p>}
    </div>
  );
}

function RecommendationCard({
  rec,
  installed,
  canPull,
  pulling,
  pullProg,
  onPull,
  busy,
}: {
  rec: LocalRecommendation;
  installed: boolean;
  canPull: boolean;
  pulling: boolean;
  pullProg: PullProgress | null;
  onPull: () => void;
  busy: boolean;
}) {
  const f = rec.fit;
  // MoE when fewer params are active per token than the model holds (matches the catalog's own rule).
  const isMoe = rec.active_parameters_b + 0.01 < rec.parameters_b;
  const tag = ollamaTag(rec.install.ollama);
  const pct =
    pullProg && pullProg.total_bytes
      ? Math.min(100, Math.round((100 * (pullProg.completed_bytes ?? 0)) / pullProg.total_bytes))
      : null;
  return (
    <div className="rounded-[var(--radius-sm)] border border-border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-sm font-medium text-ink">{rec.display_name}</span>
            <FitBadge verdict={f.verdict} />
            {isMoe && <span className="text-[0.625rem] text-ink4">MoE</span>}
            {rec.multimodal && <span className="text-[0.625rem] text-ink4">vision</span>}
            {rec.reasoning && <span className="text-[0.625rem] text-ink4">reasoning</span>}
            {rec.role_hint && (
              <span className="text-[0.625rem] text-ink4">suits {rec.role_hint}</span>
            )}
            {/* Every row says what its weights are under. A restricted licence is the one worth
                catching the eye, so it takes the attention colour the rest of the chips don't. */}
            <a
              href={rec.licence.url}
              target="_blank"
              rel="noreferrer noopener"
              className={`text-[0.625rem] underline decoration-dotted underline-offset-2 ${
                rec.licence.open ? "text-ink4" : "text-st-due"
              }`}
            >
              {rec.licence.name}
            </a>
          </div>
          {rec.gpu.kind === "split" ? (
            <div className="mt-1 space-y-1">
              <div className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
                <span>
                  {rec.parameters_b}B{isMoe ? " total" : ""}
                </span>
                {isMoe && <span>{rec.active_parameters_b}B active</span>}
              </div>
              <p className="text-[0.625rem] text-ink4">
                Two ways to run it here — same model, lighter settings for speed:
              </p>
              <ConfigRow label="Highest quality" fit={f} />
              <ConfigRow label="Fastest on GPU" fit={rec.gpu.fit} />
              {canPull && (
                <p className="text-[0.625rem] text-faint">
                  Download fetches the runner's default quant — set the quant &amp; context to
                  match.
                </p>
              )}
            </div>
          ) : (
            <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
              <span>
                {rec.parameters_b}B{isMoe ? " total" : ""}
              </span>
              {isMoe && <span>{rec.active_parameters_b}B active</span>}
              {f.quant && <span>{f.quant}</span>}
              {f.context != null && <span>{(f.context / 1024).toFixed(0)}k ctx</span>}
              {f.kv === "q8_0" && <span>q8_0 KV</span>}
              {f.est_tokens_per_sec != null && (
                <span>~{f.est_tokens_per_sec.toFixed(0)} tok/s</span>
              )}
              {f.est_memory_gb != null && <span>{fmtGb(f.est_memory_gb)}</span>}
            </div>
          )}
        </div>
        <div className="shrink-0">
          {installed ? (
            <span className="text-xs font-medium text-st-quick">Installed</span>
          ) : canPull ? (
            <Button
              variant="secondary"
              onClick={onPull}
              disabled={busy || f.verdict === "stay_on_cloud"}
              className="text-xs"
            >
              {pulling ? "Downloading…" : "Download"}
            </Button>
          ) : null}
        </div>
      </div>

      {pulling && (
        <div className="mt-2">
          <div className="h-1.5 w-full overflow-hidden rounded-full bg-surface">
            <div
              className="h-full rounded-full bg-accent transition-[width] duration-300"
              style={{ width: pct != null ? `${pct}%` : "100%" }}
            />
          </div>
          <p className="mt-1 font-mono text-[0.625rem] text-ink4">
            {pullProg?.status ?? "starting…"}
            {pullProg?.total_bytes
              ? ` · ${fmtBytes(pullProg.completed_bytes)} / ${fmtBytes(pullProg.total_bytes)}`
              : ""}
          </p>
        </div>
      )}

      {!installed && !canPull && tag && (
        <div className="mt-2 flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
            ollama pull {tag}
          </code>
          <Button
            variant="tertiary"
            onClick={() => void navigator.clipboard?.writeText(`ollama pull ${tag}`)}
            className="px-2 py-0.5 text-xs"
          >
            Copy
          </Button>
        </div>
      )}

      {rec.gpu.kind === "split"
        ? // Each Split row states its own caveat (and KV chip) via ConfigRow — nothing shared to add.
          null
        : f.notes.length > 0 && (
            <p className="mt-1.5 text-[0.6875rem] text-faint">{f.notes.join(" ")}</p>
          )}
    </div>
  );
}

function StatusChip({ status }: { status: LocalLlmStatus | null }) {
  let label = "Checking…";
  let token = "--ink4";
  if (status) {
    if (status.in_cooldown) {
      label = `Cooling down (${status.cooldown_remaining_s}s)`;
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
  const warn =
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

const ROUTING_OPTIONS = [
  { value: "cloud", label: "Cloud" },
  { value: "local", label: "Local only" },
  { value: "local-then-cloud", label: "Local, fall back to cloud" },
];

function RoleRow({
  label,
  hint,
  model,
  routing,
  served,
  onModel,
  onRouting,
}: {
  label: string;
  hint: string;
  model: string;
  routing: string;
  served: LocalServedModel[];
  onModel: (m: string) => void;
  onRouting: (p: string) => void;
}) {
  // Keep the currently-saved model selectable even if the endpoint isn't serving it right now.
  // The embedder gate is applied AFTER this line, not by filtering `served` before it — otherwise
  // an embedder a user had already saved would slip back in through this branch.
  // A saved model the endpoint isn't serving right now stays enabled: it is already the current
  // value, disabling it would render the Select's own selection greyed out, and the gate's job is
  // to stop a NEW bad assignment. That also keeps the embedder predicate in exactly one place
  // (Rust), rather than a second copy here that could drift.
  const saved = served.some((m) => m.id === model);
  const options: LocalServedModel[] =
    model && !saved ? [{ id: model, embedding: false }, ...served] : served;
  return (
    <div>
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-sm font-medium text-ink2">{label}</span>
        <span className="text-[0.6875rem] text-ink4">{hint}</span>
      </div>
      <div className="mt-1.5 flex flex-wrap items-center gap-2">
        <Select
          value={model}
          onChange={(e) => onModel(e.target.value)}
          className="min-w-[10rem] flex-1"
        >
          <option value="">— use cloud —</option>
          {options.map((m) => (
            // Shown, not hidden: a model you can see in Ollama but not in PM reads as a PM bug,
            // whereas one shown with its reason reads as an explanation — and it makes a
            // mis-classification visible instead of a model that silently vanished.
            <option key={m.id} value={m.id} disabled={m.embedding}>
              {m.embedding ? `${m.id} — embedding model` : m.id}
            </option>
          ))}
        </Select>
        <Select value={routing} onChange={(e) => onRouting(e.target.value)}>
          {ROUTING_OPTIONS.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </Select>
      </div>
    </div>
  );
}

function RunnerInstall() {
  const g = ollamaGuide();
  return (
    <div className="mt-1 text-xs text-ink4">
      <p className="text-ink2">{g.name}</p>
      <p className="mt-0.5">{g.summary}</p>
      <ol className="ml-4 mt-1.5 list-decimal space-y-1">
        {g.steps.map((s, i) => (
          <li key={i}>{s}</li>
        ))}
      </ol>
      <p className="mt-1.5">
        LM Studio and llama-server also work — connect them by URL above (they have no one-click
        download, so you pick models in their own app).
      </p>
    </div>
  );
}
