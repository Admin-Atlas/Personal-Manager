// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { formatWhen } from "../../lib/format";
import {
  costSummary,
  getSettings,
  hasOpenRouterBackgroundKey,
  hasOpenRouterKey,
  refreshPricing,
  setBackgroundAutoSwitch,
  setBackgroundModels,
  setChatAutoSwitch,
  setChatModels,
  setOpenRouterBackgroundKey,
  setOpenRouterKey,
  settingsDefaults,
} from "../../lib/ipc";
import type { CostSummary, Settings } from "../../lib/types";
import { ModelListEditor } from "../ModelListEditor";
import { Button, Input, SectionInfo, Toggle } from "../ui";
import { TabResetSection } from "./ResetControls";
import { readReviewAiEnabled, writeReviewAiEnabled } from "../../lib/reviewPrefs";

/** Order-sensitive equality for the two model-role lists — a reset is offered only when a role's
 *  ordered list (or its auto-switch) differs from the default. */
function sameList(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((m, i) => m === b[i]);
}

/** Format a USD cost, or "—" when unknown (the model isn't in the price cache yet). */
function fmtUsd(v: number | null): string {
  if (v == null) return "—";
  if (v === 0) return "$0.00";
  return v < 0.01 ? `$${v.toFixed(4)}` : `$${v.toFixed(2)}`;
}

/** The AI & Models Settings tab. Self-contained and immediate-save: model lists and auto-switch
 *  persist the moment you change them; the API keys save when you click away from the field, with a
 *  green confirmation. Errors surface inline. Onboarding has its own key + model inputs (they share a
 *  Get-started button there) — this is the non-onboarding tab. */
export function AiModelsSettings() {
  const [key, setKey] = useState("");
  const [bgKey, setBgKey] = useState("");
  const [keyAlreadySet, setKeyAlreadySet] = useState(false);
  const [bgKeyAlreadySet, setBgKeyAlreadySet] = useState(false);
  const [keySaved, setKeySaved] = useState(false);
  const [bgKeySaved, setBgKeySaved] = useState(false);
  const [chatModels, setChatModelsState] = useState<string[]>([]);
  const [backgroundModels, setBackgroundModelsState] = useState<string[]>([]);
  const [chatAuto, setChatAuto] = useState(false);
  const [backgroundAuto, setBackgroundAuto] = useState(false);
  // True once getSettings() has populated the model lists. Model writes gate on this (not a non-empty
  // list) so clearing a role back to "use the default" persists, while a write can never fire against
  // the empty pre-load state.
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  // The out-of-the-box defaults (one backend source), for the per-role + per-tab reset affordances.
  const [defaults, setDefaults] = useState<Settings | null>(null);
  const [cost, setCost] = useState<CostSummary | null>(null);
  const [refreshingPrices, setRefreshingPrices] = useState(false);
  // Whether Review asks the model to suggest a project/tags/importance per item. A per-device frontend
  // gate (localStorage), shared with the Review tab's "Turn on AI" banner — off by default.
  const [reviewAi, setReviewAi] = useState(readReviewAiEnabled);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const [hasKey, hasBgKey, settings, defs] = await Promise.all([
          hasOpenRouterKey(),
          hasOpenRouterBackgroundKey(),
          getSettings(),
          settingsDefaults(),
        ]);
        if (cancelled) return;
        setKeyAlreadySet(hasKey);
        setBgKeyAlreadySet(hasBgKey);
        setChatModelsState(settings.chat_models);
        setBackgroundModelsState(settings.background_models);
        setChatAuto(settings.chat_auto_switch);
        setBackgroundAuto(settings.background_auto_switch);
        setDefaults(defs);
        setSettingsLoaded(true);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Cost summary loads on its own (its first read may trigger a daily pricing fetch), so it never
  // blocks the keys/models from showing.
  useEffect(() => {
    costSummary()
      .then(setCost)
      .catch(() => {});
  }, []);

  // The green "Saved" confirmations fade a couple of seconds after a key is stored.
  useEffect(() => {
    if (!keySaved) return;
    const t = setTimeout(() => setKeySaved(false), 2500);
    return () => clearTimeout(t);
  }, [keySaved]);
  useEffect(() => {
    if (!bgKeySaved) return;
    const t = setTimeout(() => setBgKeySaved(false), 2500);
    return () => clearTimeout(t);
  }, [bgKeySaved]);

  // Keys save the moment you click away — no Save button. A blank field is a no-op (so tabbing past an
  // already-saved key never wipes it); a real value is stored in the OS keychain and the field clears
  // to its masked "saved" placeholder with a green confirmation.
  async function saveKey() {
    const trimmed = key.trim();
    if (!trimmed) return;
    setError(null);
    try {
      await setOpenRouterKey(trimmed);
      setKey("");
      setKeyAlreadySet(true);
      setKeySaved(true);
    } catch (e) {
      setError(String(e));
    }
  }
  async function saveBgKey() {
    const trimmed = bgKey.trim();
    if (!trimmed) return;
    setError(null);
    try {
      await setOpenRouterBackgroundKey(trimmed);
      setBgKey("");
      setBgKeyAlreadySet(true);
      setBgKeySaved(true);
    } catch (e) {
      setError(String(e));
    }
  }

  function changeChatModels(models: string[]) {
    setChatModelsState(models);
    if (settingsLoaded) void setChatModels(models).catch((e) => setError(String(e)));
  }
  function changeBackgroundModels(models: string[]) {
    setBackgroundModelsState(models);
    if (settingsLoaded) void setBackgroundModels(models).catch((e) => setError(String(e)));
  }
  function changeChatAuto(on: boolean) {
    setChatAuto(on);
    void setChatAutoSwitch(on).catch((e) => setError(String(e)));
  }
  function changeBackgroundAuto(on: boolean) {
    setBackgroundAuto(on);
    void setBackgroundAutoSwitch(on).catch((e) => setError(String(e)));
  }
  function changeReviewAi(on: boolean) {
    setReviewAi(on);
    writeReviewAiEnabled(on); // shared with the Review tab's "Turn on AI" banner
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

  // A role is at its default when its ordered list AND auto-switch match the backend defaults. Until
  // the defaults load, nothing is offered a reset. Reset restores both the list and the auto toggle
  // (reusing the same change* handlers, so it persists exactly like a manual edit).
  const chatRoleIsDefault = !defaults || (sameList(chatModels, defaults.chat_models) && !chatAuto);
  const backgroundRoleIsDefault =
    !defaults || (sameList(backgroundModels, defaults.background_models) && !backgroundAuto);
  function resetChatRole() {
    if (!defaults) return;
    changeChatModels(defaults.chat_models);
    changeChatAuto(false);
  }
  function resetBackgroundRole() {
    if (!defaults) return;
    changeBackgroundModels(defaults.background_models);
    changeBackgroundAuto(false);
  }

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

      <label
        id="sec-ai-keys"
        data-settings-section
        className="mt-5 block text-sm font-medium text-ink2"
      >
        OpenRouter API key
      </label>
      <Input
        type="password"
        autoComplete="off"
        data-help="settings-api-key"
        value={key}
        onChange={(e) => setKey(e.target.value)}
        onBlur={() => void saveKey()}
        placeholder={keyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
        className="mt-1"
      />
      {keySaved ? (
        <p className="mt-1 text-xs font-medium text-st-track">✓ Saved to your keychain</p>
      ) : (
        <a
          href="https://openrouter.ai/keys"
          target="_blank"
          rel="noreferrer"
          className="mt-1 inline-block text-xs text-ink4 hover:text-ink2"
        >
          Get a key at openrouter.ai/keys →
        </a>
      )}

      <label className="mt-4 block text-sm font-medium text-ink2">Background API key</label>
      <Input
        type="password"
        autoComplete="off"
        data-help="settings-background-key"
        value={bgKey}
        onChange={(e) => setBgKey(e.target.value)}
        onBlur={() => void saveBgKey()}
        placeholder={bgKeyAlreadySet ? "•••••••• (saved — type to replace)" : "sk-or-..."}
        className="mt-1"
      />
      {bgKeySaved && (
        <p className="mt-1 text-xs font-medium text-st-track">✓ Saved to your keychain</p>
      )}
      {/* Both key fields' explanation in one disclosure at the foot of the pair —
          including the keychain sentence that used to head every tab. */}
      <SectionInfo title="About your API keys">
        <p>
          Your API key lives in the OS keychain and saves as soon as you click away. The model is
          swappable anytime.
        </p>
        <p>
          The background key is used for background work (sorting proposals, learning). Lets you
          track that spend separately. Falls back to your main key if blank.
        </p>
      </SectionInfo>

      <div
        id="sec-ai-models"
        data-settings-section
        className="mt-5 space-y-5 border-t border-border pt-4"
      >
        <ModelListEditor
          label="Chat model"
          description="Answers your chats. Add several and turn on auto-switch to fall back when one runs out."
          helpId="settings-chat-models"
          models={chatModels}
          onChange={changeChatModels}
          autoSwitch={chatAuto}
          onAutoSwitchChange={changeChatAuto}
          onReset={chatRoleIsDefault ? undefined : resetChatRole}
        />
        <ModelListEditor
          label="Background model"
          description="Runs sorting proposals and Learning You. Free models work well here; chain a few for daily limits."
          helpId="settings-background-models"
          models={backgroundModels}
          onChange={changeBackgroundModels}
          autoSwitch={backgroundAuto}
          onAutoSwitchChange={changeBackgroundAuto}
          onReset={backgroundRoleIsDefault ? undefined : resetBackgroundRole}
        />
      </div>

      <div id="sec-ai-review" data-settings-section className="mt-5 border-t border-border pt-4">
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
              Filing suggestions
            </label>
            <p className="mt-1 text-xs text-ink4">
              Let PM propose a project, tags and importance for each new item in Review — a real
              help when you're importing a lot. Off by default; uses the background model above. You
              can also turn this on from the banner in Review.
            </p>
          </div>
          <Toggle
            checked={reviewAi}
            onChange={changeReviewAi}
            ariaLabel="AI filing suggestions in Review"
          />
        </div>
      </div>

      {cost && (
        <div
          id="sec-ai-usage"
          data-settings-section
          className="mt-5 border-t border-border pt-4"
          data-help="settings-usage-cost"
        >
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
          {/* The "Spend & breakdown" Collapsible is gone: it hid your own numbers
              behind a caret (and unfolded them only for Power), while its `meta` slot
              leaked the 30d total back out to prove the point. The totals and the
              per-model table are readouts — the thing you opened this section for — so
              they stay visible; only the pricing methodology folds away below. */}
          <div className="mt-3 flex gap-6 text-sm">
            <div>
              <div className="text-xs text-ink4">Last 30 days</div>
              <div className="font-mono text-ink2">{fmtUsd(cost.total_30d_usd)}</div>
            </div>
            <div>
              <div className="text-xs text-ink4">All time</div>
              <div className="font-mono text-ink2">{fmtUsd(cost.total_all_time_usd)}</div>
            </div>
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
                        {s.prompt_tokens.toLocaleString()} / {s.completion_tokens.toLocaleString()}
                      </td>
                      <td className="py-1 text-right font-mono text-ink3">{fmtUsd(s.cost_usd)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="mt-2 text-xs text-ink4">No model calls logged yet.</p>
          )}
          <p className="mt-2 text-xs text-faint">
            Local models (Settings → Local AI) aren&apos;t counted here — this ledger tracks only
            your paid cloud calls.
          </p>
          <SectionInfo title="How this is calculated">
            <p>
              Your real per-call cost as reported by OpenRouter where available, otherwise estimated
              from the tokens each call used × OpenRouter&apos;s per-token price
              {cost.pricing_updated_at
                ? ` (prices updated ${formatWhen(cost.pricing_updated_at)})`
                : ""}
              .
            </p>
            <p>
              Each model reply reports the tokens it used (your prompt + its reply). PM logs those
              per call. It fetches OpenRouter&apos;s public price list about once a day and caches
              it — no extra model call, and your API key is never used for it.
            </p>
            <p>
              Where OpenRouter reports a call&apos;s actual cost (reflecting any prompt-cache
              discount) PM shows that; for older calls without it, cost = prompt&nbsp;tokens ×
              prompt&nbsp;price + reply&nbsp;tokens × reply&nbsp;price. It&apos;s computed when you
              open this page, so a later price change re-prices your history. A model with no
              reported cost and not yet in the price cache shows{" "}
              <span className="font-mono text-ink4">—</span>, never an understated&nbsp;$0.
            </p>
          </SectionInfo>
        </div>
      )}

      <TabResetSection
        tabName="AI & Models"
        isDefault={chatRoleIsDefault && backgroundRoleIsDefault && !reviewAi}
        onReset={() => {
          resetChatRole();
          resetBackgroundRole();
          changeReviewAi(false);
        }}
        confirmBody={
          <>
            Restores the chat and background models (and their auto-switch) and turns Review filing
            suggestions back off. Your saved API keys aren&apos;t affected.
          </>
        }
      />
    </>
  );
}
