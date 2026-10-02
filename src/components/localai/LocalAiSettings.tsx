// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import {
  dismissLocalBetterFit,
  getLocalLlmConfig,
  listLocalLlmModels,
  localAiSettingsAreDefault,
  localBetterFitNotice,
  localHardwareScan,
  localLlmStatus,
  localModelRecommendations,
  onLocalLlmStatus,
  resetLocalAiSettings,
  setLocalLlmEndpoint,
  setLocalLlmRoleModel,
  setLocalLlmRouting,
  setLocalModelRescanCadence,
  setLocalModelScanDir,
} from "../../lib/ipc";
import type { LocalRole } from "../../lib/localModelState";
import type {
  LocalBetterFit,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalRecommendations,
  LocalRescanCadence,
  LocalServedModel,
} from "../../lib/types";
import { onSettingSaved } from "../../lib/settingsSaved";
import { subscribeUntilCleanup } from "../../lib/subscribe";
import { LocalAiCatalog } from "./LocalAiCatalog";
import { LocalAiDownloaded } from "./LocalAiDownloaded";
import { LocalAiEndpoint } from "./LocalAiEndpoint";
import { LocalAiLifecycle } from "./LocalAiLifecycle";
import { LocalAiMachine } from "./LocalAiMachine";
import { LocalAiPower } from "./LocalAiPower";
import { LocalAiRoles } from "./LocalAiRoles";
import { LocalAiStart } from "./LocalAiStart";
import { recCardId, TUNING_ID, type LocalAiTarget } from "./locate";
import { runnerOf, shownPick, steps, type AssignPlan, type ReadinessInput } from "./readiness";
import { LocateProvider } from "./SectionLink";
import { usePull } from "./usePull";
import { useReleaseSettings } from "./useReleaseSettings";
import { useRoleTests } from "./useRoleTests";
import { useServerDetect } from "./useServerDetect";
import { TabResetSection } from "../settings/ResetControls";
import { ConfirmDialog } from "../ui";

/** The Local AI tab (#296): read this machine's hardware, size a curated model catalog against it,
 *  and turn on the local-endpoint provider (#297) — connect a local server, assign it to the chat /
 *  background roles, with cloud fallback. Self-contained and immediate-persist; errors surface inline.
 *  Frontend-only over existing backend commands, plus the one streaming Ollama pull.
 *
 *  This file is the tab, not the sections. It owns exactly what more than one section reads — the
 *  stored config, the live status, the served-model list, the hardware/catalog scan — and the
 *  reloads that refresh them — plus three jobs held here so that every control that starts or shows
 *  one reads the same state: the download (`usePull`), the role tests (`useRoleTests`) and the
 *  release settings (`useReleaseSettings`). Everything that belongs to one section lives with it,
 *  like the endpoint form. One file per section, rather than one screenful each of a 1,100-line
 *  function, which is what this was.
 *
 *  Errors are per section: each one is said in the section whose control failed, so a refused
 *  connect reads under the form that sent it rather than at the top of a long tab. And pointers
 *  between sections are names, not directions (`SectionLink`): `locate` opens whatever fold the
 *  target is in and scrolls there, through `onLocate` when the host has its own way to.
 *
 *  The first section, "Your local model", is a way through the rest: where the user stands, PM's
 *  pick, and four steps (`readiness.ts`). Its buttons make the same writes the sections do — they are
 *  wired here, beside the section controls that make them, so the two can't drift into two
 *  different things.
 *
 *  Last comes the "Reset to defaults" footer every settings tab has (#445). Whether the tab is at
 *  its defaults is the backend's answer, not one worked out here: the settings are fourteen stored
 *  rows owned by five modules, plus a keychain entry, and only the backend can say none of them is
 *  there. The answer is read again after every write the tab makes — every `set_*` command
 *  announces itself (`onSettingSaved`), and the three that aren't one (Disconnect, Forget token,
 *  accepting a licence) say so through `onSettingsWritten`. A reset then remounts the sections
 *  under a new key, so every section, hook and fold starts again from what is now stored — nothing
 *  can go on showing the server just forgotten — and the start card is back at step 1. */
export function LocalAiSettings({ onBetterFitChange, onLocate }: LocalAiSettingsProps = {}) {
  /** Bumped by a reset: the key the sections remount under. */
  const [epoch, setEpoch] = useState(0);
  // null until the backend has answered. Read as "at its defaults" meanwhile, the way the other tabs
  // offer no reset before their defaults load; an answer that FAILS reads as not, so a keychain PM
  // couldn't read never hides the reset.
  const [atDefaults, setAtDefaults] = useState<boolean | null>(null);
  // Writes land close together (a connect writes the address, then the token), and an older reply
  // landing last would put back an answer a later write has moved past. Only the newest one counts.
  const asked = useRef(0);
  const refreshDefaults = useCallback(() => {
    const n = ++asked.current;
    localAiSettingsAreDefault()
      .then((d) => {
        if (n === asked.current) setAtDefaults(d);
      })
      .catch(() => {
        if (n === asked.current) setAtDefaults(false);
      });
  }, []);
  useEffect(() => {
    refreshDefaults();
    return onSettingSaved(refreshDefaults);
  }, [refreshDefaults]);

  // After a reset, take the reader to where they now start. The remount re-lays the whole tab out
  // from its loading state, so wherever the scroll lands otherwise is an accident. In an effect, so
  // it runs once the NEW sections are in the DOM rather than against the ones being replaced.
  useEffect(() => {
    if (epoch === 0) return;
    const id = requestAnimationFrame(() => {
      const start = "sec-localai-start";
      if (onLocate) onLocate(start);
      else document.getElementById(start)?.scrollIntoView?.();
    });
    return () => cancelAnimationFrame(id);
    // Only on a reset: `onLocate` is a fresh function on each of the host's renders.
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the reset alone
  }, [epoch]);

  async function reset() {
    try {
      await resetLocalAiSettings();
    } catch (e) {
      // The backend's likely failures — a locked vault, a keychain that refuses — come before it
      // changes anything, so the tab stays as it is, and so does the reader's place beside the
      // error the footer says. Its answer is read again all the same, in case the store failed
      // after the token had gone.
      refreshDefaults();
      throw e;
    }
    setEpoch((n) => n + 1);
    refreshDefaults();
    // The update check is one of the settings, and the better-fit dot in the sidebar and the
    // settings rail follows it.
    onBetterFitChange?.();
  }

  return (
    <>
      <LocalAiTab
        key={epoch}
        onBetterFitChange={onBetterFitChange}
        onLocate={onLocate}
        onSettingsWritten={refreshDefaults}
      />
      <TabResetSection
        tabName="Local AI"
        isDefault={atDefaults ?? true}
        onReset={reset}
        confirmBody={
          <>
            Disconnects your model server and forgets its access key, and puts roles, On battery,
            Model memory, the extra folder, the update check and the licences you agreed to back to
            their defaults. Your models stay where they are — on your server and on this computer —
            and so do your cloud key, your chats and the tray icon.
          </>
        }
      />
    </>
  );
}

interface LocalAiSettingsProps {
  onBetterFitChange?: () => void;
  /** Scroll the host to the element with this id (Settings' own section jump), or omitted to
   *  scroll it into view directly. */
  onLocate?: (id: string) => void;
}

/** The tab's sections and the state they share — everything a reset starts again. */
function LocalAiTab({
  onBetterFitChange,
  onLocate,
  onSettingsWritten,
}: LocalAiSettingsProps & {
  /** A write landed that isn't a `set_*` command, so the reset footer's answer has to be re-read
   *  by hand: a Disconnect, a Forget token, or a licence accepted. */
  onSettingsWritten: () => void;
}) {
  const [recs, setRecs] = useState<LocalRecommendations | null>(null);
  const [betterFit, setBetterFit] = useState<LocalBetterFit | null>(null);
  const [loading, setLoading] = useState(true);
  const [rescanning, setRescanning] = useState(false);
  const [config, setConfig] = useState<LocalLlmConfig | null>(null);
  // The stored config couldn't be read on mount, so `config` is null for good rather than for now —
  // nothing reads it again until the tab remounts or a connect writes one.
  const [configError, setConfigError] = useState<string | null>(null);
  const [status, setStatus] = useState<LocalLlmStatus | null>(null);
  const [served, setServed] = useState<LocalServedModel[]>([]);
  // Whether `served` is an ANSWER rather than a starting value. It starts empty and is filled
  // asynchronously, and a listing that fails leaves it empty too — so "empty" alone cannot tell
  // "this server serves nothing" from "we haven't asked yet" or "we asked and couldn't reach it".
  // Only a resolved listing sets this, so copy that speaks for the empty case can never claim a
  // server serves nothing when PM simply doesn't know.
  const [servedLoaded, setServedLoaded] = useState(false);
  const [errors, setErrors] = useState<Partial<Record<ErrorSection, string>>>({});
  /** The "Settings PM's numbers assume" fold under Model server — held here so a pointer from
   *  another section can open it. */
  const [tuningOpen, setTuningOpen] = useState(false);
  /** The "Show all … models" fold under All models, held here for the same reason. */
  const [catalogOpen, setCatalogOpen] = useState(false);
  /** A role was written in this mount, and the endpoint hasn't changed since — what makes "send a
   *  test message" the next step rather than an optional one. */
  const [justAssigned, setJustAssigned] = useState(false);
  /** The start card's two writes in flight: the address a Connect is for, and a "Use … for both". */
  const [connecting, setConnecting] = useState<string | null>(null);
  const [assigning, setAssigning] = useState(false);
  /** Bumped whenever the stored endpoint or its token changes. The role tests reset on it, so
   *  anything proved about the OLD server — a passing test — goes with it rather than having to be
   *  remembered and invalidated piecemeal. */
  const [endpointEpoch, setEndpointEpoch] = useState(0);

  /** Say `message` in `section`, or clear that section's error with `null`. */
  const setError = useCallback((section: ErrorSection, message: string | null) => {
    setErrors((prev) => {
      if (message == null) {
        if (!(section in prev)) return prev;
        const next = { ...prev };
        delete next[section];
        return next;
      }
      return { ...prev, [section]: message };
    });
  }, []);

  /** The stored endpoint or its token changed: what was proved or chosen against the old one is
   *  stale. */
  function bumpEpoch() {
    setEndpointEpoch((n) => n + 1);
    setJustAssigned(false);
  }

  /** Take the reader to `target`: open the fold it sits in, then scroll there once it has rendered
   *  open. */
  function locate(target: LocalAiTarget) {
    let id: string = target;
    if (target === "tuning") {
      setTuningOpen(true);
      id = TUNING_ID;
    } else if (target === "catalog") {
      setCatalogOpen(true);
      id = "sec-localai-models";
    } else if (target.startsWith("rec:")) {
      setCatalogOpen(true);
      id = recCardId(target.slice("rec:".length));
    }
    requestAnimationFrame(() => {
      if (onLocate) onLocate(id);
      else document.getElementById(id)?.scrollIntoView?.();
    });
  }

  const configured = !!config?.base_url;
  // A role that reaches the local server AND has a model bound. Both halves are load-bearing.
  // Without the routing half the "not measured yet" line fires for someone entirely on cloud, who
  // has no local model to measure. Without the model half it fires for someone who flipped routing
  // to local and left the select on "— no local model —": `role_local_model` returns null for an
  // empty model, so the window would be null forever and the line would promise a reading that can
  // never arrive, because the gateway treats an absent model as unconfigured.
  const anyLocalRoleWithModel =
    (config?.chat_routing !== "cloud" && !!config?.chat_model?.trim()) ||
    (config?.background_routing !== "cloud" && !!config?.background_model?.trim());
  // Whether the connected endpoint is an Ollama server (the only runner with a one-click pull API).
  // By its port, parsed rather than matched (`runnerOf`). An Ollama on a custom port degrades
  // honestly to the copy-paste command; anything else on 11434 gets a button whose pull fails with
  // a clear error. A real flavour probe is the better gate if this ever grows another consumer.
  const isOllama = runnerOf(config?.base_url) === "Ollama";

  async function reloadConfig() {
    // Called after the server, its token or a role has changed — Disconnect and Forget token among
    // them, which aren't `set_*` commands and so announce nothing themselves. (After a download
    // too, where the second look is merely spare.)
    onSettingsWritten();
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

  const refreshRecs = useCallback(async () => {
    try {
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError("machine", String(e));
    }
  }, [setError]);

  const pick = shownPick(recs);
  const pull = usePull({
    recs,
    // Only ever a licence just accepted, which is one of the settings a reset clears.
    onRecs: (r) => {
      setRecs(r);
      onSettingsWritten();
    },
    onReload: reloadConfig,
    onRefreshRecs: refreshRecs,
    // Said where it was asked for. A download this view only adopted has no asker: PM's pick's own
    // download is the start card's (its progress is shown there), and any other is its card's.
    onError: (message, origin) => setError(origin === "start" ? "start" : "models", message),
    adoptedOrigin: (tag) => (pick?.kind === "catalogue" && pick.tag === tag ? "start" : "models"),
  });
  const roleTests = useRoleTests(endpointEpoch);
  const release = useReleaseSettings({ status });
  // Not before the stored config has been read, like the status poll below: `configured` is false
  // until it has, so a connected user's every visit to the tab sent a probe to three local ports
  // whose answer was then thrown away.
  const detect = useServerDetect(config === null ? null : configured);

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
        if (!cancelled) {
          setError("endpoint", String(e));
          setConfigError(String(e));
        }
      }
      try {
        const r = await localModelRecommendations();
        if (!cancelled) setRecs(r);
      } catch (e) {
        if (!cancelled) setError("machine", String(e));
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
  }, [setError]);

  // Poll the live status (the backend debounces the actual probe to once / 30s, so this can't hammer
  // the user's server). Always, not only once connected: where each job's requests really go is in
  // the status even with no server — a keyed user's jobs go to the cloud, a keyless one's nowhere —
  // and the start card says which. The served-model list rides the same tick once there is a server
  // to ask: the copy under Assign roles promises "download a model and it appears here", and before
  // this the list only refreshed on save/pull — a user following the copy-paste path while sitting on
  // the tab waited forever.
  //
  // Keyed on whether a server is stored, and not run before the stored config has been read: an
  // answer about "no server" must never stand in for one about the server just connected.
  const pollKey = config === null ? null : configured;
  useEffect(() => {
    if (pollKey === null) return;
    const withServer = pollKey;
    let cancelled = false;
    setStatus(null);
    // Replies can land out of order — a poll and a push fired close together — and an older one
    // landing last would put back a state a write has already moved past (the same "Use … for both"
    // button, just pressed). A later call reads later settings, so only the newest reply counts.
    let issued = 0;
    let applied = 0;
    const fetchStatus = () => {
      const n = ++issued;
      localLlmStatus()
        .then((s) => {
          if (!cancelled && n > applied) {
            applied = n;
            setStatus(s);
          }
        })
        .catch(() => {});
    };
    const tick = () => {
      fetchStatus();
      if (!withServer) return;
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
        fetchStatus();
      }),
    );
    return () => {
      cancelled = true;
      clearInterval(id);
      offStatus();
    };
  }, [pollKey]);

  // The pick and the "installed" rows are worked out against the stored server and what it serves,
  // so when either changes — connecting, disconnecting, a download landing, a model removed by hand —
  // they are read again. The server is part of the key, not only its list: the backend counts a file
  // on disk towards the pick only if the stored server could serve it, and reports what the server
  // holds only once there is one, so connecting to an EMPTY server changes the pick and the
  // "Already on this device" ladder while the list stays the same empty list it was before. The first
  // answer after mount is the baseline the initial read already matches.
  const recsKey = [config?.base_url ?? "", ...served.map((m) => m.id).sort()].join("\n");
  const recsBaseline = useRef<string | null>(null);
  const servedKnown = config !== null && (!configured || servedLoaded);
  useEffect(() => {
    if (!servedKnown) return;
    if (recsBaseline.current === null) {
      recsBaseline.current = recsKey;
      return;
    }
    if (recsBaseline.current === recsKey) return;
    recsBaseline.current = recsKey;
    void refreshRecs();
  }, [recsKey, servedKnown, refreshRecs]);

  /** Acknowledge the better-fit suggestion: it stays quiet until the cadence says to look again.
   *  Clears the dot here and, via the callback, in the sidebar and the settings nav. */
  async function dismissBetterFit() {
    setBetterFit(null);
    try {
      await dismissLocalBetterFit();
    } catch (e) {
      setError("start", String(e));
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
      setError("models", String(e));
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
    setError("downloaded", null);
    try {
      await setLocalModelScanDir(dir);
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError("downloaded", String(e));
    }
  }

  async function rescan() {
    setRescanning(true);
    setError("machine", null);
    try {
      await localHardwareScan(true);
      setRecs(await localModelRecommendations());
    } catch (e) {
      setError("machine", String(e));
    } finally {
      setRescanning(false);
    }
  }

  /** Connect to an address the probe found — the same write Model server's form makes. */
  async function connectTo(url: string) {
    setConnecting(url);
    setError("start", null);
    try {
      await setLocalLlmEndpoint(url);
      bumpEpoch();
      await reloadConfig();
    } catch (e) {
      setError("start", String(e));
    } finally {
      setConnecting(null);
    }
  }

  /** Put one model on both jobs — the writes Assign roles' selects make, model first, then the
   *  routing of each job the plan switches. Optimistic, like those selects. */
  async function assignBoth(plan: AssignPlan) {
    setAssigning(true);
    setError("start", null);
    const patch: Partial<LocalLlmConfig> = {};
    if (plan.models.chat != null) patch.chat_model = plan.models.chat;
    if (plan.models.background != null) patch.background_model = plan.models.background;
    if (plan.routing.chat) patch.chat_routing = plan.routing.chat;
    if (plan.routing.background) patch.background_routing = plan.routing.background;
    setConfig((c) => (c ? { ...c, ...patch } : c));
    roleTests.clearTest("chat");
    roleTests.clearTest("background");
    try {
      if (plan.models.chat != null) await setLocalLlmRoleModel("chat", plan.models.chat);
      if (plan.models.background != null)
        await setLocalLlmRoleModel("background", plan.models.background);
      for (const role of ["chat", "background"] as const) {
        const pref = plan.routing[role];
        if (pref) await setLocalLlmRouting(role, pref);
      }
      setJustAssigned(true);
    } catch (e) {
      setError("start", String(e));
      // Some of it may have been written: show what is really stored.
      await reloadConfig().catch(() => {});
    } finally {
      setAssigning(false);
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

  const prog = pull.pullProg;
  const input: ReadinessInput = {
    config,
    configError,
    status,
    served,
    servedLoaded,
    recs,
    recsLoading: loading,
    detected: detect.detected,
    detecting: detect.detecting,
    pull: {
      tag: pull.pulling,
      pct:
        prog && prog.total_bytes
          ? Math.min(100, Math.round((100 * (prog.completed_bytes ?? 0)) / prog.total_bytes))
          : null,
    },
    lastPulledTag: pull.lastPulledTag,
    tests: {
      running: roleTests.testing as LocalRole | null,
      chat: roleTests.tests.chat,
      background: roleTests.tests.background,
    },
    justAssigned,
    residency: release.residency,
  };
  const stepList = steps(input);
  // All models gives the pick's rung to the start card only while step 2 is offering exactly that
  // download — one Download per tag, so there is never a second button for the same file.
  const stepTwo = stepList[1];
  const pickDownloadTag = stepTwo.action?.kind === "download" ? stepTwo.action.tag : null;

  return (
    <LocateProvider locate={locate}>
      <LocalAiStart
        input={input}
        steps={stepList}
        betterFit={betterFit}
        onDismissBetterFit={() => void dismissBetterFit()}
        error={errors.start}
        pull={pull}
        actions={{
          connect: (url) => void connectTo(url),
          connecting,
          assign: (plan) => void assignBoth(plan),
          assigning,
          test: (role) => void roleTests.runTest(role),
          detect: () => void detect.detect(),
          detecting: detect.detecting,
          release: () => void release.release(),
          releasing: release.releasing,
          rescan: () => void rescan(),
          rescanning,
        }}
      />

      <LocalAiEndpoint
        config={config}
        status={status}
        configured={configured}
        onReload={reloadConfig}
        onError={(m) => setError("endpoint", m)}
        onEndpointChanged={bumpEpoch}
        error={errors.endpoint}
        tuningOpen={tuningOpen}
        onTuningOpenChange={setTuningOpen}
        detected={detect.detected}
        onDetect={detect.detect}
        guideInStart={stepList[0].setup !== null}
        pickContext={pick?.fit.context ?? null}
      />

      <LocalAiRoles
        config={config}
        status={status}
        served={served}
        servedLoaded={servedLoaded}
        configured={configured}
        coResidency={recs?.co_residency ?? null}
        anyLocalRoleWithModel={anyLocalRoleWithModel}
        roleTests={roleTests}
        onConfigPatch={(patch) => {
          setConfig((c) => (c ? { ...c, ...patch } : c));
          setJustAssigned(true);
        }}
        onError={(m) => setError("roles", m)}
        error={errors.roles}
      />

      {/* Between the two it depends on: it only ever moves a role Assign roles set to Local, fall
          back to cloud, and it hands the graphics card back on battery through the same release
          settings the next section stores. */}
      <LocalAiPower
        status={status}
        configured={configured}
        anyLocalRoleWithModel={anyLocalRoleWithModel}
        onError={(m) => setError("power", m)}
        release={release}
        error={errors.power}
      />

      <LocalAiLifecycle
        configured={configured}
        power={status?.power ?? null}
        release={release}
        // Until the scan has answered, assume the card: it is the wording every existing machine
        // reading has, and the shared-memory one is only true once PM knows there is no card.
        hasDiscreteGpu={
          recs ? recs.hardware.vram_gb != null && !recs.hardware.unified_memory : true
        }
        error={errors.lifecycle}
      />

      <LocalAiCatalog
        recs={recs}
        loading={loading}
        configured={configured}
        isOllama={isOllama}
        runner={runnerOf(config?.base_url)}
        servedTags={servedTags}
        installedRepos={installedRepos}
        pull={pull}
        pickDownloadTag={pickDownloadTag}
        pickProgressShown={stepTwo.progress}
        open={catalogOpen}
        onOpenChange={setCatalogOpen}
        onCadence={(c) => void changeCadence(c)}
        error={errors.models}
      />

      <LocalAiDownloaded
        recs={recs}
        loading={loading}
        configured={configured}
        baseUrl={config?.base_url ?? null}
        onPickFolder={() => void pickScanFolder()}
        onClearFolder={() => void clearScanFolder()}
        error={errors.downloaded}
      />

      <LocalAiMachine
        recs={recs}
        loading={loading}
        rescanning={rescanning}
        onRescan={() => void rescan()}
        error={errors.machine}
      />

      {/* The licence ask for a restricted model, answered before its download starts. Once, here,
          because the download it guards is the tab's: whichever control asked for it, there is
          only ever one question open. */}
      <ConfirmDialog
        open={pull.termsFor !== null}
        title={
          pull.termsFor
            ? `${pull.termsFor.rec.display_name} is under the ${pull.termsFor.rec.licence.name}`
            : ""
        }
        confirmLabel="Accept and download"
        onConfirm={() => void pull.acceptTermsAndPull()}
        onClose={pull.closeTerms}
      >
        {pull.termsFor && (
          <>
            <p>{pull.termsFor.rec.licence.summary}</p>
            <p className="mt-2">
              <a
                href={pull.termsFor.rec.licence.url}
                target="_blank"
                rel="noreferrer noopener"
                className="underline decoration-dotted underline-offset-2"
              >
                Read the full terms
              </a>
              .
            </p>
            <p className="mt-2 text-ink4">
              PM doesn't download the weights — your own Ollama fetches them from the publisher, and
              PM can't enforce these terms either way. Accepting here records that you've read them.
              PM won't ask again for another model under the same licence.
            </p>
          </>
        )}
      </ConfirmDialog>
    </LocateProvider>
  );
}

/** The sections an error can be said in. */
type ErrorSection =
  "start" | "endpoint" | "roles" | "power" | "lifecycle" | "models" | "downloaded" | "machine";
