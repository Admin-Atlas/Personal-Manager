// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import {
  costSummary,
  getLearningProfile,
  getSettings,
  hasOpenRouterBackgroundKey,
  hasOpenRouterKey,
  recommendedModels,
  refreshLearningProfile,
  refreshPricing,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setOpenRouterBackgroundKey,
  setOpenRouterKey,
  setTimeZone,
} from "../lib/ipc";
import { useHelp } from "../lib/help";
import { CalendarSettings } from "./CalendarSettings";
import { ModelListEditor } from "./ModelListEditor";
import type { CostSummary, LearningProfile } from "../lib/types";
import { useTheme, useDepth, ACCENTS } from "../theme";
import { Button, Input, SegmentedControl, Select } from "./ui";

interface Props {
  onClose: () => void;
  /** First-run onboarding requires a key before the app is usable. */
  onboarding: boolean;
}

export function SettingsView({ onClose, onboarding }: Props) {
  const help = useHelp();
  const { system, setSystem, mode, setMode, depth, setDepth, accent, setAccent } = useTheme();
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
  const [profile, setProfile] = useState<LearningProfile | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [timeZone, setTimeZoneState] = useState("");
  const [tzAuto, setTzAuto] = useState(true);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [refreshingPrices, setRefreshingPrices] = useState(false);

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
        if (!onboarding) setProfile(await getLearningProfile());
      } catch (e) {
        setError(String(e));
      }
    })();
  }, [onboarding]);

  // Cost summary loads on its own (its first read may trigger a daily pricing fetch),
  // so it never blocks the rest of Settings from showing.
  useEffect(() => {
    if (onboarding) return;
    costSummary().then(setCost).catch(() => {});
  }, [onboarding]);

  async function refreshProfile() {
    setRefreshing(true);
    setError(null);
    try {
      setProfile(await refreshLearningProfile());
    } catch (e) {
      setError(String(e));
    } finally {
      setRefreshing(false);
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
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-[var(--radius)] border border-border bg-panel p-6 shadow-xl">
        <div className="flex items-center gap-2">
          <h1 className="font-head text-lg font-semibold text-ink">
            {onboarding ? "Welcome to PM" : "Settings"}
          </h1>
          {onboarding && (
            <span
              title="PM is in alpha — under active development; expect rough edges and changes between updates."
              className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-wide text-accent-text"
            >
              Alpha
            </span>
          )}
        </div>
        <p className="mt-1 text-sm text-ink3">
          {onboarding
            ? "Add your OpenRouter API key to start chatting. It's stored in your OS keychain, never on disk or in the repo."
            : "Your API key lives in the OS keychain. The model is swappable anytime."}
        </p>

        {!onboarding && (
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
                      accent === hex ? "ring-2 ring-ink ring-offset-2 ring-offset-[var(--surface)]" : ""
                    }`}
                  />
                ))}
              </div>
            </div>
          </div>
        )}

        {!onboarding && (
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
        )}

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

        {!onboarding && (
          <>
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
              Used for background work (sorting proposals, learning). Lets you track that
              spend separately. Falls back to your main key if blank.
            </p>
          </>
        )}

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
          {!onboarding && (
            <div className="flex flex-wrap gap-2" data-help="settings-recommended-models">
              <Button
                variant="tertiary"
                className="px-2 py-1 text-xs"
                onClick={async () => setChatModelsState(await recommendedModels("chat"))}
              >
                Use recommended chat models
              </Button>
              <Button
                variant="tertiary"
                className="px-2 py-1 text-xs"
                onClick={async () => setBackgroundModelsState(await recommendedModels("background"))}
              >
                Use recommended background models
              </Button>
            </div>
          )}
        </div>

        {!onboarding && showMeta && cost && (
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
            <p className="mt-1 text-xs text-ink4">
              Token spend, priced from OpenRouter
              {cost.pricing_updated_at ? ` · prices updated ${formatWhen(cost.pricing_updated_at)}` : ""}.
            </p>
            <div className="mt-2 flex gap-6 text-sm">
              <div>
                <div className="text-xs text-ink4">Last 30 days</div>
                <div className="font-mono text-ink2">{fmtUsd(cost.total_30d_usd)}</div>
              </div>
              <div>
                <div className="text-xs text-ink4">All time</div>
                <div className="font-mono text-ink2">{fmtUsd(cost.total_all_time_usd)}</div>
              </div>
            </div>
            {showPower && cost.all_time.length > 0 && (
              <table className="mt-3 w-full text-left text-xs">
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
                        {s.prompt_tokens.toLocaleString()} / {s.completion_tokens.toLocaleString()}
                      </td>
                      <td className="py-1 text-right font-mono text-ink3">{fmtUsd(s.cost_usd)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        )}

        {!onboarding && (
          <div className="mt-5 border-t border-border pt-4" data-help="settings-learning">
            <div className="flex items-center justify-between">
              <label className="block text-sm font-medium text-ink2">Learning You</label>
              <Button
                variant="tertiary"
                onClick={refreshProfile}
                disabled={refreshing}
              >
                {refreshing ? "Refreshing…" : "Refresh now"}
              </Button>
            </div>
            <p className="mt-1 text-xs text-ink4">
              What PM has learned about how you organise, distilled from your review corrections,
              and fed into its suggestions and chat.
            </p>
            <div className="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap rounded-[var(--radius)] border border-border bg-surface px-3 py-2 text-xs text-ink2">
              {profile?.profile?.trim()
                ? profile.profile
                : "Nothing learned yet — it builds up as you correct the AI's proposals in Review."}
            </div>
            <p className="mt-1 text-xs text-faint">
              {profile ? `${profile.correction_count} correction${profile.correction_count === 1 ? "" : "s"} logged` : ""}
              {profile?.updated_at ? ` · updated ${formatWhen(profile.updated_at)}` : ""}
            </p>
          </div>
        )}

        {!onboarding && <CalendarSettings />}

        {!onboarding && (
          <div className="mt-4 flex items-start justify-between gap-3 border-t border-border pt-4" data-help="settings-help-mode">
            <div>
              <label className="block text-sm font-medium text-ink2">Help mode</label>
              <p className="mt-1 text-xs text-ink4">
                When on, hovering any highlighted section shows a short explanation of what it does.
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
        )}

        {!onboarding && (
          <div className="mt-5 border-t border-border pt-4 text-xs leading-relaxed text-ink4" data-help="settings-license">
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
          {!onboarding && (
            <Button variant="tertiary" onClick={onClose}>
              Cancel
            </Button>
          )}
          <Button variant="primary" onClick={save} disabled={!canSave}>
            {saving ? "Saving…" : "Save"}
          </Button>
        </div>
      </div>
    </div>
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
