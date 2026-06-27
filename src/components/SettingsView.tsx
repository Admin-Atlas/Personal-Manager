// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import {
  appLockStatus,
  costSummary,
  createShareableVault,
  exportAllData,
  getPref,
  getSettings,
  hasOpenRouterBackgroundKey,
  hasOpenRouterKey,
  installOptionalTsne,
  languageOptions,
  openDataFolder,
  optionalTsneStatus,
  refreshPricing,
  setPref,
  startSemanticLayout,
  setAppLock,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setIndexingSpeed,
  setOpenRouterBackgroundKey,
  setOpenRouterKey,
  setReranking,
  setTimeZone,
  setVaultEmbedder,
} from "../lib/ipc";
import { useHelp } from "../lib/help";
import { ConnectorsSettings } from "./ConnectorsSettings";
import { ModelListEditor } from "./ModelListEditor";
import { ModelRecommendationCards } from "./ModelRecommendationCards";
import { RebuildProgress } from "./RebuildProgress";
import { VaultCard } from "./VaultCard";
import type { AppLockStatus, CostSummary, LanguageOptions } from "../lib/types";
import { isDevBuild, useDevMode } from "../lib/capabilities";
import { useTheme, useDepth, ACCENTS } from "../theme";
import { Button, Collapsible, ConfirmDialog, Input, NavItem, SegmentedControl, Select } from "./ui";

interface Props {
  onClose: () => void;
  /** First-run onboarding requires a key before the app is usable. */
  onboarding: boolean;
  /** Jump to the Dev tab (issue #78) — closes Settings and navigates. Non-onboarding only. */
  onOpenDev?: () => void;
}

/** The non-onboarding Settings tabs (left rail). Onboarding stays a single untabbed scroll. */
type SettingsTab = "general" | "ai" | "search" | "connectors" | "data" | "developer";

const SETTINGS_TABS: ReadonlyArray<{ id: SettingsTab; label: string }> = [
  { id: "general", label: "General" },
  { id: "ai", label: "AI & Models" },
  { id: "search", label: "Search" },
  { id: "connectors", label: "Connectors" },
  { id: "data", label: "Data & Security" },
  { id: "developer", label: "Developer" },
];

export function SettingsView({ onClose, onboarding, onOpenDev }: Props) {
  const help = useHelp();
  const {
    system,
    setSystem,
    mode,
    setMode,
    depth,
    setDepth,
    accent,
    setAccent,
    teachVisible,
    setTeachVisible,
  } = useTheme();
  const { devMode, setDevMode } = useDevMode();
  const { showMeta, showPower } = useDepth();
  const [key, setKey] = useState("");
  const [bgKey, setBgKey] = useState("");
  const [chatModels, setChatModelsState] = useState<string[]>([]);
  const [backgroundModels, setBackgroundModelsState] = useState<string[]>([]);
  const [chatAuto, setChatAuto] = useState(false);
  const [backgroundAuto, setBackgroundAuto] = useState(false);
  const [keyAlreadySet, setKeyAlreadySet] = useState(false);
  const [bgKeyAlreadySet, setBgKeyAlreadySet] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [timeZone, setTimeZoneState] = useState("");
  const [tzAuto, setTzAuto] = useState(true);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [refreshingPrices, setRefreshingPrices] = useState(false);
  const [appLock, setAppLockState] = useState<AppLockStatus | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  // First-run vault choice (onboarding only). Default device-only = today's zero-friction
  // path; "shareable" derives the key from a passphrase so the vault can be opened from
  // other profiles. Applied once, after the API key is saved.
  const [vaultMode, setVaultMode] = useState<"device" | "shareable">("device");
  const [vaultPass, setVaultPass] = useState("");
  const [vaultConfirm, setVaultConfirm] = useState("");
  // Query-time reranking toggle (default on; stateless — never triggers a Rebuild).
  const [reranking, setRerankingState] = useState(true);
  // Indexing speed: "fast" (default) or "gentle" (paced for low-end machines).
  const [indexingSpeed, setIndexingSpeedState] = useState<"fast" | "gentle">("fast");
  // Memory map (the Map tab): the default grouping (per-device, shared with the Map header toggle via
  // localStorage), the node cap (a vault-travelling pref the backend reads), and the optional t-SNE
  // component's install state.
  const [mapGrouping, setMapGrouping] = useState<"semantic" | "project">(() =>
    localStorage.getItem("pm.map.layoutMode") === "semantic" ? "semantic" : "project",
  );
  const [mapNodeCap, setMapNodeCap] = useState(1000);
  const [tsneInstalled, setTsneInstalled] = useState<boolean | null>(null);
  const [installingTsne, setInstallingTsne] = useState(false);
  // Search-language choices: the selectable embedders + the chosen id (onboarding picks one;
  // non-onboarding switches it, re-indexing the vault). Loaded best-effort.
  const [langOpts, setLangOpts] = useState<LanguageOptions | null>(null);
  const [embedderId, setEmbedderId] = useState("");
  // Settings language switcher (non-onboarding): the pending confirm target, the in-flight switch
  // (drives the guided re-index modal: { to, from }), and any error from the switch itself.
  const [switchTarget, setSwitchTarget] = useState<string | null>(null);
  const [switching, setSwitching] = useState<{ to: string; from: string } | null>(null);
  const [rebuildOpen, setRebuildOpen] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  // Active tab (non-onboarding only). The scrolling content pane is reset to the top on a
  // tab change so each tab opens from its first section.
  const [tab, setTab] = useState<SettingsTab>("general");
  const contentRef = useRef<HTMLDivElement>(null);

  function selectTab(next: SettingsTab) {
    setTab(next);
    contentRef.current?.scrollTo({ top: 0 });
  }

  useEffect(() => {
    (async () => {
      try {
        setKeyAlreadySet(await hasOpenRouterKey());
        setBgKeyAlreadySet(await hasOpenRouterBackgroundKey());
        const settings = await getSettings();
        setChatModelsState(settings.chat_models);
        setBackgroundModelsState(settings.background_models);
        setChatAuto(settings.chat_auto_switch);
        setBackgroundAuto(settings.background_auto_switch);
        if (settings.time_zone) {
          setTimeZoneState(settings.time_zone);
          setTzAuto(settings.time_zone === detectTimeZone());
        } else {
          // First launch: detect the device zone and persist it so the backend's
          // "today"/agenda reasoning has a zone from the start.
          const detected = detectTimeZone();
          setTimeZoneState(detected);
          setTzAuto(true);
          // Best-effort: an exotic zone chrono-tz doesn't recognise stays unsaved and
          // the backend reasons in UTC — don't let it block the rest of the load.
          try {
            await setTimeZone(detected);
          } catch {
            /* ignore — UTC fallback in the backend */
          }
        }
        setRerankingState(settings.reranking);
        setIndexingSpeedState(settings.indexing_speed === "gentle" ? "gentle" : "fast");
      } catch (e) {
        setError(String(e));
      }
      // Search-language options are best-effort: a failure just hides the picker.
      try {
        const lo = await languageOptions();
        setLangOpts(lo);
        setEmbedderId(lo.selected);
      } catch {
        /* ignore — the picker / language line simply won't show */
      }
    })();
  }, [onboarding]);

  // Cost summary loads on its own (its first read may trigger a daily pricing fetch),
  // so it never blocks the rest of Settings from showing.
  useEffect(() => {
    if (onboarding) return;
    costSummary()
      .then(setCost)
      .catch(() => {});
  }, [onboarding]);

  // App-lock status (enabled + whether the OS can verify) loads independently.
  useEffect(() => {
    if (onboarding) return;
    appLockStatus()
      .then(setAppLockState)
      .catch(() => {});
  }, [onboarding]);

  // Memory-map prefs + optional-t-SNE install state (non-onboarding only — there's no Map yet at setup).
  useEffect(() => {
    if (onboarding) return;
    getPref("map")
      .then((v) => {
        if (!v) return;
        try {
          const cap = JSON.parse(v)?.nodeCap;
          if (typeof cap === "number") setMapNodeCap(cap);
        } catch {
          /* ignore a malformed pref */
        }
      })
      .catch(() => {});
    optionalTsneStatus()
      .then((s) => setTsneInstalled(s.installed))
      .catch(() => setTsneInstalled(false));
  }, [onboarding]);

  function changeMapGrouping(next: "semantic" | "project") {
    setMapGrouping(next);
    localStorage.setItem("pm.map.layoutMode", next); // shared with the Map header toggle
  }

  function changeMapNodeCap(next: number) {
    setMapNodeCap(next);
    // The cap is part of the layout fingerprint; recompute in the background so the change takes hold.
    void setPref("map", JSON.stringify({ nodeCap: next }))
      .then(() => startSemanticLayout())
      .catch(() => {});
  }

  function downloadTsne() {
    setInstallingTsne(true);
    installOptionalTsne()
      .then(() => optionalTsneStatus())
      .then((s) => setTsneInstalled(s.installed))
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingTsne(false));
  }

  async function toggleAppLock(next: boolean) {
    setError(null);
    try {
      await setAppLock(next);
      setAppLockState((s) => (s ? { ...s, enabled: next } : s));
    } catch (e) {
      setError(String(e));
    }
  }

  async function toggleReranking(next: boolean) {
    setError(null);
    setRerankingState(next); // optimistic — revert if the write fails
    try {
      await setReranking(next);
    } catch (e) {
      setRerankingState(!next);
      setError(String(e));
    }
  }

  async function changeIndexingSpeed(next: "fast" | "gentle") {
    setError(null);
    const prev = indexingSpeed;
    setIndexingSpeedState(next); // optimistic — revert if the write fails
    try {
      await setIndexingSpeed(next);
    } catch (e) {
      setIndexingSpeedState(prev);
      setError(String(e));
    }
  }

  // Re-sync the language picker with the backend's truth (after a switch lands, or reverts).
  async function reloadLang() {
    try {
      const lo = await languageOptions();
      setLangOpts(lo);
      setEmbedderId(lo.selected);
    } catch {
      /* ignore — the picker simply keeps its last state */
    }
  }

  // A click on the *other* segment in Settings: stage the target and open the confirm. The picker's
  // value stays on the current selection until the switch actually lands, so a cancel snaps back.
  function requestLanguageSwitch(newId: string) {
    if (!langOpts || newId === embedderId) return;
    setSwitchError(null);
    setSwitchTarget(newId);
  }

  // Confirmed: record the new embedder. An empty vault is done immediately (the backend resized its
  // empty vector table); a populated vault launches the guided re-index, remembering the old id so
  // a download/offline failure can revert the selection (search keeps working on the old index).
  async function confirmLanguageSwitch() {
    if (!langOpts || !switchTarget) return;
    const to = switchTarget;
    const from = embedderId;
    setSwitchTarget(null);
    setSwitchError(null);
    try {
      await setVaultEmbedder(to);
    } catch (e) {
      setSwitchError(String(e));
      return;
    }
    if (langOpts.has_documents) {
      setSwitching({ to, from });
      setRebuildOpen(true);
    } else {
      setEmbedderId(to);
      await reloadLang();
    }
  }

  async function refreshPrices() {
    setRefreshingPrices(true);
    setError(null);
    try {
      setCost(await refreshPricing());
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshingPrices(false);
    }
  }

  async function revealDataFolder() {
    setError(null);
    try {
      await openDataFolder();
    } catch (e) {
      setError(String(e));
    }
  }

  async function exportData() {
    setError(null);
    setExportMsg(null);
    let dest: string | null;
    try {
      dest = await saveFileDialog({
        defaultPath: "personal-manager-export.zip",
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
    } catch (e) {
      setError(String(e));
      return;
    }
    if (!dest) return; // the user cancelled the dialog
    setExporting(true);
    try {
      await exportAllData(dest);
      setExportMsg(`Exported to ${dest}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setExporting(false);
    }
  }

  // A shareable first-run vault needs a passphrase that matches its confirmation.
  const vaultChoiceValid =
    !onboarding ||
    vaultMode === "device" ||
    (vaultPass.trim().length > 0 && vaultPass === vaultConfirm);
  const canSave = !saving && (keyAlreadySet || key.trim().length > 0) && vaultChoiceValid;

  async function save() {
    setSaving(true);
    setError(null);
    try {
      if (key.trim()) {
        await setOpenRouterKey(key.trim());
        setKey("");
        setKeyAlreadySet(true);
      }
      if (bgKey.trim()) {
        await setOpenRouterBackgroundKey(bgKey.trim());
        setBgKey("");
        setBgKeyAlreadySet(true);
      }
      // Don't persist an empty list — before the settings load resolves (or on a
      // fresh install) that would overwrite the backend's defaults with nothing.
      if (chatModels.length > 0) await setChatModels(chatModels);
      if (backgroundModels.length > 0) await setBackgroundModels(backgroundModels);
      await setChatAutoSwitch(chatAuto);
      await setBackgroundAutoSwitch(backgroundAuto);
      await setTimeZone(tzAuto ? detectTimeZone() : timeZone);
      // First-run: record the chosen search language on the still-empty vault (only if the user
      // changed it from the default). Must happen before any documents exist.
      if (onboarding && langOpts && embedderId && embedderId !== langOpts.selected) {
        await setVaultEmbedder(embedderId);
      }
      // First-run: if the user opted into a shareable vault, convert the fresh (empty)
      // device vault now that the key is saved. Device-only needs nothing — it's default.
      if (onboarding && vaultMode === "shareable") {
        await createShareableVault(vaultPass.trim());
      }
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  // ── Onboarding: a single linear first-run wizard (no tabs). ───────────────────────────────
  if (onboarding) {
    return (
      <div className="flex h-full items-center justify-center p-6">
        <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-[var(--radius)] border border-border bg-panel p-6 shadow-xl">
          <div className="flex items-center gap-2">
            <h1 className="font-head text-lg font-semibold text-ink">Welcome to PM</h1>
            <span
              title="PM is in alpha — under active development; expect rough edges and changes between updates."
              className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-wide text-accent-text"
            >
              Alpha
            </span>
          </div>
          <p className="mt-1 text-sm text-ink3">
            PM is a private, local-first assistant — your documents, notes, and chats live in an
            encrypted store on this device. Two quick things to set up below: an AI provider key,
            and how your vault is protected.
          </p>

          <div className="mt-5 border-t border-border pt-4">
            <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
              AI provider
            </label>
            <p className="mt-1 text-xs leading-relaxed text-ink4">
              PM reaches AI models through{" "}
              <a
                href="https://openrouter.ai"
                target="_blank"
                rel="noreferrer"
                className="text-accent-text underline hover:brightness-110"
              >
                OpenRouter
              </a>{" "}
              — one account for OpenAI, Anthropic, Google, and free models. It's free to start, and
              PM sends Zero-Data-Retention on every request.
            </p>
          </div>

          <label className="mt-3 block text-sm font-medium text-ink2">OpenRouter API key</label>
          <Input
            type="password"
            autoComplete="off"
            data-help="settings-api-key"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={keyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
            className="mt-1"
          />
          <a
            href="https://openrouter.ai/keys"
            target="_blank"
            rel="noreferrer"
            className="mt-1 inline-block text-xs text-ink4 hover:text-ink2"
          >
            Get a key at openrouter.ai/keys →
          </a>

          <div className="mt-5 space-y-5 border-t border-border pt-4">
            <ModelListEditor
              label="Chat model"
              description="Answers your chats. Add several and turn on auto-switch to fall back when one runs out."
              helpId="settings-chat-models"
              models={chatModels}
              onChange={setChatModelsState}
              autoSwitch={chatAuto}
              onAutoSwitchChange={setChatAuto}
            />
            <ModelListEditor
              label="Background model"
              description="Runs sorting proposals and Learning You. Free models work well here; chain a few for daily limits."
              helpId="settings-background-models"
              models={backgroundModels}
              onChange={setBackgroundModelsState}
              autoSwitch={backgroundAuto}
              onAutoSwitchChange={setBackgroundAuto}
            />
          </div>

          <div className="mt-5 border-t border-border pt-4">
            <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
              Your vault
            </label>
            <p className="mt-1 text-xs text-ink4">
              Your documents and notes live in one encrypted store. Choose how it's protected — you
              can change this anytime in Settings.
            </p>
            <div className="mt-3">
              <SegmentedControl
                value={vaultMode}
                onChange={setVaultMode}
                options={[
                  { value: "device", label: "This device only" },
                  { value: "shareable", label: "Shareable" },
                ]}
              />
            </div>
            <p className="mt-2 text-xs text-faint">
              {vaultMode === "device"
                ? "Recommended. The key stays in this device's keychain — zero friction, nothing to remember."
                : "Protected by a passphrase you choose, so the same vault can be opened from another Windows account (and your Markdown is encrypted at rest). The passphrase can't be recovered — if you forget it, the vault can't be opened."}
            </p>
            {vaultMode === "shareable" && (
              <div className="mt-3 space-y-2">
                <Input
                  type="password"
                  autoComplete="new-password"
                  placeholder="Passphrase"
                  value={vaultPass}
                  onChange={(e) => setVaultPass(e.target.value)}
                />
                <Input
                  type="password"
                  autoComplete="new-password"
                  placeholder="Confirm passphrase"
                  value={vaultConfirm}
                  onChange={(e) => setVaultConfirm(e.target.value)}
                />
                {vaultPass.length > 0 && vaultPass !== vaultConfirm && (
                  <p className="text-xs text-st-due">Passphrases don't match.</p>
                )}
              </div>
            )}
          </div>

          {langOpts && langOpts.options.length > 1 && (
            <div className="mt-5 border-t border-border pt-4">
              <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                Search language
              </label>
              <p className="mt-1 text-xs text-ink4">
                How your library is searched. Pick the one that matches your content — not basic vs
                advanced, just which fits.
              </p>
              <div className="mt-3">
                <SegmentedControl
                  value={embedderId}
                  onChange={setEmbedderId}
                  options={langOpts.options.map((o) => ({ value: o.id, label: o.label }))}
                />
              </div>
              <p className="mt-2 text-xs text-ink4">
                {langOpts.options.find((o) => o.id === embedderId)?.multilingual
                  ? "Best for libraries with real non-English content. Understands 100+ languages and finds meaning across them — not just matching words. Downloads a larger model the first time (about 1 GB, once), and uses a little more disk and time per search."
                  : "Best for libraries that are mostly English. Works straight away, stays small and fast. Files in other languages can still be found by keyword."}
              </p>
              <div className="mt-3">
                <Collapsible title="Compare" defaultOpen={false}>
                  <LanguageCompareTable />
                </Collapsible>
              </div>
              <p className="mt-3 text-xs text-faint">
                You can switch a vault&apos;s language later in Settings — it re-indexes your
                library to do so (quick on a small vault, longer on a large one). Your original
                files are never touched or lost.
              </p>
            </div>
          )}

          {error && (
            <p
              className="mt-3 rounded-[var(--radius)] px-3 py-2 text-sm text-st-due"
              style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
            >
              {error}
            </p>
          )}

          <div className="mt-6 flex justify-end gap-2">
            <Button variant="primary" onClick={save} disabled={!canSave}>
              {saving ? "Saving…" : "Get started"}
            </Button>
          </div>
        </div>
      </div>
    );
  }

  // ── Settings: a left-rail, tabbed surface. State + Save are shared across all tabs. ──────────
  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="flex max-h-[90vh] w-full max-w-3xl flex-col overflow-hidden rounded-[var(--radius)] border border-border bg-panel shadow-xl">
        <div className="shrink-0 border-b border-border px-6 py-4">
          <h1 className="font-head text-lg font-semibold text-ink">Settings</h1>
          <p className="mt-1 text-sm text-ink3">
            Your API key lives in the OS keychain. The model is swappable anytime.
          </p>
        </div>

        <div className="flex min-h-0 flex-1">
          <nav className="w-44 shrink-0 overflow-y-auto border-r border-border p-3">
            <div className="flex flex-col gap-1">
              {SETTINGS_TABS.map((t) => (
                <NavItem key={t.id} active={tab === t.id} onClick={() => selectTab(t.id)}>
                  {t.label}
                </NavItem>
              ))}
            </div>
          </nav>

          <div
            ref={contentRef}
            className="min-w-0 flex-1 overflow-y-auto px-6 py-4 [&>*:first-child]:mt-0 [&>*:first-child]:border-t-0 [&>*:first-child]:pt-0"
          >
            {tab === "general" && (
              <>
                <div className="mt-5 border-t border-border pt-4" data-help="settings-appearance">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Appearance
                  </label>
                  <p className="mt-1 text-xs text-ink4">
                    Applies instantly and is remembered on this device.
                  </p>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">System</span>
                    <SegmentedControl
                      value={system}
                      onChange={setSystem}
                      options={[
                        { value: "editorial", label: "Editorial" },
                        { value: "slate", label: "Slate" },
                        { value: "terminal", label: "Terminal" },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Mode</span>
                    <SegmentedControl
                      value={mode}
                      onChange={setMode}
                      options={[
                        { value: "dark", label: "Dark" },
                        { value: "light", label: "Light" },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Depth</span>
                    <SegmentedControl
                      value={depth}
                      onChange={setDepth}
                      options={[
                        { value: "min", label: "Min" },
                        { value: "standard", label: "Standard" },
                        { value: "power", label: "Power" },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Accent</span>
                    <div className="flex items-center gap-1.5">
                      {ACCENTS[system].map((hex) => (
                        <button
                          key={hex}
                          type="button"
                          aria-label={`Accent ${hex}`}
                          onClick={() => setAccent(hex)}
                          style={{ background: hex }}
                          className={`h-5 w-5 rounded-full transition ${
                            accent === hex
                              ? "ring-2 ring-ink ring-offset-2 ring-offset-[var(--surface)]"
                              : ""
                          }`}
                        />
                      ))}
                    </div>
                  </div>
                  <div
                    className="mt-3 flex items-center justify-between gap-3"
                    data-help="settings-teach-tab"
                  >
                    <span className="text-sm text-ink2">Review &amp; Teach tabs</span>
                    <button
                      type="button"
                      role="switch"
                      aria-checked={teachVisible}
                      aria-label="Show the Review and Teach tabs"
                      onClick={() => setTeachVisible(!teachVisible)}
                      className={`inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                        teachVisible ? "bg-accent" : "bg-surface"
                      }`}
                    >
                      <span
                        className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                          teachVisible ? "translate-x-4" : "translate-x-0.5"
                        }`}
                      />
                    </button>
                  </div>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-memory-map">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Memory map
                  </label>
                  <p className="mt-1 text-xs text-ink4">
                    The Map tab — how documents are arranged and how many are plotted.
                  </p>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Default grouping</span>
                    <SegmentedControl
                      value={mapGrouping}
                      onChange={changeMapGrouping}
                      options={[
                        { value: "semantic", label: "Semantic" },
                        { value: "project", label: "By project" },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Maximum nodes</span>
                    <Select
                      value={String(mapNodeCap)}
                      onChange={(e) => changeMapNodeCap(Number(e.target.value))}
                    >
                      {[200, 500, 1000, 2000, 3500, 5000].map((n) => (
                        <option key={n} value={n}>
                          {n.toLocaleString()}
                        </option>
                      ))}
                    </Select>
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Enhanced layout (t-SNE)</span>
                    {tsneInstalled === null ? (
                      <span className="text-xs text-ink4">…</span>
                    ) : tsneInstalled ? (
                      <span className="text-xs text-ink3">Installed</span>
                    ) : (
                      <Button variant="secondary" onClick={downloadTsne} disabled={installingTsne}>
                        {installingTsne ? "Downloading…" : "Download"}
                      </Button>
                    )}
                  </div>
                  <p className="mt-2 text-xs text-ink4">
                    Semantic proximity uses a basic on-device layout by default. The optional t-SNE
                    component (a one-time download) produces tighter clusters of related documents.
                  </p>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-timezone">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Time zone
                  </label>
                  <p className="mt-1 text-xs text-ink4">
                    Sets “today”, “due soon”, and your calendar agenda. Auto follows this device.
                  </p>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Detection</span>
                    <SegmentedControl
                      value={tzAuto ? "auto" : "manual"}
                      onChange={(v) => setTzAuto(v === "auto")}
                      options={[
                        { value: "auto", label: "Auto" },
                        { value: "manual", label: "Manual" },
                      ]}
                    />
                  </div>
                  {!tzAuto && (
                    <div className="mt-3 flex items-center justify-between gap-3">
                      <span className="text-sm text-ink2">Zone</span>
                      <Select
                        value={timeZone}
                        onChange={(e) => setTimeZoneState(e.target.value)}
                        className="max-w-[14rem]"
                      >
                        {allTimeZones().map((z) => (
                          <option key={z} value={z}>
                            {z}
                          </option>
                        ))}
                      </Select>
                    </div>
                  )}
                  <p className="mt-2 text-xs text-faint">
                    {tzAuto
                      ? `Following this device: ${detectTimeZone()}`
                      : `Selected: ${timeZone || "—"}`}
                  </p>
                </div>

                <div
                  className="mt-4 flex items-start justify-between gap-3 border-t border-border pt-4"
                  data-help="settings-help-mode"
                >
                  <div>
                    <label className="block text-sm font-medium text-ink2">Help mode</label>
                    <p className="mt-1 text-xs text-ink4">
                      When on, hovering any highlighted section shows a short explanation of what it
                      does.
                    </p>
                  </div>
                  <button
                    role="switch"
                    aria-checked={help.enabled}
                    onClick={() => help.setEnabled(!help.enabled)}
                    className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                      help.enabled ? "bg-accent" : "bg-surface"
                    }`}
                  >
                    <span
                      className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                        help.enabled ? "translate-x-4" : "translate-x-0.5"
                      }`}
                    />
                  </button>
                </div>
              </>
            )}

            {tab === "ai" && (
              <>
                <label className="mt-5 block text-sm font-medium text-ink2">
                  OpenRouter API key
                </label>
                <Input
                  type="password"
                  autoComplete="off"
                  data-help="settings-api-key"
                  value={key}
                  onChange={(e) => setKey(e.target.value)}
                  placeholder={keyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
                  className="mt-1"
                />
                <a
                  href="https://openrouter.ai/keys"
                  target="_blank"
                  rel="noreferrer"
                  className="mt-1 inline-block text-xs text-ink4 hover:text-ink2"
                >
                  Get a key at openrouter.ai/keys →
                </a>

                <label className="mt-4 block text-sm font-medium text-ink2">
                  Background API key
                </label>
                <Input
                  type="password"
                  autoComplete="off"
                  data-help="settings-background-key"
                  value={bgKey}
                  onChange={(e) => setBgKey(e.target.value)}
                  placeholder={bgKeyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
                  className="mt-1"
                />
                <p className="mt-1 text-xs text-ink4">
                  Used for background work (sorting proposals, learning). Lets you track that spend
                  separately. Falls back to your main key if blank.
                </p>

                <div className="mt-5 space-y-5 border-t border-border pt-4">
                  <ModelListEditor
                    label="Chat model"
                    description="Answers your chats. Add several and turn on auto-switch to fall back when one runs out."
                    helpId="settings-chat-models"
                    models={chatModels}
                    onChange={setChatModelsState}
                    autoSwitch={chatAuto}
                    onAutoSwitchChange={setChatAuto}
                  />
                  <ModelListEditor
                    label="Background model"
                    description="Runs sorting proposals and Learning You. Free models work well here; chain a few for daily limits."
                    helpId="settings-background-models"
                    models={backgroundModels}
                    onChange={setBackgroundModelsState}
                    autoSwitch={backgroundAuto}
                    onAutoSwitchChange={setBackgroundAuto}
                  />
                  <ModelRecommendationCards
                    showMeta={showMeta}
                    showPower={showPower}
                    defaultExpanded={showPower}
                    onUseForChat={(m) =>
                      setChatModelsState((prev) => [m, ...prev.filter((x) => x !== m)].slice(0, 50))
                    }
                    onUseForBackground={(m) =>
                      setBackgroundModelsState((prev) =>
                        [m, ...prev.filter((x) => x !== m)].slice(0, 50),
                      )
                    }
                  />
                </div>

                {cost && (
                  <div className="mt-5 border-t border-border pt-4" data-help="settings-usage-cost">
                    <div className="flex items-center justify-between">
                      <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                        Usage &amp; cost
                      </label>
                      <Button
                        variant="tertiary"
                        onClick={refreshPrices}
                        disabled={refreshingPrices}
                        className="px-2 py-0.5 text-xs"
                      >
                        {refreshingPrices ? "Refreshing…" : "Refresh prices"}
                      </Button>
                    </div>
                    <div className="mt-2">
                      <Collapsible
                        title="Spend & breakdown"
                        defaultOpen={showPower}
                        meta={`${fmtUsd(cost.total_30d_usd)} · 30d`}
                      >
                        <p className="pt-2 text-xs text-ink4">
                          Your real per-call cost as reported by OpenRouter where available,
                          otherwise estimated from the tokens each call used × OpenRouter&apos;s
                          per-token price
                          {cost.pricing_updated_at
                            ? ` (prices updated ${formatWhen(cost.pricing_updated_at)})`
                            : ""}
                          .
                        </p>
                        <div className="mt-2 flex gap-6 text-sm">
                          <div>
                            <div className="text-xs text-ink4">Last 30 days</div>
                            <div className="font-mono text-ink2">{fmtUsd(cost.total_30d_usd)}</div>
                          </div>
                          <div>
                            <div className="text-xs text-ink4">All time</div>
                            <div className="font-mono text-ink2">
                              {fmtUsd(cost.total_all_time_usd)}
                            </div>
                          </div>
                        </div>
                        <div className="space-y-2 pt-2 text-xs leading-relaxed text-ink3">
                          <p>
                            Each model reply reports the tokens it used (your prompt + its reply).
                            PM logs those per call. It fetches OpenRouter&apos;s public price list
                            about once a day and caches it — no extra model call, and your API key
                            is never used for it.
                          </p>
                          <p>
                            Where OpenRouter reports a call&apos;s actual cost (reflecting any
                            prompt-cache discount) PM shows that; for older calls without it, cost =
                            prompt&nbsp;tokens × prompt&nbsp;price + reply&nbsp;tokens ×
                            reply&nbsp;price. It&apos;s computed when you open this page, so a later
                            price change re-prices your history. A model with no reported cost and
                            not yet in the price cache shows{" "}
                            <span className="font-mono text-ink4">—</span>, never an
                            understated&nbsp;$0.
                          </p>
                        </div>
                        {cost.all_time.length > 0 ? (
                          <div className="mt-3">
                            <p className="pb-1 font-mono text-[10px] uppercase tracking-wide text-ink4">
                              By model · most expensive first (all time)
                            </p>
                            <table className="w-full text-left text-xs">
                              <thead className="font-mono uppercase tracking-wide text-ink4">
                                <tr className="border-b border-rule">
                                  <th className="py-1 font-medium">Model</th>
                                  <th className="py-1 text-right font-medium">Reqs</th>
                                  <th className="py-1 text-right font-medium">Tokens in/out</th>
                                  <th className="py-1 text-right font-medium">Cost</th>
                                </tr>
                              </thead>
                              <tbody>
                                {cost.all_time.map((s) => (
                                  <tr key={s.model} className="border-b border-rule">
                                    <td className="py-1 pr-2 text-ink2">{s.model}</td>
                                    <td className="py-1 text-right text-ink3">{s.request_count}</td>
                                    <td className="py-1 text-right font-mono text-ink4">
                                      {s.prompt_tokens.toLocaleString()} /{" "}
                                      {s.completion_tokens.toLocaleString()}
                                    </td>
                                    <td className="py-1 text-right font-mono text-ink3">
                                      {fmtUsd(s.cost_usd)}
                                    </td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          </div>
                        ) : (
                          <p className="mt-2 text-xs text-ink4">No model calls logged yet.</p>
                        )}
                      </Collapsible>
                    </div>
                  </div>
                )}
              </>
            )}

            {tab === "search" && (
              <>
                <div className="mt-4 border-t border-border pt-4">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Search
                  </label>
                  {langOpts && langOpts.options.length > 1 && (
                    <div className="mt-2">
                      <div className="flex items-center justify-between gap-3">
                        <p className="text-xs text-ink4">
                          Language:{" "}
                          <span className="text-ink2">
                            {langOpts.options.find((o) => o.id === embedderId)?.label ?? "English"}
                          </span>
                        </p>
                        <SegmentedControl
                          value={embedderId}
                          onChange={requestLanguageSwitch}
                          options={langOpts.options.map((o) => ({ value: o.id, label: o.label }))}
                        />
                      </div>
                      <p className="mt-1 text-xs text-faint">
                        Switching re-indexes your whole library from your Markdown files —
                        Multilingual downloads a larger model the first time (about 1 GB, once).
                        Your original files are never touched.
                      </p>
                      {switchError && <p className="mt-1 text-xs text-st-due">{switchError}</p>}
                    </div>
                  )}
                  <div className="mt-3 flex items-start justify-between gap-3">
                    <div>
                      <label className="block text-sm font-medium text-ink2">
                        Re-rank search results
                      </label>
                      <p className="mt-1 text-xs text-ink4">
                        A second pass re-scores search hits for sharper relevance. First use
                        downloads a small model; turn off for fastest results.
                      </p>
                    </div>
                    <button
                      type="button"
                      role="switch"
                      aria-checked={reranking}
                      aria-label="Re-rank search results"
                      onClick={() => void toggleReranking(!reranking)}
                      className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                        reranking ? "bg-accent" : "bg-surface"
                      }`}
                    >
                      <span
                        className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                          reranking ? "translate-x-4" : "translate-x-0.5"
                        }`}
                      />
                    </button>
                  </div>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-learning">
                  <label className="block text-sm font-medium text-ink2">Preferences</label>
                  <p className="mt-1 text-xs text-ink4">
                    What PM has learned about how you work — what belongs where, how things are
                    named, how answers should read — now lives in the{" "}
                    <span className="text-ink2">Teach</span> tab as editable preferences. Your
                    earlier “Learning&nbsp;You” profile was carried over into them automatically.
                  </p>
                </div>
              </>
            )}

            {tab === "connectors" && (
              <>
                <ConnectorsSettings
                  indexingSpeed={indexingSpeed}
                  onChangeIndexingSpeed={(s) => void changeIndexingSpeed(s)}
                />
              </>
            )}

            {tab === "data" && (
              <>
                <div
                  className="mt-4 flex items-start justify-between gap-3 border-t border-border pt-4"
                  data-help="settings-app-lock"
                >
                  <div>
                    <label className="block text-sm font-medium text-ink2">App lock</label>
                    <p className="mt-1 text-xs text-ink4">
                      {appLock?.available
                        ? "Require Windows Hello (face, fingerprint, or PIN) to open PM. A convenience lock for the window — your store is always encrypted at rest. Takes effect next time you open PM."
                        : "Requires Windows Hello or a configured biometric. Not available on this device yet."}
                    </p>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={appLock?.enabled ?? false}
                    aria-label="App lock"
                    disabled={!appLock?.available}
                    onClick={() => void toggleAppLock(!(appLock?.enabled ?? false))}
                    className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
                      appLock?.enabled ? "bg-accent" : "bg-surface"
                    }`}
                  >
                    <span
                      className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                        appLock?.enabled ? "translate-x-4" : "translate-x-0.5"
                      }`}
                    />
                  </button>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-data">
                  <label className="block text-sm font-medium text-ink2">Data</label>
                  <div className="mt-2 flex flex-wrap gap-2">
                    <Button variant="tertiary" onClick={revealDataFolder}>
                      Open data folder
                    </Button>
                    <Button variant="tertiary" onClick={exportData} disabled={exporting}>
                      {exporting ? "Exporting…" : "Export all data…"}
                    </Button>
                  </div>
                  {exportMsg && <p className="mt-2 break-all text-xs text-faint">{exportMsg}</p>}
                  <div className="mt-3">
                    <Collapsible title="About your data & export" defaultOpen={showPower}>
                      <p className="pt-2 text-xs text-ink4">
                        Your documents and the encrypted store live in one folder (
                        <span className="font-medium">Personal Manager</span>). Open it to back it
                        up by hand, or export everything to a single{" "}
                        <span className="font-medium">.zip</span> — the Markdown vault plus the
                        encrypted store (the regenerable runtime is left out). The store stays
                        encrypted in the archive.
                      </p>
                      <p className="mt-1 text-xs text-ink4">
                        Your documents in the Markdown vault are stored unencrypted so any tool can
                        read them. To protect them when your machine is off or logged out, turn on
                        full-disk encryption (BitLocker on Windows, FileVault on macOS).
                      </p>
                    </Collapsible>
                  </div>
                </div>

                <VaultCard />

                <div className="mt-5 border-t border-border pt-4" data-help="settings-license">
                  <Collapsible title="License" defaultOpen={showPower}>
                    <div className="pt-2 text-xs leading-relaxed text-ink4">
                      <p>
                        PM is free software, licensed under the{" "}
                        <a
                          href="https://www.gnu.org/licenses/agpl-3.0.html"
                          target="_blank"
                          rel="noreferrer"
                          className="text-ink3 underline hover:text-ink"
                        >
                          GNU Affero General Public License v3
                        </a>
                        . © 2026 Bobby Yu.
                      </p>
                      <p className="mt-1">
                        Source code:{" "}
                        <a
                          href="https://github.com/Admin-Atlas/Personal-Manager"
                          target="_blank"
                          rel="noreferrer"
                          className="text-ink3 underline hover:text-ink"
                        >
                          github.com/Admin-Atlas/Personal-Manager
                        </a>
                      </p>
                    </div>
                  </Collapsible>
                </div>
              </>
            )}

            {tab === "developer" && (
              <div className="mt-5 border-t border-border pt-4" data-help="settings-developer">
                <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                  Developer mode
                </label>
                <p className="mt-1 text-xs text-ink4">
                  Reveals read-only inspection surfaces — a dedicated Dev tab (raw tables, row
                  counts, the corrections log, system &amp; build info) plus internals shown in
                  place — for debugging and watching how PM works. Strictly read-only: it never
                  changes your data. Independent of the density preset, and off by default.
                </p>
                <div className="mt-3 flex items-center justify-between gap-3">
                  <span className="text-sm text-ink2">Developer mode</span>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={devMode}
                    aria-label="Developer mode"
                    onClick={() => setDevMode(!devMode)}
                    className={`inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                      devMode ? "bg-accent" : "bg-surface"
                    }`}
                  >
                    <span
                      className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                        devMode ? "translate-x-4" : "translate-x-0.5"
                      }`}
                    />
                  </button>
                </div>
                <div className="mt-3 flex items-center justify-between gap-3 text-xs">
                  <span className="text-ink3">Signals</span>
                  <span className="font-mono text-ink4">
                    build: {isDevBuild ? "dev" : "release"} · runtime: {devMode ? "on" : "off"}
                  </span>
                </div>
                {devMode && onOpenDev && (
                  <div className="mt-3">
                    <Button variant="tertiary" onClick={onOpenDev}>
                      Open Dev tab →
                    </Button>
                  </div>
                )}
              </div>
            )}
          </div>
        </div>

        <div className="shrink-0 border-t border-border px-6 py-4">
          {error && (
            <p
              className="mb-3 rounded-[var(--radius)] px-3 py-2 text-sm text-st-due"
              style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
            >
              {error}
            </p>
          )}
          <div className="flex justify-end gap-2">
            <Button variant="tertiary" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" onClick={save} disabled={!canSave}>
              {saving ? "Saving…" : "Save"}
            </Button>
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={switchTarget !== null}
        title="Switch search language?"
        confirmLabel="Switch & re-index"
        onConfirm={() => void confirmLanguageSwitch()}
        onClose={() => setSwitchTarget(null)}
      >
        This re-indexes your whole library from your Markdown files
        {langOpts?.options.find((o) => o.id === switchTarget)?.multilingual
          ? ", and downloads a larger language model the first time (about 1 GB, once)"
          : ""}
        . Your original files aren&apos;t changed, and it can take a while on a large library.
      </ConfirmDialog>

      {switching && (
        <RebuildProgress
          open={rebuildOpen}
          title="Switching search language"
          subtitle={`Re-indexing your library for ${
            langOpts?.options.find((o) => o.id === switching.to)?.label ?? "the new language"
          }.`}
          onError={() => {
            // The re-index failed (e.g. offline): revert the selection so search keeps working on
            // the existing index.
            void setVaultEmbedder(switching.from).catch(() => {});
          }}
          onClose={() => {
            setRebuildOpen(false);
            setSwitching(null);
            void reloadLang();
          }}
        />
      )}
    </div>
  );
}

/** The English-vs-Multilingual comparison shown under the onboarding picker's "Compare" expander.
 *  Framed as "which fits your content", never better/worse. */
function LanguageCompareTable() {
  const rows: Array<[string, string, string]> = [
    ["Best for", "Mostly-English libraries", "Real non-English content"],
    ["Languages", "English", "100+ languages"],
    ["Finds meaning in other languages?", "No — keyword matches only", "Yes — understands meaning"],
    ["First-time download", "None — built in", "~1 GB, once (then cached)"],
    ["Disk per vault", "Smallest", "Larger (~2.7× the search data)"],
    ["Speed", "Fastest", "A little slower per search"],
    ["Model", "bge-small-en", "multilingual-e5-large"],
  ];
  return (
    <table className="mt-1 w-full text-left text-xs">
      <thead className="font-mono uppercase tracking-wide text-ink4">
        <tr className="border-b border-rule">
          <th className="py-1 font-medium" />
          <th className="py-1 font-medium">English</th>
          <th className="py-1 font-medium">Multilingual</th>
        </tr>
      </thead>
      <tbody>
        {rows.map(([label, en, ml]) => (
          <tr key={label} className="border-b border-rule align-top">
            <td className="py-1 pr-2 text-ink4">{label}</td>
            <td className="py-1 pr-2 text-ink2">{en}</td>
            <td className="py-1 text-ink2">{ml}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** A friendly "when" for the learning-profile timestamp; falls back to the raw value. */
function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

/** Format a USD cost, or "—" when unknown (the model isn't in the price cache yet). */
function fmtUsd(v: number | null): string {
  if (v == null) return "—";
  if (v === 0) return "$0.00";
  return v < 0.01 ? `$${v.toFixed(4)}` : `$${v.toFixed(2)}`;
}

/** The device's IANA time zone (e.g. "Europe/London"), via the Intl API. */
function detectTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

/** Every IANA zone the runtime knows, for the manual picker. Falls back to just the
 *  detected zone on a runtime without `Intl.supportedValuesOf`. */
function allTimeZones(): string[] {
  const intl = Intl as typeof Intl & { supportedValuesOf?: (key: string) => string[] };
  return typeof intl.supportedValuesOf === "function"
    ? intl.supportedValuesOf("timeZone")
    : [detectTimeZone()];
}
