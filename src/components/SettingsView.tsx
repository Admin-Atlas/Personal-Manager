// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import {
  getLearningProfile,
  getSettings,
  hasOpenRouterBackgroundKey,
  hasOpenRouterKey,
  refreshLearningProfile,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setOpenRouterBackgroundKey,
  setOpenRouterKey,
} from "../lib/ipc";
import { useHelp } from "../lib/help";
import { CalendarSettings } from "./CalendarSettings";
import { ModelListEditor } from "./ModelListEditor";
import type { LearningProfile } from "../lib/types";

interface Props {
  onClose: () => void;
  /** First-run onboarding requires a key before the app is usable. */
  onboarding: boolean;
}

export function SettingsView({ onClose, onboarding }: Props) {
  const help = useHelp();
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
        if (!onboarding) setProfile(await getLearningProfile());
      } catch (e) {
        setError(String(e));
      }
    })();
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
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex h-full items-center justify-center p-6">
      <div className="max-h-[90vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-neutral-800 bg-neutral-900 p-6 shadow-xl">
        <div className="flex items-center gap-2">
          <h1 className="text-lg font-semibold text-neutral-100">
            {onboarding ? "Welcome to PM" : "Settings"}
          </h1>
          {onboarding && (
            <span
              title="PM is in alpha — under active development; expect rough edges and changes between updates."
              className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-amber-300"
            >
              Alpha
            </span>
          )}
        </div>
        <p className="mt-1 text-sm text-neutral-400">
          {onboarding
            ? "Add your OpenRouter API key to start chatting. It's stored in your OS keychain, never on disk or in the repo."
            : "Your API key lives in the OS keychain. The model is swappable anytime."}
        </p>

        <label className="mt-5 block text-sm font-medium text-neutral-300">
          OpenRouter API key
        </label>
        <input
          type="password"
          autoComplete="off"
          data-help="settings-api-key"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder={keyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
          className="mt-1 w-full rounded-lg border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100 outline-none focus:border-neutral-500"
        />
        <a
          href="https://openrouter.ai/keys"
          target="_blank"
          rel="noreferrer"
          className="mt-1 inline-block text-xs text-neutral-500 hover:text-neutral-300"
        >
          Get a key at openrouter.ai/keys →
        </a>

        {!onboarding && (
          <>
            <label className="mt-4 block text-sm font-medium text-neutral-300">
              Background API key
            </label>
            <input
              type="password"
              autoComplete="off"
              data-help="settings-background-key"
              value={bgKey}
              onChange={(e) => setBgKey(e.target.value)}
              placeholder={bgKeyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
              className="mt-1 w-full rounded-lg border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-neutral-100 outline-none focus:border-neutral-500"
            />
            <p className="mt-1 text-xs text-neutral-500">
              Used for background work (sorting proposals, learning). Lets you track that
              spend separately. Falls back to your main key if blank.
            </p>
          </>
        )}

        <div className="mt-5 space-y-5 border-t border-neutral-800 pt-4">
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

        {!onboarding && (
          <div className="mt-5 border-t border-neutral-800 pt-4" data-help="settings-learning">
            <div className="flex items-center justify-between">
              <label className="block text-sm font-medium text-neutral-300">Learning You</label>
              <button
                onClick={refreshProfile}
                disabled={refreshing}
                className="rounded-md px-2 py-1 text-xs text-neutral-400 hover:bg-neutral-800 hover:text-neutral-200 disabled:opacity-40"
              >
                {refreshing ? "Refreshing…" : "Refresh now"}
              </button>
            </div>
            <p className="mt-1 text-xs text-neutral-500">
              What PM has learned about how you organise, distilled from your review corrections,
              and fed into its suggestions and chat.
            </p>
            <div className="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap rounded-lg border border-neutral-800 bg-neutral-950 px-3 py-2 text-xs text-neutral-300">
              {profile?.profile?.trim()
                ? profile.profile
                : "Nothing learned yet — it builds up as you correct the AI's proposals in Review."}
            </div>
            <p className="mt-1 text-xs text-neutral-600">
              {profile ? `${profile.correction_count} correction${profile.correction_count === 1 ? "" : "s"} logged` : ""}
              {profile?.updated_at ? ` · updated ${formatWhen(profile.updated_at)}` : ""}
            </p>
          </div>
        )}

        {!onboarding && <CalendarSettings />}

        {!onboarding && (
          <div className="mt-4 flex items-start justify-between gap-3 border-t border-neutral-800 pt-4" data-help="settings-help-mode">
            <div>
              <label className="block text-sm font-medium text-neutral-300">Help mode</label>
              <p className="mt-1 text-xs text-neutral-500">
                When on, hovering any highlighted section shows a short explanation of what it does.
              </p>
            </div>
            <button
              role="switch"
              aria-checked={help.enabled}
              onClick={() => help.setEnabled(!help.enabled)}
              className={`mt-0.5 inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors ${
                help.enabled ? "bg-amber-500" : "bg-neutral-700"
              }`}
            >
              <span
                className={`inline-block h-4 w-4 transform rounded-full bg-white transition-transform ${
                  help.enabled ? "translate-x-4" : "translate-x-0.5"
                }`}
              />
            </button>
          </div>
        )}

        {!onboarding && (
          <div className="mt-5 border-t border-neutral-800 pt-4 text-xs leading-relaxed text-neutral-500" data-help="settings-license">
            <p>
              PM is free software, licensed under the{" "}
              <a
                href="https://www.gnu.org/licenses/agpl-3.0.html"
                target="_blank"
                rel="noreferrer"
                className="text-neutral-400 underline hover:text-neutral-200"
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
                className="text-neutral-400 underline hover:text-neutral-200"
              >
                github.com/Admin-Atlas/Personal-Manager
              </a>
            </p>
          </div>
        )}

        {error && (
          <p className="mt-3 rounded-lg bg-red-950/60 px-3 py-2 text-sm text-red-300">
            {error}
          </p>
        )}

        <div className="mt-6 flex justify-end gap-2">
          {!onboarding && (
            <button
              onClick={onClose}
              className="rounded-lg px-4 py-2 text-sm text-neutral-300 hover:bg-neutral-800"
            >
              Cancel
            </button>
          )}
          <button
            onClick={save}
            disabled={!canSave}
            className="rounded-lg bg-neutral-100 px-4 py-2 text-sm font-medium text-neutral-900 hover:bg-white disabled:cursor-not-allowed disabled:opacity-40"
          >
            {saving ? "Saving…" : "Save"}
          </button>
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
