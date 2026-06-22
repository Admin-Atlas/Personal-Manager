// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { modelRecommendations, setRecommendDenylist } from "../lib/ipc";
import type { ModelRecommendation, ModelRecommendations } from "../lib/types";
import { Button, Collapsible, Skeleton, Textarea } from "./ui";

interface Props {
  /** Assign a recommended model to a role — prepends it to the matching editor list;
   *  the user still Saves, so nothing is applied silently (spec §6). */
  onUseForChat: (model: string) => void;
  onUseForBackground: (model: string) => void;
  /** Depth gates: extra cost/capability detail at meta+, the denylist editor at power. */
  showMeta: boolean;
  showPower: boolean;
}

/**
 * The model recommender surface (spec §6): two cards — Day-to-day (cheapest reliable) and
 * Advanced (highest-faithfulness for high-stakes chat) — each proposing one model the user
 * can apply to either role. PM enforces Zero-Data-Retention on every request, so each card
 * carries a ZDR marker; an optional denylist (power depth) excludes providers/models as
 * defense-in-depth. It proposes; the user chooses.
 */
export function ModelRecommendationCards({
  onUseForChat,
  onUseForBackground,
  showMeta,
  showPower,
}: Props) {
  const [recs, setRecs] = useState<ModelRecommendations | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [denyText, setDenyText] = useState("");
  const [savingDeny, setSavingDeny] = useState(false);

  useEffect(() => {
    modelRecommendations()
      .then((r) => {
        setRecs(r);
        setDenyText(r.denylist.join("\n"));
      })
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, []);

  async function saveDenylist() {
    setSavingDeny(true);
    setError(null);
    try {
      const list = denyText
        .split(/[\n,]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      await setRecommendDenylist(list);
      // Re-run the recommender so excluding something updates the picks immediately.
      const r = await modelRecommendations();
      setRecs(r);
      setDenyText(r.denylist.join("\n"));
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingDeny(false);
    }
  }

  return (
    <div data-help="settings-recommended-models">
      <div className="flex items-center justify-between">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Recommended models
        </label>
        {recs?.stale && (
          <span
            title="Couldn't refresh the model list — showing the last known recommendations."
            aria-label="Recommendations may be out of date"
            className="font-mono text-[10px] uppercase tracking-wide text-st-due"
          >
            may be out of date
          </span>
        )}
      </div>
      <p className="mt-1 text-xs text-ink4">
        PM suggests two models from OpenRouter and explains why. Apply either to your chat or
        background slot above — nothing changes until you Save.
      </p>

      {loading && (
        <div className="mt-3 grid gap-3 sm:grid-cols-2">
          <Skeleton className="h-28 w-full" />
          <Skeleton className="h-28 w-full" />
        </div>
      )}

      {!loading && error && (
        <p className="mt-3 text-xs text-st-due">
          Couldn&apos;t load recommendations ({error}). Your chosen models above still work.
        </p>
      )}

      {!loading && !error && recs && (
        <>
          {!recs.day_to_day && !recs.advanced ? (
            <p className="mt-3 text-xs text-ink4">
              No recommendations yet — open this page while online once so PM can fetch the model
              list.
            </p>
          ) : (
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              <RecCard
                slot="Day-to-day"
                tagline="Cheapest reliable — high-volume, low-risk"
                rec={recs.day_to_day}
                zdr={recs.zdr_enforced}
                showMeta={showMeta}
                onUseForChat={onUseForChat}
                onUseForBackground={onUseForBackground}
              />
              <RecCard
                slot="Advanced"
                tagline="Highest faithfulness — high-stakes chat"
                rec={recs.advanced}
                zdr={recs.zdr_enforced}
                showMeta={showMeta}
                onUseForChat={onUseForChat}
                onUseForBackground={onUseForBackground}
              />
            </div>
          )}

          {showPower && (
            <div className="mt-3">
              <Collapsible title="Recommendation exclusions">
                <div className="space-y-2 pt-2 text-xs leading-relaxed text-ink3">
                  <p>
                    Every PM request is sent with{" "}
                    <span className="font-mono text-ink4">zero-data-retention</span> enforced, so a
                    provider can&apos;t store or train on your prompts — this is the real boundary,
                    applied to whichever model you pick. (OpenRouter exposes no per-model
                    data-retention flag, so PM enforces it per request rather than guessing from a
                    list.)
                  </p>
                  <p>
                    The list below only removes models from the two suggestions above — it does not
                    block a model you pick yourself (ZDR already protects every request). One slug
                    per line, e.g. <span className="font-mono text-ink4">openai</span> or{" "}
                    <span className="font-mono text-ink4">openai/gpt-5.5</span>.
                  </p>
                </div>
                <div className="mt-2">
                  <Textarea
                    value={denyText}
                    onChange={(e) => setDenyText(e.target.value)}
                    placeholder="provider-or-model slugs, one per line"
                    className="h-20 font-mono text-xs"
                  />
                  <div className="mt-1 flex justify-end">
                    <Button
                      variant="tertiary"
                      onClick={saveDenylist}
                      disabled={savingDeny}
                      className="px-2 py-0.5 text-xs"
                    >
                      {savingDeny ? "Saving…" : "Save exclusions"}
                    </Button>
                  </div>
                </div>
              </Collapsible>
            </div>
          )}
        </>
      )}
    </div>
  );
}

interface CardProps {
  slot: string;
  tagline: string;
  rec: ModelRecommendation | null;
  zdr: boolean;
  showMeta: boolean;
  onUseForChat: (model: string) => void;
  onUseForBackground: (model: string) => void;
}

function RecCard({
  slot,
  tagline,
  rec,
  zdr,
  showMeta,
  onUseForChat,
  onUseForBackground,
}: CardProps) {
  return (
    <div className="flex flex-col rounded-[var(--radius)] border border-border2 bg-surface p-3">
      <div className="flex items-center justify-between gap-2">
        <span className="font-mono text-[10px] font-medium uppercase tracking-wide text-ink3">
          {slot}
        </span>
        {zdr && (
          <span
            title="Zero-Data-Retention is enforced on every PM request — providers can't store or train on your data."
            aria-label="Zero-Data-Retention enforced on every request"
            className="rounded-[var(--radius-sm)] bg-[color-mix(in_oklab,var(--st-quick)_15%,transparent)] px-1.5 py-0.5 text-[10px] font-medium text-[var(--st-quick)]"
          >
            ZDR
          </span>
        )}
      </div>

      {!rec ? (
        <p className="mt-2 flex-1 text-xs text-ink4">
          No model currently clears the bar for this slot.
        </p>
      ) : (
        <>
          <p className="mt-1 text-[11px] text-ink4">{tagline}</p>
          <div className="mt-1.5">
            <div className="truncate text-sm text-ink" title={rec.name}>
              {rec.name}
            </div>
            <div className="truncate font-mono text-[11px] text-ink4" title={rec.model}>
              {rec.model}
            </div>
          </div>
          <p className="mt-1.5 text-xs leading-relaxed text-ink3">{rec.why}</p>

          {showMeta && (
            <div className="mt-2 flex flex-wrap gap-1.5 font-mono text-[10px] text-ink4">
              <span title="Cache-weighted effective price (not headline per-token).">
                {fmtCostPerM(rec.effective_cost_per_mtok)}
              </span>
              {fmtContext(rec.context_length) && (
                <span className="text-ink4">· {fmtContext(rec.context_length)}</span>
              )}
              {rec.intelligence_index != null && (
                <span title="Artificial-Analysis intelligence index (general capability).">
                  · index {rec.intelligence_index.toFixed(0)}
                </span>
              )}
              {rec.curated && (
                <span className="text-accent-text" title="On PM's curated faithfulness list.">
                  · curated
                </span>
              )}
            </div>
          )}

          <div className="mt-3 flex gap-2 pt-1">
            <Button
              variant="tertiary"
              onClick={() => onUseForChat(rec.model)}
              className="flex-1 px-2 py-1 text-xs"
            >
              Use for chat
            </Button>
            <Button
              variant="tertiary"
              onClick={() => onUseForBackground(rec.model)}
              className="flex-1 px-2 py-1 text-xs"
            >
              Use for background
            </Button>
          </div>
        </>
      )}
    </div>
  );
}

/** Effective (cache-weighted) USD per million tokens, rendered friendly. */
function fmtCostPerM(n: number | null): string {
  if (n == null) return "price unknown";
  if (n === 0) return "Free";
  if (n < 10) return `~$${n.toFixed(2)}/1M`;
  return `~$${n.toFixed(0)}/1M`;
}

/** A compact context-window label ("1M ctx" / "256K ctx"), or null when unknown. */
function fmtContext(n: number | null): string | null {
  if (!n) return null;
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M ctx`;
  }
  if (n >= 1000) return `${Math.round(n / 1000)}K ctx`;
  return `${n} ctx`;
}
