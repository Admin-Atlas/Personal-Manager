// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  dismissLocalBetterFit,
  getLocalLlmConfig,
  listLocalLlmModels,
  localBetterFitNotice,
  localHardwareScan,
  localLlmStatus,
  localModelRecommendations,
  onLocalLlmStatus,
  setLocalModelRescanCadence,
  setLocalModelScanDir,
} from "../../lib/ipc";
import type {
  LocalBetterFit,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalRecommendations,
  LocalRescanCadence,
  LocalServedModel,
} from "../../lib/types";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import { LocalAiCatalog } from "./LocalAiCatalog";
import { LocalAiDownloaded } from "./LocalAiDownloaded";
import { LocalAiEndpoint } from "./LocalAiEndpoint";
import { LocalAiLifecycle } from "./LocalAiLifecycle";
import { LocalAiMachine } from "./LocalAiMachine";
import { LocalAiRoles } from "./LocalAiRoles";
import { Button, Callout } from "../ui";

/** The Local AI tab (#296): read this machine's hardware, size a curated model catalog against it,
 *  and turn on the local-endpoint provider (#297) — connect a local server, assign it to the chat /
 *  background roles, with cloud fallback. Self-contained and immediate-persist; errors surface inline.
 *  Frontend-only over existing backend commands, plus the one streaming Ollama pull.
 *
 *  This file is the tab, not the sections. It owns exactly what more than one section reads — the
 *  stored config, the live status, the served-model list, the hardware/catalog scan — and the
 *  reloads that refresh them. Everything that belongs to one section lives with it: the endpoint
 *  form, the download, the role tests. Five section files rather than one screenful each of a
 *  1,100-line function, which is what this was. */
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
  /** Bumped whenever the stored endpoint or its token changes. It is the `key` on the roles
   *  section, so anything that section proved about the OLD server — a passing test — goes with the
   *  mount rather than having to be remembered and invalidated from up here. */
  const [endpointEpoch, setEndpointEpoch] = useState(0);

  const configured = !!config?.base_url;
  // A role that reaches the local server AND has a model bound. Both halves are load-bearing.
  // Without the routing half the "not measured yet" line fires for someone entirely on cloud, who
  // has no local model to measure. Without the model half it fires for someone who flipped routing
  // to local and left the select on "— use cloud —": `role_local_model` returns null for an empty
  // model, so the window would be null forever and the line would promise a reading that can never
  // arrive, because the gateway treats an absent model as unconfigured.
  const anyLocalRoleWithModel =
    (config?.chat_routing !== "cloud" && !!config?.chat_model?.trim()) ||
    (config?.background_routing !== "cloud" && !!config?.background_model?.trim());
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

  async function reloadConfig() {
    const cfg = await getLocalLlmConfig();
    setConfig(cfg);
    if (cfg.base_url) {
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

  async function refreshRecs() {
    try {
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError(String(e));
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
    // Also on the push signal. The status carries whether a role is answering RIGHT NOW, and a 30 s
    // tick cannot report a thing that lasts six seconds: the "this model is answering, so a test
    // would wait its turn" hint beside each role would be a coin flip. The event fires when a call
    // starts and again when it ends, so the hint appears and clears with the call.
    const offStatus = subscribeUntilCleanup(() =>
      onLocalLlmStatus(() => {
        localLlmStatus()
          .then((s) => !cancelled && setStatus(s))
          .catch(() => {});
      }),
    );
    return () => {
      cancelled = true;
      clearInterval(id);
      offStatus();
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

  const installedRepos = new Set(
    (recs?.installed ?? []).map((m) => m.matched_repo).filter((r): r is string => r !== null),
  );
  // The ids the endpoint actually serves, lower-cased. An `hf.co/<repo>:<QUANT>` pull is served
  // under the very tag it was pulled with (measured against a live Ollama 0.33), so a card can match
  // a RUNG exactly instead of matching the repo — which reported "Installed" for a quant that was
  // neither of the two the card offers.
  const servedTags = new Set(served.map((m) => m.id.toLowerCase()));

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

      <LocalAiMachine
        recs={recs}
        loading={loading}
        rescanning={rescanning}
        onRescan={() => void rescan()}
      />

      <LocalAiCatalog
        recs={recs}
        loading={loading}
        configured={configured}
        isOllama={isOllama}
        servedTags={servedTags}
        installedRepos={installedRepos}
        onRecs={setRecs}
        onReload={reloadConfig}
        onRefreshRecs={refreshRecs}
        onCadence={(c) => void changeCadence(c)}
        onError={setError}
      />

      <LocalAiDownloaded
        recs={recs}
        loading={loading}
        configured={configured}
        onPickFolder={() => void pickScanFolder()}
        onClearFolder={() => void clearScanFolder()}
      />

      <LocalAiEndpoint
        config={config}
        status={status}
        configured={configured}
        onReload={reloadConfig}
        onError={setError}
        onEndpointChanged={() => setEndpointEpoch((n) => n + 1)}
      />

      <LocalAiRoles
        key={endpointEpoch}
        config={config}
        status={status}
        served={served}
        servedLoaded={servedLoaded}
        configured={configured}
        coResidency={recs?.co_residency ?? null}
        anyLocalRoleWithModel={anyLocalRoleWithModel}
        onConfigPatch={(patch) => setConfig((c) => (c ? { ...c, ...patch } : c))}
        onError={setError}
      />

      <LocalAiLifecycle configured={configured} />
    </>
  );
}
