// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  activeLocalPull,
  cancelLocalPull,
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
  PullSnapshot,
} from "../../lib/types";
import { formatBytes, formatGib } from "../../lib/format";
import { IngestProgress } from "../IngestProgress";
import { installCommand, runnerGuides } from "../../lib/workbenchGuide";
import {
  Button,
  Callout,
  Collapsible,
  ConfirmDialog,
  Input,
  SectionInfo,
  SectionLabel,
  Select,
  useFieldA11y,
} from "../ui";

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
  // Whether `served` is an ANSWER rather than a starting value. It starts empty and is filled
  // asynchronously, and a listing that fails leaves it empty too — so "empty" alone cannot tell
  // "this server serves nothing" from "we haven't asked yet" or "we asked and couldn't reach it".
  // Only a resolved listing sets this, so copy that speaks for the empty case can never claim a
  // server serves nothing when PM simply doesn't know.
  const [servedLoaded, setServedLoaded] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Endpoint form.
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

  // Model pull (Ollama only). `pulling` holds the pull TAG (`hf.co/<repo>:<QUANT>`), not the repo:
  // the backend's job snapshot is keyed on the tag, so a view that mounts mid-download can adopt it
  // and mark the right card.
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProg, setPullProg] = useState<PullProgress | null>(null);
  /** The model whose licence terms are being shown, and the pull tag the user asked for, or null
   *  when no dialog is open. The TAG rides along because a card can offer more than one way to run
   *  the same model: resolving it again after the dialog would resolve the card's default, not the
   *  row the user actually clicked. */
  const [termsFor, setTermsFor] = useState<{ rec: LocalRecommendation; tag: string } | null>(null);
  /** Mirrors `pulling` synchronously. `pull()` below is async, so the `pulling` it captured when it
   *  started is stale by the time its `finally` runs — and that `finally` must be able to tell
   *  whether the tag it started is still the one on screen. */
  const pullingRef = useRef<string | null>(null);

  const configured = !!config?.base_url;
  // Both roles actually going to the local server, and going there with DIFFERENT models — the only
  // case where the two choices interact, since one server holding one model costs what the Workbench
  // said it would. Guarded this narrowly so the warning never fires on the common setups.
  const bothLocal = config?.chat_routing !== "cloud" && config?.background_routing !== "cloud";
  const twoModels =
    !!config?.chat_model &&
    !!config?.background_model &&
    config.chat_model !== config.background_model;
  // Whether the connected endpoint is an Ollama server (the only runner with a one-click pull API).
  // Heuristic: Ollama's default port — parsed from the URL, not a substring test (":114341", or
  // "11434" anywhere in a path, must not count). An Ollama on a custom port degrades honestly to
  // the copy-paste command; anything else on 11434 gets a button whose pull fails with a clear
  // error. A real flavour probe is the better gate if this ever grows a third consumer.
  const isOllama = (() => {
    if (!config?.base_url) return false;
    try {
      return new URL(config.base_url).port === "11434";
    } catch {
      return false;
    }
  })();

  /** The single writer for the pull marker. Keeps `pullingRef` in step with the state so the two
   *  can never disagree — a marker cleared in one and not the other silently kills the 1s snapshot
   *  poller, whose effect dependency is `pulling`. */
  const markPulling = useCallback((tag: string | null) => {
    pullingRef.current = tag;
    setPulling(tag);
  }, []);

  /** Mirror the backend's pull job into the view. The snapshot is the source of truth: it survives
   *  this view unmounting, and it is the only thing that knows about a download this component did
   *  not start. A snapshot with nothing running is deliberately NOT a reset — callers decide that. */
  const applyPullSnapshot = useCallback(
    (snap: PullSnapshot | null) => {
      if (!snap?.running) return false;
      markPulling(snap.model);
      setPullProg({
        status: snap.status,
        completed_bytes: snap.completed_bytes,
        total_bytes: snap.total_bytes,
        done: false,
      });
      return true;
    },
    [markPulling],
  );

  async function reloadConfig() {
    const cfg = await getLocalLlmConfig();
    setConfig(cfg);
    if (cfg.base_url) {
      setUrlInput((u) => u || cfg.base_url || "");
      listLocalLlmModels()
        .then((m) => {
          setServed(m);
          setServedLoaded(true);
        })
        .catch(() => {
          setServed([]);
          setServedLoaded(false);
        });
    } else {
      setServed([]);
      setServedLoaded(false);
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
            .then((m) => {
              if (cancelled) return;
              setServed(m);
              setServedLoaded(true);
            })
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
  // once / 30s, so this can't hammer the user's server). The served-model list rides the same tick:
  // the copy under Assign roles promises "download a model and it appears here", and before this the
  // list only refreshed on save/pull — a user following the copy-paste path while sitting on the tab
  // waited forever.
  useEffect(() => {
    if (!configured) {
      setStatus(null);
      return;
    }
    let cancelled = false;
    const tick = () => {
      localLlmStatus()
        .then((s) => !cancelled && setStatus(s))
        .catch(() => {});
      listLocalLlmModels()
        .then((m) => {
          if (cancelled) return;
          setServed(m);
          setServedLoaded(true);
        })
        .catch(() => {
          /* transient listing failure: keep the last answer rather than flashing "serves nothing" */
        });
    };
    void tick();
    const id = setInterval(tick, 30000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [configured]);

  // A download owned by the BACKEND may be running while this view mounts (the tab router unmounts
  // on every switch): adopt it, and while any pull is marked running keep re-reading the snapshot —
  // it is the source of truth that survives the unmount, and its terminal state carries the error
  // a channel nobody was listening to could not deliver.
  useEffect(() => {
    let cancelled = false;
    void activeLocalPull()
      .then((snap) => {
        if (cancelled) return;
        applyPullSnapshot(snap);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [applyPullSnapshot]);
  useEffect(() => {
    if (pulling === null) return;
    let cancelled = false;
    const id = setInterval(() => {
      void activeLocalPull()
        .then((snap) => {
          if (cancelled || !snap || snap.model !== pulling) return;
          if (snap.running) {
            setPullProg({
              status: snap.status,
              completed_bytes: snap.completed_bytes,
              total_bytes: snap.total_bytes,
              done: false,
            });
            return;
          }
          // Terminal. The locally-started path also lands here if its invoke handler is gone.
          markPulling(null);
          setPullProg(null);
          if (snap.error) setError(snap.error);
          void reloadConfig();
          void localModelRecommendations()
            .then((r) => !cancelled && setRecs(r))
            .catch(() => {});
        })
        .catch(() => {});
    }, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pulling]);

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
  function requestPull(rec: LocalRecommendation, tag: string) {
    const needsTerms = !rec.licence.open && !(recs?.terms_accepted ?? []).includes(rec.licence.id);
    if (needsTerms) {
      setTermsFor({ rec, tag });
      return;
    }
    void pull(tag);
  }

  async function acceptTermsAndPull() {
    const pending = termsFor;
    if (!pending) return;
    const { rec, tag } = pending;
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
    await pull(tag);
  }

  async function pull(tag: string) {
    markPulling(tag);
    setPullProg(null);
    setError(null);
    try {
      // The job itself is backend-owned (it survives this view unmounting); the channel is just
      // the low-latency progress feed while we ARE mounted — the 1s snapshot poll is the fallback.
      await pullLocalModel(tag, setPullProg);
      await reloadConfig(); // the model now shows as served / installed
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError(String(e));
      // The job is backend-owned and the backend refuses a second concurrent pull, so this is the
      // ordinary outcome of clicking a second Download while one runs. The optimistic mark above has
      // already displaced whatever was running; recover it from the snapshot rather than dropping to
      // null, because null tears down the 1s poller (its dependency is `pulling`) and leaves a live
      // download with no progress bar and no Cancel until the view happens to remount.
      applyPullSnapshot(await activeLocalPull().catch(() => null));
    } finally {
      // Per-tag, never unconditional: the re-adoption above may have just put ANOTHER pull on
      // screen, and this reset runs after it.
      if (pullingRef.current === tag) {
        markPulling(null);
        setPullProg(null);
      }
    }
  }

  const installedRepos = new Set(
    (recs?.installed ?? []).map((m) => m.matched_repo).filter(Boolean),
  );

  return (
    <>
      {error && <Callout className="mt-4">{error}</Callout>}

      {/* A better-fitting model is available (#437). A passive strip at the top of the tab — the
          quiet counterpart to the dots on the sidebar and the settings nav, and the thing they
          lead to. Never a modal, never a gate: dismissing it is always enough. */}
      {betterFit && (
        <Callout tone="info" body="ink" live className="mt-4 flex flex-wrap items-center gap-2">
          <span className="min-w-0 flex-1 text-ink2">
            <span className="text-ink">{betterFit.display_name}</span>{" "}
            {betterFit.already_downloaded
              ? "is already on this device and fits your machine better than"
              : "would fit your machine better than"}{" "}
            {betterFit.replaces}.
          </span>
          <Button variant="tertiary" size="sm" onClick={() => void dismissBetterFit()}>
            Dismiss
          </Button>
        </Callout>
      )}

      {/* ── Your machine ─────────────────────────────────────────────────────────────────── */}
      <div
        id="sec-localai-machine"
        data-settings-section
        data-help="settings-localai-machine"
        className="mt-5 border-t border-border pt-4"
      >
        <SectionLabel
          action={
            <Button
              variant="tertiary"
              size="sm"
              onClick={() => void rescan()}
              disabled={rescanning}
            >
              {rescanning ? "Scanning…" : "Re-scan"}
            </Button>
          }
        >
          Your machine
        </SectionLabel>
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
        <SectionLabel
          align="baseline"
          action={
            !loading &&
            recs &&
            recs.curated.length > 0 && (
              <span className="shrink-0 text-[0.6875rem] text-ink4">
                {recs.curated.length} in the catalog
              </span>
            )
          }
        >
          Recommended models
        </SectionLabel>
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
                canPull={configured && isOllama && !!rec.ollama_pull}
                pulling={pulling !== null && pulling === rec.ollama_pull}
                pullProg={pulling !== null && pulling === rec.ollama_pull ? pullProg : null}
                onPull={() => rec.ollama_pull && requestPull(rec, rec.ollama_pull)}
                onCancel={() => void cancelLocalPull().catch(() => {})}
                busy={pulling !== null}
              />
            ))}
          </div>
        ) : (
          <p className="mt-3 text-xs text-ink4">No catalog models to show.</p>
        )}
        <ConfirmDialog
          open={termsFor !== null}
          title={
            termsFor ? `${termsFor.rec.display_name} is under the ${termsFor.rec.licence.name}` : ""
          }
          confirmLabel="Accept and download"
          onConfirm={() => void acceptTermsAndPull()}
          onClose={() => setTermsFor(null)}
        >
          {termsFor && (
            <>
              <p>{termsFor.rec.licence.summary}</p>
              <p className="mt-2">
                <a
                  href={termsFor.rec.licence.url}
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
        <p className="mt-3 text-xs text-ink4">
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
        <SectionLabel
          align="baseline"
          action={
            !loading &&
            recs &&
            recs.on_disk.length > 0 && (
              <span className="shrink-0 text-[0.6875rem] text-ink4">
                {recs.on_disk.length} on this device
              </span>
            )
          }
        >
          Already downloaded
        </SectionLabel>
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
        <SectionLabel>Assign roles</SectionLabel>
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
            {servedLoaded && served.length === 0 && (
              // Unfolded, for the same reason as the two hints below: the settings doctrine folds
              // prose but never a gating hint, and "both dropdowns are empty, and here is why" is
              // exactly one. This state only became something a user can sit in once the endpoint
              // check learned to accept a server with an empty model list (#790) — before that a
              // fresh runner failed to connect at all, so nothing here had to speak for it.
              <p className="text-xs text-ink4">
                This server isn't serving any models yet, so there is nothing to assign and both
                roles stay on cloud. Download a model into it and it will appear here within about
                half a minute.
              </p>
            )}
            {bothLocal && twoModels && (
              // Unfolded, for the same reason as the gating hint below: the doctrine folds prose but
              // never a loss warning, and this is one. Chat and Background share ONE endpoint, so
              // two different models are both resident on one machine and their memory adds up —
              // while every verdict in the Workbench was worked out for a single model against the
              // whole budget. Two independently "Comfortable" models can be jointly impossible, and
              // nothing else in PM says so: the better-fit check compares against the LARGER of the
              // two, never their sum, so it cannot warn about this either.
              <p className="text-xs text-ink4">
                Chat and Background go to the same server, so picking a different model for each
                means your machine holds both at once. The fit shown in Local AI Workbench is for
                one model on its own — two that each fit alone may not fit together.
              </p>
            )}
            {status?.served_window != null && status.served_window < COMFORTABLE_WINDOW && (
              // Unfolded: a loss warning, not prose. This is the number that explains the symptom
              // people blame on model size. A server serving 4096 tokens (Ollama's default, applied
              // silently) cannot hold one filing batch, so PM now sends fewer documents per call —
              // and past a point it stops rather than let the server cut the instructions off the
              // front of the prompt and answer anyway.
              <p className="text-xs text-ink4">
                Your server is serving{" "}
                <span className="text-ink2">
                  {status.served_window.toLocaleString()} tokens
                  {status.served_window_proven ? "" : " (PM's estimate — it hasn't measured yet)"}
                </span>{" "}
                of context. PM's background work — sorting proposals, summaries, learning — sends
                more than that in one go, so it will send smaller batches to fit. Raising it makes
                that work better: Ollama uses{" "}
                <span className="text-ink2">OLLAMA_CONTEXT_LENGTH</span>, llama-server uses{" "}
                <span className="text-ink2">--ctx-size</span>, and LM Studio has a context-length
                slider on the model.
              </p>
            )}
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
          {/* Three states, not two. `on_disk` has already had everything the endpoint serves
              removed from it, so "empty" alone cannot tell a machine with no runner installed from
              one whose runner is installed but empty — which is exactly what a user sees the moment
              they remove their last model. `disk_found` is the pre-filter count. */}
          {found.length === 0
            ? `No model folder found for ${SUPPORTED_RUNTIMES}. If your models live somewhere else, point PM at that folder below.`
            : recs.disk_found === 0
              ? `Found ${listJoin(found)} on this device, but nothing downloaded into it yet.`
              : `Found ${listJoin(found)} on this device, with nothing downloaded that isn't already being served.`}
        </p>
      ) : (
        <>
          <p className="mb-2 text-xs text-ink4">
            {configured
              ? "None of these can be assigned yet — PM can only use a model your endpoint is actually serving. Load one in the app you downloaded it with and it shows up under Assign roles above within about half a minute."
              : "This is what's on your device, not what PM can use yet. Connect an endpoint above, then load the model in the app you downloaded it with, and it appears under Assign roles."}
          </p>
          <div className="max-h-72 space-y-2 overflow-y-auto pr-1">
            {recs.on_disk.map((m) => (
              <OnDiskCard key={`${m.source}:${m.path}:${m.name}`} model={m} />
            ))}
          </div>
          {found.length > 0 && (
            <p className="mt-2 text-xs text-ink4">Found via {listJoin(found)}.</p>
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
        <Button variant="tertiary" size="sm" onClick={onPickFolder}>
          {recs.scan_dir ? "Change folder…" : "Also look in a folder…"}
        </Button>
        {recs.scan_dir && (
          <>
            <span className="min-w-0 break-all text-xs text-ink4">{recs.scan_dir}</span>
            <Button variant="tertiary" size="sm" onClick={onClearFolder}>
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
        {DISK_SOURCE_LABEL[model.source]} · {formatGib(model.size_gb)}
        {model.quant ? ` · ${model.quant}` : ""}
        {model.shards > 1 ? ` · ${model.shards} files` : ""}
      </p>
      <ConfigRow label="In system memory" fit={model.fit} />
      {model.fit.notes.map((n, i) => (
        <p key={i} className="mt-1 text-xs text-ink4">
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

/** How to get a model PM can't download for you.
 *
 *  Honest per runner rather than one command pretending to be universal: the three name models three
 *  different ways, and the same weights are `qwen2.5:7b-instruct-q4_K_M` to Ollama, `…@q4_k_m` to LM
 *  Studio and `user/repo:Q4_K_M` to llama-server. Pasting one into another gets you nothing. So PM
 *  prints the command it can stand behind and describes the route for the two it can't. */
function ModelInstallHint({
  repo,
  quant,
  ollamaPull,
  shardedQuant,
}: {
  repo: string;
  quant: string | null;
  ollamaPull: string | null;
  shardedQuant: boolean;
}) {
  const cmd = installCommand("llama-server", repo, quant);
  return (
    <div className="mt-2 space-y-1.5">
      {cmd && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
            {cmd}
          </code>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void navigator.clipboard?.writeText(cmd)}
          >
            Copy
          </Button>
        </div>
      )}
      {ollamaPull && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
            {`ollama pull ${ollamaPull}`}
          </code>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void navigator.clipboard?.writeText(`ollama pull ${ollamaPull}`)}
          >
            Copy
          </Button>
        </div>
      )}
      <p className="text-[0.6875rem] text-ink4">
        That command downloads and serves it in one step. In LM Studio, paste{" "}
        <span className="font-mono text-ink3">{repo}</span> into the Discover tab's search.
        {shardedQuant
          ? " Ollama can't fetch this quantization — it ships as split files, which Ollama won't pull. A smaller one of the same model will work."
          : ""}
      </p>
    </div>
  );
}

function HardwareReadout({ recs }: { recs: LocalRecommendations }) {
  const h = recs.hardware;
  const rows: Array<[string, string]> = [
    ["Memory", `${formatGib(h.available_ram_gb)} free of ${formatGib(h.total_ram_gb)}`],
    [
      "Processor",
      `${h.cpu_brand ?? "—"}${h.cpu_cores ? ` · ${h.cpu_cores} cores` : ""}${h.cpu_threads ? ` / ${h.cpu_threads} threads` : ""}`,
    ],
    [
      "Graphics",
      h.gpu_name
        ? `${h.gpu_name}${h.vram_gb ? ` · ${formatGib(h.vram_gb)}${h.unified_memory ? " unified" : " VRAM"}` : ""}${h.gpu_bandwidth_gbps ? ` · ~${h.gpu_bandwidth_gbps.toFixed(0)} GB/s` : ""}`
        : "No dedicated GPU detected",
    ],
    ["Free disk", formatGib(h.disk_free_gb)],
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
        <p className="mt-1.5 text-xs text-ink4">
          Running under WSL — GPU access depends on your WSL setup.
        </p>
      )}
      {h.notes.length > 0 && <p className="mt-1.5 text-xs text-ink4">{h.notes.join(" ")}</p>}
      {/* The RAM reserve is subtracted on EVERY machine, so it is stated on every machine. This whole
          line used to be gated on having a discrete GPU, which meant a CPU-only box, an Apple
          Silicon Mac and any laptop on integrated graphics were never told their fit was scored
          against free RAM minus a reserve — the one number most likely to explain a verdict they
          disagreed with. Only the GPU half is conditional now. */}
      <p className="mt-1.5 text-xs text-ink4">
        Sized with ~{recs.reserve_gb.toFixed(0)} GB of RAM
        {h.vram_gb != null && !h.unified_memory
          ? ` and ~${recs.gpu_reserve_gb.toFixed(0)} GB of GPU memory`
          : ""}{" "}
        kept free, measured as PM scored these models.
      </p>
      {h.vram_gb != null && !h.unified_memory && h.gpu_bandwidth_gbps == null && (
        <p className="mt-1.5 text-xs text-ink4">
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
      <p className="pt-1 text-ink4">
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
      {fit.est_memory_gb != null && <span>{formatGib(fit.est_memory_gb)}</span>}
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
      {caveat && <p className="mt-0.5 text-[0.625rem] text-ink4">{caveat}</p>}
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
  onCancel,
  busy,
}: {
  rec: LocalRecommendation;
  installed: boolean;
  canPull: boolean;
  pulling: boolean;
  pullProg: PullProgress | null;
  onPull: () => void;
  onCancel: () => void;
  busy: boolean;
}) {
  const f = rec.fit;
  // MoE when fewer params are active per token than the model holds (matches the catalog's own rule).
  const isMoe = rec.active_parameters_b + 0.01 < rec.parameters_b;
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
            {/* No "vision" chip, though `rec.multimodal` still says so. This row is what PM will do
                with the model, and PM cannot send it an image: chat messages carry a plain string,
                so no picture reaches any model, cloud or local. PM reads images through the sidecar
                instead. Advertising it here made a heavier model look more capable for PM's purposes
                than a lighter one, which is the opposite of true. A chip has no room for the caveat,
                which is itself the argument for leaving it out. */}
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
                // Since #793 the button pulls the exact quant the "Highest quality" row measured
                // (`hf.co/<repo>:<QUANT>`), so say that — the old "runner's default quant" caption
                // predates the verified-tag route and told this card's users the opposite.
                <p className="text-[0.625rem] text-ink4">
                  Download fetches the Highest-quality file measured above. Fastest on GPU is a
                  different file (its quant is on its row) — PM's button doesn't fetch that one.
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
              {f.est_memory_gb != null && <span>{formatGib(f.est_memory_gb)}</span>}
            </div>
          )}
        </div>
        <div className="shrink-0">
          {installed ? (
            <span className="text-xs font-medium text-st-quick">Installed</span>
          ) : canPull ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={onPull}
              disabled={busy || f.verdict === "stay_on_cloud"}
            >
              {pulling ? "Downloading…" : "Download"}
            </Button>
          ) : null}
        </div>
      </div>

      {pulling && (
        <div className="mt-2">
          {/* The shared per-depth progress surface: shimmer while the total is unknown (the
              manifest/verify phases used to render a FULL bar, which reads as "done"), percent
              once bytes flow. The status line stays — it is a status readout, never folded. */}
          <IngestProgress
            processed={pct ?? 0}
            total={pct != null ? 100 : null}
            label={`Downloading ${rec.display_name}`}
            mode="percent"
          />
          <div className="mt-1 flex items-center justify-between gap-2">
            <p className="min-w-0 truncate font-mono text-[0.625rem] text-ink4">
              {pullProg?.status ?? "starting…"}
              {pullProg?.total_bytes
                ? ` · ${formatBytes(pullProg.completed_bytes)} / ${formatBytes(pullProg.total_bytes)}`
                : ""}
            </p>
            <Button variant="tertiary" size="sm" onClick={onCancel}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {!installed && !canPull && (
        // The row that offers a way to GET this model when PM can't fetch it for you — no endpoint
        // connected, or the endpoint isn't Ollama. Both commands are real: llama-server takes the
        // Hugging Face repo id directly, and Ollama takes the catalogue's verified `hf.co/…` tag.
        // When the fitted quant is sharded there is no Ollama command to give, and it says why.
        <ModelInstallHint
          repo={rec.repo}
          quant={f.quant}
          ollamaPull={rec.ollama_pull}
          shardedQuant={rec.sharded_quant}
        />
      )}

      {rec.gpu.kind === "split"
        ? // Each Split row states its own caveat (and KV chip) via ConfigRow — nothing shared to add.
          null
        : f.notes.length > 0 && (
            <p className="mt-1.5 text-[0.6875rem] text-ink4">{f.notes.join(" ")}</p>
          )}
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

/// Below this served window PM says so under Assign roles. One filing batch is ~3.5k tokens of
/// prompt before the reply reserve, so 8192 is the point at which a batch stops being comfortable
/// rather than the point at which it breaks — a user is better told early than told by the work
/// quietly getting worse.
const COMFORTABLE_WINDOW = 8192;

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
            {/* Unfolded, never a caret: a hardware exclusion or a "it stops when you close the
                window" is a gating fact, and the settings doctrine folds prose but not those. */}
            {g.caveat && <p className="mt-1 text-ink3">Worth knowing: {g.caveat}</p>}
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
