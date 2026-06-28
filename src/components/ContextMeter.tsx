// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The chat context-usage meter + its ~80% alert (board card 7D, #143). A subtle, Depth-tiered indicator of
// how full the SELECTED model's context window is — measured from the backend (`chat_context_status`), which
// reads the exact prompt size OpenRouter reported last turn over the model's catalogued window. When usage
// crosses the alert line the meter offers the card's three actions: Compress (reclaim room by folding older
// turns into the rolling summary, with a HITL "what was condensed" check + Undo), Continue (dismiss with a
// soft warning, re-armed only after usage drops back under the line), and Upgrade (switch to a
// larger-context model — shown only when one meaningfully larger exists; the backend suppresses it on 1M+).
//
// Thresholds live in Rust (`context_budget`) — this component only renders what the status reports.

import { useCallback, useEffect, useState } from "react";
import { chatContextStatus, compressChat, revertCompress } from "../lib/ipc";
import type { CompressResult, ContextStatus } from "../lib/types";
import { useDepth } from "../theme/depth";
import { Button, Modal } from "./ui";

interface Props {
  conversationId: number | null;
  /** Changes when a new turn lands (message count) so the meter re-reads the freshly-measured usage. */
  refreshKey: number;
  /** Switch the chat to a larger-context model. Delegated to the host (which owns settings). */
  onUpgrade: (modelId: string) => void;
}

/** Compact token counts: 980 → "980", 12_300 → "12k", 1_000_000 → "1M". */
function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${Math.round(n / 100_000) / 10}M`.replace(".0M", "M");
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return `${n}`;
}

export function ContextMeter({ conversationId, refreshKey, onUpgrade }: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const [status, setStatus] = useState<ContextStatus | null>(null);
  const [dismissed, setDismissed] = useState(false);
  const [panelOpen, setPanelOpen] = useState(false);
  const [compressing, setCompressing] = useState(false);
  const [reverting, setReverting] = useState(false);
  const [preview, setPreview] = useState<CompressResult | null>(null);

  const refresh = useCallback(() => {
    if (conversationId == null) {
      setStatus(null);
      return;
    }
    chatContextStatus(conversationId)
      .then((s) => {
        setStatus(s);
        // Re-arm the alert once usage falls back under the line (the next crossing nags again).
        if (!s.alerting) setDismissed(false);
      })
      .catch(() => setStatus(null));
  }, [conversationId]);

  // Re-read on conversation switch and after each new turn (and reset transient UI on switch).
  useEffect(() => {
    setPanelOpen(false);
    setDismissed(false);
    refresh();
  }, [conversationId, refresh]);
  useEffect(() => {
    refresh();
  }, [refreshKey, refresh]);

  if (conversationId == null || status == null) return null;

  const known = status.percent != null;
  const pct = Math.round((status.percent ?? 0) * 100);
  const frac = Math.max(0, Math.min(1, status.percent ?? 0));
  const alerting = status.alerting;
  const barColor = alerting ? "var(--st-due)" : "var(--accent)";
  const showPanel = (alerting && !dismissed) || panelOpen;

  // Minimal depth stays out of the way entirely until usage is in alert territory.
  if (minimal && !alerting) return null;

  async function handleCompress() {
    if (conversationId == null) return;
    setCompressing(true);
    try {
      const result = await compressChat(conversationId);
      if (result) {
        setPreview(result);
        setPanelOpen(false);
      }
    } catch {
      /* best-effort: leave the alert up so the user can retry or upgrade */
    } finally {
      setCompressing(false);
      refresh();
    }
  }

  async function handleUndo() {
    if (conversationId == null || preview == null) return;
    setReverting(true);
    try {
      await revertCompress(conversationId, preview.snapshot);
    } catch {
      /* best-effort */
    } finally {
      setReverting(false);
      setPreview(null);
      refresh();
    }
  }

  function handleContinue() {
    setDismissed(true);
    setPanelOpen(false);
  }

  return (
    <div className="border-t border-border px-4 pt-2">
      <div className="mx-auto max-w-3xl">
        <button
          type="button"
          onClick={() => setPanelOpen((o) => !o)}
          title={
            known
              ? `${pct}% of ${status.model}'s context used`
              : `Context usage is unknown for ${status.model}`
          }
          className="flex w-full items-center gap-2 text-left"
          data-help="context-meter"
        >
          {alerting && (
            <span
              aria-hidden
              className="h-1.5 w-1.5 shrink-0 rounded-full"
              style={{ background: "var(--st-due)" }}
            />
          )}
          {showMeta && (
            <span
              className="shrink-0 text-xs"
              style={{ color: alerting ? "var(--st-due)" : "var(--ink4)" }}
            >
              Context
            </span>
          )}
          {/* The bar (standard + power). A token-driven track so it can carry the alert colour. */}
          {showMeta && (
            <span className="h-1 flex-1 overflow-hidden rounded-[var(--radius-sm)] bg-border">
              {known && (
                <span
                  className="block h-full rounded-[var(--radius-sm)] transition-[width] duration-300 ease-out"
                  style={{ width: `${frac * 100}%`, background: barColor }}
                />
              )}
            </span>
          )}
          {showMeta && (
            <span
              className="shrink-0 font-mono text-xs"
              style={{ color: alerting ? "var(--st-due)" : "var(--ink4)" }}
            >
              {known ? `${pct}%` : "—"}
              {showPower && known && status.used_tokens != null && status.context_window != null
                ? ` · ${formatTokens(status.used_tokens)}/${formatTokens(status.context_window)}`
                : ""}
            </span>
          )}
          {/* Minimal depth: no bar, just the alert affordance. */}
          {minimal && (
            <span className="text-xs" style={{ color: "var(--st-due)" }}>
              Context almost full — review
            </span>
          )}
        </button>

        {showPanel && (
          <div className="mt-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
            <p className="text-sm text-ink2">
              {alerting
                ? `This conversation is filling ${status.model}'s context (${known ? `${pct}%` : "—"}).`
                : `${status.model}'s context is ${known ? `${pct}%` : "—"} full.`}
            </p>
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {status.compress.available ? (
                <Button variant="primary" onClick={handleCompress} disabled={compressing}>
                  {compressing ? "Compressing…" : "Compress"}
                </Button>
              ) : (
                status.compress.reason && (
                  <span className="text-xs text-ink4">{status.compress.reason}</span>
                )
              )}
              {status.upgrade.length > 0 && (
                <div className="flex flex-wrap items-center gap-1.5">
                  {status.upgrade.map((m) => (
                    <Button
                      key={m.id}
                      variant="secondary"
                      onClick={() => {
                        onUpgrade(m.id);
                        setPanelOpen(false);
                        setDismissed(true);
                        // The host re-reads settings; refresh once the switch has landed.
                        setTimeout(refresh, 0);
                      }}
                      title={`${m.name} · ${formatTokens(m.context_length)} context`}
                    >
                      Switch to {m.name} ({formatTokens(m.context_length)})
                    </Button>
                  ))}
                </div>
              )}
              {alerting && (
                <Button variant="tertiary" onClick={handleContinue}>
                  Continue anyway
                </Button>
              )}
            </div>
          </div>
        )}
      </div>

      {/* HITL verify: compression is already applied; show what was condensed so the user can Undo. */}
      <Modal open={preview != null} onClose={() => setPreview(null)} widthClassName="max-w-md">
        <div className="p-5">
          <h2 className="font-head text-base font-semibold text-ink">
            Compressed — here&rsquo;s what was condensed
          </h2>
          <p className="mt-1 text-xs text-ink4">
            The older turns were folded into the running summary
            {preview && preview.reclaimed_est > 0
              ? `, reclaiming about ${formatTokens(preview.reclaimed_est)} tokens`
              : ""}
            . Your full conversation is still kept word-for-word in your vault.
          </p>
          {preview?.condensed_bullets && (
            <div className="mt-3 max-h-60 overflow-y-auto whitespace-pre-wrap rounded-[var(--radius-sm)] border border-border bg-panel p-3 text-sm text-ink2">
              {preview.condensed_bullets}
            </div>
          )}
          <div className="mt-5 flex justify-end gap-2">
            <Button variant="tertiary" onClick={handleUndo} disabled={reverting}>
              {reverting ? "Undoing…" : "Undo"}
            </Button>
            <Button variant="primary" onClick={() => setPreview(null)} disabled={reverting}>
              Keep
            </Button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
