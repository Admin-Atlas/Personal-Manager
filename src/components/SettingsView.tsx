// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import {
  getPref,
  getSettings,
  hasOpenRouterKey,
  installOptionalTsne,
  languageOptions,
  onTsneInstall,
  optionalTsneStatus,
  setPref,
  startSemanticLayout,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setIndexingSpeed,
  setOpenRouterKey,
  setTimeZone,
  setVaultEmbedder,
  vaultStatus,
} from "../lib/ipc";
import { useHelp } from "../lib/help";
import { BackupSettings } from "./BackupSettings";
import { ConnectorsSettings } from "./ConnectorsSettings";
import { AiModelsSettings } from "./settings/AiModelsSettings";
import { DataSecuritySettings } from "./settings/DataSecuritySettings";
import { DeveloperSettings } from "./settings/DeveloperSettings";
import { SearchSettings } from "./settings/SearchSettings";
import { ModelListEditor } from "./ModelListEditor";
import { IngestProgress } from "./IngestProgress";
import { StorageSettings } from "./StorageSettings";
import type { LanguageOptions } from "../lib/types";
import {
  MAP_COHESION_KEY,
  MAP_MODE_KEY,
  readMapCohesion,
  readMapMode,
  type MapLayoutMode,
} from "../lib/mapPrefs";
import { readConfirmDelete, writeConfirmDelete } from "../lib/pinboard/prefs";
import {
  useTheme,
  ACCENTS,
  accentName,
  MONO_ACCENT,
  EIGENGRAU,
  deviceCoords,
  coordsForTimezone,
  formatCoords,
  deviceTimeZone,
  allTimeZones,
} from "../theme";
import {
  Button,
  Collapsible,
  Input,
  NavItem,
  SectionInfo,
  SegmentedControl,
  Select,
  Toggle,
} from "./ui";

interface Props {
  onClose: () => void;
  /** First-run onboarding requires a key before the app is usable. */
  onboarding: boolean;
  /** Jump to the Dev tab (issue #78) — closes Settings and navigates. Non-onboarding only. */
  onOpenDev?: () => void;
}

/** The non-onboarding Settings tabs (left rail). Onboarding stays a single untabbed scroll. */
type SettingsTab =
  "general" | "ai" | "search" | "connectors" | "data" | "backup" | "storage" | "developer";

const SETTINGS_TABS: ReadonlyArray<{ id: SettingsTab; label: string }> = [
  { id: "general", label: "General" },
  { id: "ai", label: "AI & Models" },
  { id: "search", label: "Search" },
  { id: "connectors", label: "Connectors" },
  { id: "data", label: "Data & Security" },
  { id: "backup", label: "Backup" },
  { id: "storage", label: "Storage" },
  { id: "developer", label: "Developer" },
];

export function SettingsView({ onClose, onboarding, onOpenDev }: Props) {
  const help = useHelp();
  const {
    system,
    setSystem,
    mode,
    modePref,
    setModePref,
    modeSource,
    modeCoords,
    modeNextChange,
    autoLocation,
    setAutoLocation,
    depth,
    setDepth,
    accent,
    setAccent,
    teachVisible,
    setTeachVisible,
  } = useTheme();
  // Seeded from localStorage rather than watched: the toggle is the only writer here, and the
  // Pinboard reads the pref fresh at the moment you click delete (see pinboard/prefs.ts).
  const [confirmDelete, setConfirmDelete] = useState(readConfirmDelete);
  const [key, setKey] = useState("");
  const [chatModels, setChatModelsState] = useState<string[]>([]);
  const [backgroundModels, setBackgroundModelsState] = useState<string[]>([]);
  // True once getSettings() has populated the model lists. Save gates on this (not a non-empty
  // list) so deliberately clearing a role to "use the default" persists, while a save that races
  // ahead of the initial load still can't clobber the backend defaults with the empty init state.
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [chatAuto, setChatAuto] = useState(false);
  const [backgroundAuto, setBackgroundAuto] = useState(false);
  const [keyAlreadySet, setKeyAlreadySet] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [timeZone, setTimeZoneState] = useState("");
  const [tzAuto, setTzAuto] = useState(true);
  // Onboarding on a JOINED shared vault (issue #337): the vault already exists and its search
  // language travels with it, so the language chooser is replaced by a short "what's yours alone"
  // checklist and `set_vault_embedder` is suppressed. (First-run always creates a device vault;
  // sharing happens later through the guided wizard.)
  const [joinedVault, setJoinedVault] = useState(false);
  // Indexing speed: "fast" (default) or "gentle" (paced for low-end machines).
  const [indexingSpeed, setIndexingSpeedState] = useState<"fast" | "gentle">("fast");
  // Memory map (the Map tab): the default grouping (per-device, shared with the Map header toggle via
  // localStorage), the node cap (a vault-travelling pref the backend reads), and the optional t-SNE
  // component's install state.
  const [mapGrouping, setMapGrouping] = useState<MapLayoutMode>(readMapMode);
  const [mapNodeCap, setMapNodeCap] = useState(1000);
  // Project cohesion (0 = pure meaning, the default; ≤0.5) — a render-time blend, so it lives in
  // localStorage like the grouping pref, not in the backend layout fingerprint.
  const [mapCohesion, setMapCohesion] = useState<number>(readMapCohesion);
  const [tsneInstalled, setTsneInstalled] = useState<boolean | null>(null);
  // Whether to *use* t-SNE when it's installed (vs falling back to PCA). Default true; lives in the
  // `map` pref alongside nodeCap, so the backend reads it for the layout fingerprint.
  const [mapTsneEnabled, setMapTsneEnabled] = useState(true);
  const [installingTsne, setInstallingTsne] = useState(false);
  const [tsneInstallFrac, setTsneInstallFrac] = useState(0);
  // Search-language choices: the selectable embedders + the chosen id (onboarding picks one;
  // non-onboarding switches it, re-indexing the vault). Loaded best-effort.
  const [langOpts, setLangOpts] = useState<LanguageOptions | null>(null);
  const [embedderId, setEmbedderId] = useState("");
  // Active tab (non-onboarding only). The scrolling content pane is reset to the top on a
  // tab change so each tab opens from its first section.
  const [tab, setTab] = useState<SettingsTab>("general");
  const contentRef = useRef<HTMLDivElement>(null);

  function selectTab(next: SettingsTab) {
    setTab(next);
    contentRef.current?.scrollTo({ top: 0 });
  }

  useEffect(() => {
    void (async () => {
      try {
        setKeyAlreadySet(await hasOpenRouterKey());
        const settings = await getSettings();
        setChatModelsState(settings.chat_models);
        setBackgroundModelsState(settings.background_models);
        setChatAuto(settings.chat_auto_switch);
        setBackgroundAuto(settings.background_auto_switch);
        setSettingsLoaded(true); // model lists now reflect the backend — clearing them can persist

        if (settings.time_zone) {
          setTimeZoneState(settings.time_zone);
          setTzAuto(settings.time_zone === deviceTimeZone());
        } else {
          // First launch: detect the device zone and persist it so the backend's
          // "today"/agenda reasoning has a zone from the start.
          const detected = deviceTimeZone();
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
      // Onboarding after joining a shared vault: the vault is already passphrase-mode,
      // so the vault/language choosers don't apply (best-effort — a status failure just
      // shows the standard onboarding).
      if (onboarding) {
        try {
          const vs = await vaultStatus();
          setJoinedVault(vs.mode === "passphrase");
        } catch {
          /* ignore — standard onboarding */
        }
      }
    })();
  }, [onboarding]);

  // Memory-map prefs + optional-t-SNE install state (non-onboarding only — there's no Map yet at setup).
  useEffect(() => {
    if (onboarding) return;
    getPref("map")
      .then((v) => {
        if (!v) return;
        try {
          const pref = JSON.parse(v);
          if (typeof pref?.nodeCap === "number") setMapNodeCap(pref.nodeCap);
          if (typeof pref?.tsneEnabled === "boolean") setMapTsneEnabled(pref.tsneEnabled);
        } catch {
          /* ignore a malformed pref */
        }
      })
      .catch(() => {});
    optionalTsneStatus()
      .then((s) => setTsneInstalled(s.installed))
      .catch(() => setTsneInstalled(false));
  }, [onboarding]);

  // Follow the optional-t-SNE download's progress so the row shows a real percentage bar.
  useEffect(() => {
    if (onboarding) return;
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void onTsneInstall((e) => {
      if (!cancelled) setTsneInstallFrac(e.fraction);
    }).then((u) => {
      unlisten = u;
      if (cancelled) u();
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [onboarding]);

  function changeMapGrouping(next: MapLayoutMode) {
    setMapGrouping(next);
    localStorage.setItem(MAP_MODE_KEY, next); // shared with the Map header toggle
  }

  function changeMapCohesion(next: number) {
    setMapCohesion(next);
    localStorage.setItem(MAP_COHESION_KEY, String(next)); // shared with the Map's cohesion control
  }

  // The `map` pref is one blob (`{ nodeCap, tsneEnabled }`), both part of the layout fingerprint —
  // write it whole (so neither key is lost) and recompute in the background so the change takes hold.
  function persistMapPref(nodeCap: number, tsneEnabled: boolean) {
    return setPref("map", JSON.stringify({ nodeCap, tsneEnabled }))
      .then(() => startSemanticLayout())
      .catch(() => {});
  }

  function changeMapNodeCap(next: number) {
    setMapNodeCap(next);
    void persistMapPref(next, mapTsneEnabled);
  }

  function changeTsneEnabled(next: boolean) {
    setMapTsneEnabled(next);
    void persistMapPref(mapNodeCap, next);
  }

  function downloadTsne() {
    setInstallingTsne(true);
    setTsneInstallFrac(0);
    installOptionalTsne()
      .then(() => optionalTsneStatus())
      .then((s) => {
        setTsneInstalled(s.installed);
        if (s.installed) {
          setMapTsneEnabled(true); // a fresh install starts enabled
          void persistMapPref(mapNodeCap, true);
        }
      })
      .catch((e) => setError(String(e)))
      .finally(() => setInstallingTsne(false));
  }

  // Time zone saves immediately, like everything else in Settings. Auto persists the device zone;
  // Manual persists the selected zone. The shared time/location context re-reads on the event.
  function changeTz(auto: boolean, zone: string) {
    setTzAuto(auto);
    setTimeZoneState(zone);
    void setTimeZone(auto ? deviceTimeZone() : zone)
      .then(() => window.dispatchEvent(new Event("pm:settings-changed")))
      .catch((e) => setError(String(e)));
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

  const canSave = !saving && (keyAlreadySet || key.trim().length > 0);

  async function save() {
    setSaving(true);
    setError(null);
    try {
      if (key.trim()) {
        await setOpenRouterKey(key.trim());
        setKey("");
        setKeyAlreadySet(true);
      }
      // Persist the model lists only once they've loaded from the backend — gating on the load
      // flag (not a non-empty list) lets the user clear a role back to "use the default" and have
      // it stick, while still refusing to overwrite the defaults with the empty pre-load state.
      if (settingsLoaded) {
        await setChatModels(chatModels);
        await setBackgroundModels(backgroundModels);
      }
      await setChatAutoSwitch(chatAuto);
      await setBackgroundAutoSwitch(backgroundAuto);
      await setTimeZone(tzAuto ? deviceTimeZone() : timeZone);
      // Let the shared time/location context re-read the zone at once (it also re-reads on refocus).
      window.dispatchEvent(new Event("pm:settings-changed"));
      // First-run: record the chosen search language on the still-empty vault (only if the user
      // changed it from the default). Must happen before any documents exist. A JOINED vault
      // already has both a language and a passphrase — its choosers aren't shown, and neither
      // call may run against it (issue #337).
      if (
        onboarding &&
        !joinedVault &&
        langOpts &&
        embedderId &&
        embedderId !== langOpts.selected
      ) {
        await setVaultEmbedder(embedderId);
      }
      // First-run always creates a device vault (the recommended default). Sharing with other
      // Windows accounts is set up afterwards via the guided wizard, which moves the vault to a
      // reachable folder — so onboarding can never strand a "shareable" vault inside this
      // profile's private folder, unreachable by every other account (the issue #337 trap).
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

          {joinedVault ? (
            <div className="mt-5 border-t border-border pt-4">
              <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                Your vault
              </label>
              <p className="mt-1 text-xs text-ink4">
                You're connected to the shared vault — its documents, chats, and projects are
                already here, and its search language travels with it.
              </p>
              <p className="mt-2 text-xs text-ink4">Two things are yours alone on this account:</p>
              <ul className="ml-4 mt-1 list-disc space-y-1 text-xs text-ink4">
                <li>
                  Your own <strong>OpenRouter key</strong> (above) — AI features wake up once it's
                  saved. Keys never travel between Windows accounts.
                </li>
                <li>
                  Your own <strong>sign-ins</strong> — reconnect Drive, OneDrive, and calendars
                  under Settings → Connectors when you're ready.
                </li>
              </ul>
            </div>
          ) : (
            <div className="mt-5 border-t border-border pt-4">
              <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                Your vault
              </label>
              <p className="mt-1 text-xs text-ink4">
                Your documents and notes live in one encrypted store on this device — the key stays
                in this device's keychain, so there's nothing to remember.
              </p>
              <p className="mt-2 text-xs text-faint">
                Want to open the same vault from another Windows account on this PC? Set that up
                anytime after setup under Settings → Data &amp; Security → Share with other
                accounts.
              </p>
            </div>
          )}

          {!joinedVault && langOpts && langOpts.options.length > 1 && (
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
        {/* The window header carries the title only. Its old subtitle ("your API key lives in the
            OS keychain…") was AI-tab material shown on every tab; it now sits in that tab's
            "About your API keys" disclosure, next to the keys it describes. */}
        <div className="shrink-0 border-b border-border px-6 py-4">
          <h1 className="font-head text-lg font-semibold text-ink">Settings</h1>
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
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">System</span>
                    <SegmentedControl
                      value={system}
                      onChange={setSystem}
                      options={[
                        {
                          value: "editorial",
                          label: "Editorial",
                          title: "Editorial — serif headings, warm paper tones",
                        },
                        {
                          value: "slate",
                          label: "Slate",
                          title: "Slate — clean sans, cool neutrals (default)",
                        },
                        {
                          value: "terminal",
                          label: "Terminal",
                          title: "Terminal — monospace, high contrast",
                        },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Mode</span>
                    <SegmentedControl
                      value={modePref}
                      onChange={setModePref}
                      options={[
                        { value: "light", label: "Light" },
                        { value: "dark", label: "Dark" },
                        {
                          value: "system",
                          label: "System",
                          title: "Follow your device's light/dark setting",
                        },
                        {
                          value: "auto",
                          label: "Auto",
                          title: "Follow sunrise and sunset at your location",
                        },
                      ]}
                    />
                  </div>
                  {modePref === "system" && (
                    <p className="mt-1.5 text-xs text-ink4">
                      Following your device's light/dark setting — currently{" "}
                      {mode === "dark" ? "dark" : "light"}.
                    </p>
                  )}
                  {modePref === "auto" && (
                    <>
                      <p className="mt-1.5 text-xs text-ink4">
                        {modeSource === "auto" ? (
                          <>
                            Follows sunrise &amp; sunset — currently{" "}
                            {mode === "dark" ? "dark" : "light"}
                            {modeCoords ? ` · ${formatCoords(modeCoords)}` : ""}
                            {modeNextChange
                              ? ` · switches to ${mode === "dark" ? "light" : "dark"} at ${modeNextChange.toLocaleTimeString(
                                  [],
                                  { hour: "2-digit", minute: "2-digit" },
                                )}`
                              : ""}
                            .
                          </>
                        ) : (
                          <>
                            Couldn't determine your location, so it's following your device's
                            light/dark setting for now. Enter a location below for sunrise &amp;
                            sunset.
                          </>
                        )}
                      </p>
                      <div className="mt-2 flex items-center justify-between gap-3">
                        <span className="text-sm text-ink2">Location</span>
                        <div className="flex items-center gap-2">
                          <Input
                            value={autoLocation}
                            onChange={(e) => setAutoLocation(e.target.value)}
                            placeholder={
                              deviceCoords()
                                ? `${formatCoords(deviceCoords()!)} (detected)`
                                : "e.g. 51.51, -0.13"
                            }
                            aria-label="Location for sunrise and sunset, as latitude, longitude"
                            className="w-44"
                          />
                          {autoLocation && (
                            <button
                              type="button"
                              className="text-xs text-ink4 transition hover:text-ink"
                              onClick={() => setAutoLocation("")}
                            >
                              Reset
                            </button>
                          )}
                        </div>
                      </div>
                    </>
                  )}
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
                      {ACCENTS[system].map((hex) => {
                        const isMono = hex === MONO_ACCENT;
                        const name = accentName(hex);
                        return (
                          <button
                            key={hex}
                            type="button"
                            aria-label={isMono ? "Monochrome (Eigengrau)" : `Accent: ${name}`}
                            title={
                              isMono ? "Monochrome — Eigengrau base, white text & accents" : name
                            }
                            onClick={() => setAccent(hex)}
                            style={{
                              background: isMono ? EIGENGRAU : hex,
                              // The Eigengrau swatch is near-black; a white rim makes it legible and
                              // signals the "white accents" treatment.
                              border: isMono ? "1px solid rgba(255,255,255,0.55)" : undefined,
                            }}
                            className={`h-5 w-5 rounded-full transition ${
                              accent === hex
                                ? "ring-2 ring-ink ring-offset-2 ring-offset-[var(--surface)]"
                                : ""
                            }`}
                          />
                        );
                      })}
                    </div>
                  </div>
                  <div
                    className="mt-3 flex items-center justify-between gap-3"
                    data-help="settings-pinboard-confirm-delete"
                  >
                    <span className="text-sm text-ink2">
                      Confirm before deleting a pinboard card
                    </span>
                    <Toggle
                      checked={confirmDelete}
                      onChange={(on) => {
                        setConfirmDelete(on);
                        writeConfirmDelete(on);
                      }}
                      ariaLabel="Ask before deleting a note or timeline"
                    />
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
                  {/* Both of Appearance's paragraphs — the "it saves itself" reassurance and the
                      Location field's format + privacy note — fold into this one disclosure at the
                      foot. The auto-mode status line and its "couldn't find your location" fallback
                      stay inline above: they're a readout and a gating hint, not explanation. */}
                  <SectionInfo title="What these settings do">
                    <p>Applies instantly and is remembered on this device.</p>
                    {modePref === "auto" && (
                      <p>
                        Location is a latitude, longitude pair. Blank uses your device's timezone
                        {(() => {
                          try {
                            const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
                            return tz && coordsForTimezone(tz) ? ` (${tz})` : "";
                          } catch {
                            return "";
                          }
                        })()}
                        . Nothing about your location leaves this device.
                      </p>
                    )}
                  </SectionInfo>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-memory-map">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Memory map
                  </label>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Default grouping</span>
                    <SegmentedControl
                      value={mapGrouping}
                      onChange={changeMapGrouping}
                      options={[
                        { value: "project", label: "By project" },
                        { value: "semantic", label: "Semantic" },
                      ]}
                    />
                  </div>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Project cohesion</span>
                    <Select
                      value={String(mapCohesion)}
                      onChange={(e) => changeMapCohesion(Number(e.target.value))}
                    >
                      <option value="0">Off</option>
                      <option value="0.15">Low</option>
                      <option value="0.3">Medium</option>
                      <option value="0.5">High</option>
                    </Select>
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
                      <button
                        type="button"
                        role="switch"
                        aria-checked={mapTsneEnabled}
                        aria-label="Use the enhanced t-SNE layout"
                        onClick={() => changeTsneEnabled(!mapTsneEnabled)}
                        className={`inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                          mapTsneEnabled ? "bg-accent" : "bg-surface"
                        }`}
                      >
                        <span
                          className={`inline-block h-4 w-4 transform rounded-full bg-accent-ink transition-transform ${
                            mapTsneEnabled ? "translate-x-4" : "translate-x-0.5"
                          }`}
                        />
                      </button>
                    ) : (
                      <Button variant="secondary" onClick={downloadTsne} disabled={installingTsne}>
                        {installingTsne ? "Downloading…" : "Download"}
                      </Button>
                    )}
                  </div>
                  {installingTsne && (
                    <IngestProgress
                      mode="percent"
                      processed={Math.round(tsneInstallFrac * 100)}
                      total={100}
                      label="Downloading the enhanced (t-SNE) layout"
                      className="mt-2"
                    />
                  )}
                  {/* The section's opening blurb and its trailing rationale said one thing between
                      them — how the map is laid out — so they're one disclosure now. */}
                  <SectionInfo title="How the map is arranged">
                    <p>The Map tab — how documents are arranged and how many are plotted.</p>
                    <p>
                      Semantic proximity uses a basic on-device layout by default. Project cohesion
                      gently pulls same-project documents together (Off keeps the layout purely by
                      meaning). The optional t-SNE component (a one-time download) produces tighter
                      clusters of related documents — turn it on or off here, or remove it to free
                      space under Settings → Storage.
                    </p>
                  </SectionInfo>
                </div>

                <div className="mt-5 border-t border-border pt-4" data-help="settings-timezone">
                  <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
                    Time zone
                  </label>
                  <div className="mt-3 flex items-center justify-between gap-3">
                    <span className="text-sm text-ink2">Detection</span>
                    <SegmentedControl
                      value={tzAuto ? "auto" : "manual"}
                      onChange={(v) => changeTz(v === "auto", timeZone)}
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
                        onChange={(e) => changeTz(false, e.target.value)}
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
                  {/* The zone in force is a readout — it stays visible; only what the zone *means*
                      folds away. */}
                  <p className="mt-2 text-xs text-faint">
                    {tzAuto
                      ? `Following this device: ${deviceTimeZone()}`
                      : `Selected: ${timeZone || "—"}`}
                  </p>
                  <SectionInfo title="What the time zone affects">
                    <p>
                      Sets “today”, “due soon”, and your calendar agenda. Auto follows this device.
                    </p>
                  </SectionInfo>
                </div>

                <div className="mt-4 border-t border-border pt-4" data-help="settings-help-mode">
                  <div className="flex items-start justify-between gap-3">
                    <label className="block text-sm font-medium text-ink2">Help mode</label>
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
                  <SectionInfo title="What is help mode?">
                    <p>
                      When on, hovering any highlighted section shows a short explanation of what it
                      does.
                    </p>
                  </SectionInfo>
                </div>
              </>
            )}

            {tab === "ai" && <AiModelsSettings />}

            {tab === "search" && <SearchSettings />}

            {tab === "connectors" && (
              <>
                <ConnectorsSettings
                  indexingSpeed={indexingSpeed}
                  onChangeIndexingSpeed={(s) => void changeIndexingSpeed(s)}
                />
              </>
            )}

            {tab === "data" && <DataSecuritySettings />}

            {tab === "backup" && <BackupSettings />}

            {tab === "storage" && (
              <StorageSettings onNavigate={(t) => selectTab(t as SettingsTab)} />
            )}

            {tab === "developer" && <DeveloperSettings onOpenDev={onOpenDev} />}
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
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      </div>
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
