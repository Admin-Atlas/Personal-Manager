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
import { Popover } from "./calendar/Popover";

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
  const { minimal, showPower } = useDepth();
  const [status, setStatus] = useState<ContextStatus | null>(null);
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
      .then((s) => setStatus(s))
      .catch(() => setStatus(null));
  }, [conversationId]);

  // Re-read on conversation switch and after each new turn (and close the panel on switch).
  useEffect(() => {
    setPanelOpen(false);
    refresh();
  }, [conversationId, refresh]);
  useEffect(() => {
    refresh();
  }, [refreshKey, refresh]);

  if (conversationId == null || status == null) return null;

  const known = status.percent != null;
  const pct = Math.round((status.percent ?? 0) * 100);
  const alerting = status.alerting;

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
    setPanelOpen(false);
  }

  return (
    <>
      <Popover
        side="top"
        align="left"
        open={panelOpen}
        onOpenChange={setPanelOpen}
        ariaLabel="Context usage"
        panelClassName="w-[min(90vw,26rem)] max-h-[60vh] overflow-y-auto"
        trigger={({ toggle }) => (
          <button
            type="button"
            onClick={toggle}
            title={
              known
                ? `${pct}% of ${status.model}'s context used`
                : `Context usage is unknown for ${status.model}`
            }
            data-help="context-meter"
            className="flex shrink-0 items-center gap-1.5 rounded-[var(--radius-sm)] border border-border2 px-2 py-1 text-xs text-ink4 hover:text-ink2"
            style={
              alerting
                ? {
                    color: "var(--st-due)",
                    borderColor: "color-mix(in oklab, var(--st-due) 45%, var(--border2))",
                  }
                : undefined
            }
          >
            {alerting && (
              <span
                aria-hidden
                className="h-1.5 w-1.5 shrink-0 rounded-full"
                style={{ background: "var(--st-due)" }}
              />
            )}
            {minimal ? (
              <span>Context full</span>
            ) : (
              <>
                <span>Context</span>
                <span className="font-mono">
                  {known ? `${pct}%` : "—"}
                  {showPower && known && status.used_tokens != null && status.context_window != null
                    ? ` · ${formatTokens(status.used_tokens)}/${formatTokens(status.context_window)}`
                    : ""}
                </span>
              </>
            )}
          </button>
        )}
      >
        <div className="p-2">
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
                      // The host re-reads settings; refresh once the switch has landed.
                      setTimeout(refresh, 0);
                    }}
                    title={`${m.name || m.id} · ${formatTokens(m.context_length)} context`}
                  >
                    Switch to {m.name || m.id} ({formatTokens(m.context_length)})
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
      </Popover>

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
    </>
  );
}
