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

import { useCallback, useEffect, useRef, useState } from "react";
import { chatContextStatus, compressChat, revertCompress } from "../lib/ipc";
import type { CompressResult, ContextStatus } from "../lib/types";
import { useDepth } from "../theme/depth";
import { Markdown } from "../lib/markdown";
import { Button, Dialog } from "./ui";
import { Popover } from "./ui";

interface Props {
  conversationId: number | null;
  /** Changes when a new turn lands (message count) so the meter re-reads the freshly-measured usage. */
  refreshKey: number;
  /** Switch the chat to a larger-context model. Delegated to the host (which owns settings). May
   *  return a promise that resolves once the switch has landed, so the meter can re-read then. */
  onUpgrade: (modelId: string) => void | Promise<void>;
}

/** Compact token counts: 980 → "980", 12_300 → "12k", 1_000_000 → "1M". */
function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${Math.round(n / 100_000) / 10}M`.replace(".0M", "M");
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return `${n}`;
}

/** A small circular gauge that fills clockwise with context usage — the Claude-Code context
 *  indicator, rendered from design tokens (accent fill on a border2 track; --st-due when alerting).
 *  The exact percent stays in the trigger's title tooltip, so the ring carries the at-a-glance signal. */
function FillRing({ pct, alerting }: { pct: number; alerting: boolean }) {
  const r = 6;
  const circumference = 2 * Math.PI * r;
  const filled = Math.max(0, Math.min(1, pct / 100));
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5 shrink-0" aria-hidden="true">
      <circle cx="8" cy="8" r={r} fill="none" stroke="var(--border2)" strokeWidth="2.5" />
      <circle
        cx="8"
        cy="8"
        r={r}
        fill="none"
        stroke={alerting ? "var(--st-due)" : "var(--accent)"}
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeDasharray={circumference}
        strokeDashoffset={circumference * (1 - filled)}
        transform="rotate(-90 8 8)"
      />
    </svg>
  );
}

export function ContextMeter({ conversationId, refreshKey, onUpgrade }: Props) {
  const { minimal, showPower } = useDepth();
  const [status, setStatus] = useState<ContextStatus | null>(null);
  const [panelOpen, setPanelOpen] = useState(false);
  const [compressing, setCompressing] = useState(false);
  const [reverting, setReverting] = useState(false);
  const [preview, setPreview] = useState<CompressResult | null>(null);

  // Latest conversation id, so a status read that resolves after the conversation switched is
  // dropped rather than overwriting the current conversation's meter.
  const conversationIdRef = useRef(conversationId);
  conversationIdRef.current = conversationId;
  const refresh = useCallback(() => {
    if (conversationId == null) {
      setStatus(null);
      return;
    }
    const forConv = conversationId;
    chatContextStatus(forConv)
      .then((s) => {
        if (conversationIdRef.current === forConv) setStatus(s);
      })
      .catch(() => {
        if (conversationIdRef.current === forConv) setStatus(null);
      });
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
                {known ? (
                  <FillRing pct={pct} alerting={alerting} />
                ) : (
                  <span className="font-mono">—</span>
                )}
                {showPower &&
                  known &&
                  status.used_tokens != null &&
                  status.context_window != null && (
                    <span className="font-mono">
                      {formatTokens(status.used_tokens)}/{formatTokens(status.context_window)}
                    </span>
                  )}
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
                    onClick={async () => {
                      setPanelOpen(false);
                      // Wait for the switch to actually land (onUpgrade resolves after the host
                      // writes + reloads settings), THEN re-read — a setTimeout would race it.
                      await onUpgrade(m.id);
                      refresh();
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
      <Dialog
        open={preview != null}
        onClose={() => setPreview(null)}
        widthClassName="max-w-md"
        title="Compressed — here’s what was condensed"
        subtitle={
          <>
            The older turns were folded into the running summary
            {preview && preview.reclaimed_est > 0
              ? `, reclaiming about ${formatTokens(preview.reclaimed_est)} tokens`
              : ""}
            . Your full conversation is still kept word-for-word in your vault.
          </>
        }
        footer={
          <>
            <Button variant="tertiary" onClick={handleUndo} disabled={reverting}>
              {reverting ? "Undoing…" : "Undo"}
            </Button>
            <Button variant="primary" onClick={() => setPreview(null)} disabled={reverting}>
              Keep
            </Button>
          </>
        }
      >
        {preview?.condensed_bullets && (
          <div className="pm-inline-md mt-3 max-h-60 overflow-y-auto rounded-[var(--radius-sm)] border border-border bg-panel p-3 text-sm text-ink2">
            <Markdown>{preview.condensed_bullets}</Markdown>
          </div>
        )}
      </Dialog>
    </>
  );
}
