// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, useState } from "react";
import {
  getSettings,
  hasOpenRouterKey,
  languageOptions,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setIndexingSpeed,
  setOnboardingDone,
  setOpenRouterKey,
  setTimeZone,
  setVaultEmbedder,
  vaultStatus,
} from "../lib/ipc";
import { BackupSettings } from "./BackupSettings";
import { AttentionDot } from "./Sidebar";
import { ConnectorsSettings } from "./ConnectorsSettings";
import { LocalAiSettings } from "./localai/LocalAiSettings";
import { AiModelsSettings } from "./settings/AiModelsSettings";
import { DataSecuritySettings } from "./settings/DataSecuritySettings";
import { DeveloperSettings } from "./settings/DeveloperSettings";
import { GeneralSettings } from "./settings/GeneralSettings";
import { AccessibilitySettings } from "./settings/AccessibilitySettings";
import { SearchSettings } from "./settings/SearchSettings";
import { ModelListEditor } from "./ModelListEditor";
import { OnboardingLocalConnect } from "./onboarding/OnboardingLocalConnect";
import { StorageSettings } from "./StorageSettings";
import type { LanguageOptions } from "../lib/types";
import { useScrollSpy } from "../lib/useScrollSpy";
import { deviceTimeZone, scrollBehavior } from "../theme";
import { SETTINGS_GROUPS, sectionsFor, type SettingsTab } from "./settings/registry";
import { useSettingsPending } from "../lib/settingsPending";
import { SavedTick } from "./settings/SavedTick";
import { Button, cn, Collapsible, ConfirmDialog, Input, NavItem, SegmentedControl } from "./ui";

interface Props {
  onClose: () => void;
  /** First-run onboarding requires a key before the app is usable. */
  onboarding: boolean;
  /** Jump to the Dev tab (issue #78) — closes Settings and navigates. Non-onboarding only. */
  onOpenDev?: () => void;
  /** Jump to the Teach tab — closes Settings and navigates (after an AI-memory import). */
  onOpenTeach?: () => void;
  /** A better-fitting local model is available (#437) — marks the Local AI tab row. */
  betterFit?: boolean;
  /** A connected Google account predates the Sheets permission — marks the Connectors tab row. */
  sheetsNudge?: boolean;
  /** Re-read that suggestion after the Local AI tab acts on or dismisses it, so the dots clear
   *  without waiting for the next navigation. */
  onBetterFitChange?: () => void;
}

export function SettingsView({
  onClose,
  onboarding,
  onOpenDev,
  onOpenTeach,
  betterFit,
  sheetsNudge,
  onBetterFitChange,
}: Props) {
  const [key, setKey] = useState("");
  // First-run AI-provider choice (#295): a cloud key, or a local model on this device. The local
  // pane reports readiness (an endpoint + a chat model configured) up through `localReady`.
  const [aiMode, setAiMode] = useState<"cloud" | "local">("cloud");
  const [localReady, setLocalReady] = useState(false);
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
  // Search-language choices: the selectable embedders + the chosen id (onboarding picks one;
  // non-onboarding switches it, re-indexing the vault). Loaded best-effort.
  const [langOpts, setLangOpts] = useState<LanguageOptions | null>(null);
  const [embedderId, setEmbedderId] = useState("");
  // Active tab (non-onboarding only). The scrolling content pane is reset to the top on a
  // tab change so each tab opens from its first section.
  const [tab, setTab] = useState<SettingsTab>("general");
  const contentRef = useRef<HTMLDivElement>(null);
  // Uncommitted edits on the current tab, registered by the controls that defer their write.
  const pending = useSettingsPending();
  // A pending navigation held up by that guard: what is at stake, and where we were going.
  const [leaveGuard, setLeaveGuard] = useState<{ labels: string[]; go: () => void } | null>(null);
  // When the last settings write landed. Drives the transient "Saved" tick in the footer.
  const [savedAt, setSavedAt] = useState<number | null>(null);

  function applyTab(next: SettingsTab) {
    setTab(next);
    contentRef.current?.scrollTo({ top: 0 });
  }

  /** Switch tabs, but stop first if this tab holds an edit that hasn't been committed. Almost every
   *  control here writes on change and registers nothing; the ones that defer (the backup schedule)
   *  would otherwise have their draft silently discarded by the unmount. The dialog NAMES what is
   *  pending rather than saying "unsaved changes" — the point is to let you recognise whether you
   *  care, not to make you anxious. */
  function selectTab(next: SettingsTab) {
    const labels = pending?.labelsForTab(tab) ?? [];
    if (labels.length > 0) {
      setLeaveGuard({ labels, go: () => applyTab(next) });
      return;
    }
    applyTab(next);
  }

  /** Done / clicking outside. Same guard: closing is just a tab switch that lands nowhere. */
  function requestClose() {
    const labels = pending?.labelsForTab(tab) ?? [];
    if (labels.length > 0) {
      setLeaveGuard({ labels, go: onClose });
      return;
    }
    onClose();
  }

  /** The explicit Save. It commits anything pending on this tab, but it ALWAYS flashes the tick,
   *  even when there was nothing to commit — the button exists so there is something to press, and
   *  a press that appears to do nothing is worse than no button. */
  async function saveNow() {
    try {
      await pending?.saveTab(tab);
      setSavedAt(Date.now());
    } catch (e) {
      setError(String(e));
    }
  }

  // The active tab's in-rail sub-nav: scroll-spy lights the section currently in view; clicking a
  // sub-item scrolls its section to the top of the pane. Self-contained tabs have no sections, so the
  // hook simply finds no anchors and stays inert.
  const activeSectionId = useScrollSpy(
    contentRef,
    sectionsFor(tab).map((s) => s.id),
  );

  // Jump to a section AND say so. Scrolling alone is invisible feedback on a tab short enough to fit
  // — the click "did nothing", which is how this control read on most tabs. The wash fires either
  // way, so the sub-nav always answers "which one is that?" and doubles as a locator rather than
  // being purely a scroll shortcut. Re-triggering on an already-washed section needs the class
  // removed and the layout flushed first, or the browser reuses the running animation.
  function scrollToSection(id: string) {
    const el = contentRef.current?.querySelector<HTMLElement>(`#${CSS.escape(id)}`);
    if (!el) return;
    el.scrollIntoView({ behavior: scrollBehavior(), block: "start" });
    el.classList.remove("pm-locate");
    void el.offsetWidth; // force a reflow so the re-added class restarts the animation
    el.classList.add("pm-locate");
    el.addEventListener("animationend", () => el.classList.remove("pm-locate"), { once: true });
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

  // A provider is ready when the active pane is satisfied: a cloud key (entered or already saved),
  // or a configured local model. "Get started" gates on this; "Set up AI later" bypasses it.
  const providerReady = aiMode === "cloud" ? keyAlreadySet || key.trim().length > 0 : localReady;
  const canSave = !saving && providerReady;

  async function save() {
    setSaving(true);
    setError(null);
    try {
      // Cloud pane persists a freshly-entered key; the local pane already persisted its endpoint as
      // the user connected; "Set up AI later" persists no provider at all.
      if (aiMode === "cloud" && key.trim()) {
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
      // Record that onboarding is complete so a keyless finish (a local model, or "set up later")
      // isn't re-prompted on the next launch — the keyless-onboarding gate (#295) reads this.
      if (onboarding) await setOnboardingDone();
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
              className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[0.625rem] font-medium uppercase tracking-wide text-accent-text"
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
              PM needs an AI model to power chat and the behind-the-scenes work. Use a cloud
              provider through OpenRouter, or run a model on this device — you can also set this up
              later.
            </p>
            <div className="mt-3">
              <SegmentedControl
                value={aiMode}
                onChange={setAiMode}
                options={[
                  { value: "cloud", label: "Cloud (OpenRouter)" },
                  { value: "local", label: "On this device" },
                ]}
              />
            </div>
          </div>

          {aiMode === "cloud" ? (
            <>
              <p className="mt-3 text-xs leading-relaxed text-ink4">
                PM reaches AI models through{" "}
                <a
                  href="https://openrouter.ai"
                  target="_blank"
                  rel="noreferrer"
                  className="text-accent-text underline hover:brightness-110"
                >
                  OpenRouter
                </a>{" "}
                — one account for OpenAI, Anthropic, Google, and free models. It's free to start,
                and PM sends Zero-Data-Retention on every request.
              </p>

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
            </>
          ) : (
            <OnboardingLocalConnect onConfigured={setLocalReady} />
          )}

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

          <div className="mt-6 flex items-center justify-between gap-2">
            {/* Keyless start (#295): finish onboarding with no provider. PM works — chat and the
                behind-the-scenes AI stay off (with an honest prompt) until a key or local model is
                added in Settings. */}
            <Button
              variant="tertiary"
              onClick={save}
              disabled={saving}
              data-help="onboarding-later"
            >
              Set up AI later
            </Button>
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
    // Clicking the backdrop closes Settings, through the same guard as Done. mouseDOWN, not click:
    // a click fires on the element the pointer is released over, so releasing outside after
    // starting a text selection or a slider drag INSIDE the panel would otherwise close the window
    // mid-gesture. The target check keeps it to the backdrop itself, never a click that merely
    // bubbled up from the panel.
    <div
      className="flex h-full items-center justify-center p-6"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) requestClose();
      }}
    >
      <div className="flex max-h-[90vh] w-full max-w-3xl flex-col overflow-hidden rounded-[var(--radius)] border border-border bg-panel shadow-xl">
        {/* The window header carries the title only. Its old subtitle ("your API key lives in the
            OS keychain…") was AI-tab material shown on every tab; it now sits in that tab's
            "About your API keys" disclosure, next to the keys it describes. */}
        <div className="shrink-0 border-b border-border px-6 py-4">
          <h1 className="font-head text-lg font-semibold text-ink">Settings</h1>
        </div>

        <div className="flex min-h-0 flex-1">
          <nav className="w-52 shrink-0 overflow-y-auto border-r border-border p-3">
            {SETTINGS_GROUPS.map((group, gi) => {
              const headerId = group.header
                ? `settings-group-${group.header.toLowerCase()}`
                : undefined;
              return (
                <div
                  key={group.header ?? "top"}
                  className={gi > 0 ? "mt-4" : undefined}
                  role={group.header ? "group" : undefined}
                  aria-labelledby={headerId}
                >
                  {group.header && (
                    <div
                      id={headerId}
                      className="px-3 pb-1 font-mono text-[0.625rem] font-medium uppercase tracking-wider text-ink4"
                    >
                      {group.header}
                    </div>
                  )}
                  <div className="flex flex-col gap-0.5">
                    {group.tabs.map((t) => {
                      const active = tab === t.id;
                      return (
                        <div key={t.id}>
                          <NavItem
                            active={active}
                            onClick={() => selectTab(t.id)}
                            leading={<t.Icon className="h-4 w-4" />}
                            trailing={
                              // The same quiet dot the sidebar's Settings row carries (#437), so
                              // following one leads to the other rather than to a dead end.
                              betterFit && t.id === "localai" ? (
                                <AttentionDot label="A better-fitting local model is available" />
                              ) : sheetsNudge && t.id === "connectors" ? (
                                <AttentionDot label="A Google account needs reconnecting to index Sheets" />
                              ) : undefined
                            }
                          >
                            {t.label}
                          </NavItem>
                          {/* The active tab's sub-nav: it animates open via the grid 0fr↔1fr trick and
                            is kept mounted-but-collapsed (and non-interactive) for the others, so
                            switching tabs slides smoothly. Tabs with no sections render nothing. */}
                          {t.sections.length > 0 && (
                            <div
                              className="grid transition-[grid-template-rows] duration-200 ease-out motion-reduce:transition-none"
                              style={{ gridTemplateRows: active ? "1fr" : "0fr" }}
                            >
                              <div className="overflow-hidden">
                                <div
                                  className={cn(
                                    "mt-0.5 flex flex-col gap-0.5 pb-1 pl-6",
                                    !active && "pointer-events-none",
                                  )}
                                  aria-hidden={!active}
                                >
                                  {t.sections.map((s) => (
                                    <button
                                      key={s.id}
                                      type="button"
                                      tabIndex={active ? 0 : -1}
                                      onClick={() => scrollToSection(s.id)}
                                      className={cn(
                                        "truncate border-l-2 py-1 pl-3 text-left text-xs transition-colors",
                                        active && activeSectionId === s.id
                                          ? "border-accent text-ink"
                                          : "border-transparent text-ink4 hover:text-ink2",
                                      )}
                                    >
                                      {s.label}
                                    </button>
                                  ))}
                                </div>
                              </div>
                            </div>
                          )}
                        </div>
                      );
                    })}
                  </div>
                </div>
              );
            })}
          </nav>

          <div
            ref={contentRef}
            className="min-w-0 flex-1 overflow-y-auto px-6 py-4 [&>*:first-child]:mt-0 [&>*:first-child]:border-t-0 [&>*:first-child]:pt-0"
          >
            {tab === "general" && <GeneralSettings />}

            {tab === "accessibility" && <AccessibilitySettings />}

            {tab === "ai" && (
              <AiModelsSettings
                onOpenTeach={onOpenTeach}
                onOpenLocalAi={() => selectTab("localai")}
              />
            )}

            {tab === "localai" && <LocalAiSettings onBetterFitChange={onBetterFitChange} />}

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
          {/* Save sits beside Done deliberately. Almost nothing here needs it — the tabs write on
              change — but a settings window with no Save reads as a settings window that hasn't
              saved, and the button is also the one place a deferred draft (the backup schedule) can
              be committed without hunting for its own control. Pressing it always acknowledges,
              even with nothing to commit. */}
          <div className="flex items-center justify-between gap-3">
            <SavedTick pendingLabels={pending?.labelsForTab(tab) ?? []} savedAt={savedAt} />
            <div className="flex shrink-0 items-center gap-2">
              <Button variant="secondary" onClick={() => void saveNow()}>
                Save
              </Button>
              <Button variant="primary" onClick={requestClose}>
                Done
              </Button>
            </div>
          </div>
        </div>
      </div>

      {/* Two choices, not three, and Save is not one of them. The Save button is right there in the
          footer the dialog is covering, so offering "save and continue" here would duplicate it and
          make the destructive path the quiet one. Cancel puts you back on the tab with the draft
          intact and the button in front of you. */}
      <ConfirmDialog
        open={leaveGuard !== null}
        title="This tab has changes you haven't saved"
        confirmLabel="Discard and continue"
        cancelLabel="Stay here"
        danger
        onConfirm={() => {
          const go = leaveGuard?.go;
          setLeaveGuard(null);
          go?.();
        }}
        onClose={() => setLeaveGuard(null)}
      >
        <p>These would be discarded:</p>
        <ul className="mt-1 list-disc pl-5">
          {(leaveGuard?.labels ?? []).map((l) => (
            <li key={l}>{l}</li>
          ))}
        </ul>
        <p className="mt-2">Stay here and press Save to keep them.</p>
      </ConfirmDialog>
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
