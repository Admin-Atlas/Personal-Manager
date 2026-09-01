// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";

import {
  acceptLocalModelTerms,
  activeLocalPull,
  cancelLocalPull,
  pullLocalModel,
} from "../../lib/ipc";
import type {
  LocalRecommendation,
  LocalRecommendations,
  PullProgress,
  PullSnapshot,
} from "../../lib/types";
import { formatBytes, formatGib } from "../../lib/format";
import { IngestProgress } from "../IngestProgress";
import { installCommand } from "../../lib/workbenchGuide";
import { ConfigRow, FitBadge } from "./fitDisplay";
import { Button, Collapsible, ConfirmDialog, SectionLabel, Select } from "../ui";

/**
 * "Recommended models" — the curated catalog sized against this machine, and the one-click pull.
 *
 * It owns the download, because the download is this section's: the job itself is backend-owned
 * (it survives the tab unmounting), and everything here is the view of it — which card is marked,
 * what the progress bar says, and the licence dialog that has to be answered before a restricted
 * model is fetched. The tab keeps only what other sections also read.
 */
export function LocalAiCatalog({
  recs,
  loading,
  configured,
  isOllama,
  servedTags,
  installedRepos,
  onRecs,
  onReload,
  onRefreshRecs,
  onCadence,
  onError,
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  configured: boolean;
  /** Whether the connected server is an Ollama — the only runner PM can pull into. */
  isOllama: boolean;
  servedTags: Set<string>;
  installedRepos: Set<string>;
  /** Replace the tab's recommendations (a licence acceptance, a finished pull). */
  onRecs: (recs: LocalRecommendations) => void;
  /** Re-read the stored config and the served-model list. */
  onReload: () => Promise<void>;
  /** Re-read the recommendations from the backend. */
  onRefreshRecs: () => Promise<void>;
  onCadence: (cadence: string) => void;
  onError: (message: string | null) => void;
}) {
  // `pulling` holds the pull TAG (`hf.co/<repo>:<QUANT>`), not the repo: the backend's job snapshot
  // is keyed on the tag, so a view that mounts mid-download can adopt it and mark the right card.
  const [pulling, setPulling] = useState<string | null>(null);
  const [pullProg, setPullProg] = useState<PullProgress | null>(null);
  /** The model whose licence terms are being shown, and the pull tag the user asked for, or null
   *  when no dialog is open. The TAG rides along because a card can offer more than one way to run
   *  the same model: resolving it again after the dialog would resolve the card's default, not the
   *  row the user actually clicked. */
  const [termsFor, setTermsFor] = useState<{ rec: LocalRecommendation; tag: string } | null>(null);
  /** Mirrors `pulling` synchronously. `pull()` below is async, so the `pulling` it captured when it
   *  started is stale by the time its `finally` runs — and that `finally` must be able to tell
   *  whether the tag it started is still the one on screen. */
  const pullingRef = useRef<string | null>(null);

  /** The single writer for the pull marker. Keeps `pullingRef` in step with the state so the two
   *  can never disagree — a marker cleared in one and not the other silently kills the 1s snapshot
   *  poller, whose effect dependency is `pulling`. */
  const markPulling = useCallback((tag: string | null) => {
    pullingRef.current = tag;
    setPulling(tag);
  }, []);

  /** Mirror the backend's pull job into the view. The snapshot is the source of truth: it survives
   *  this view unmounting, and it is the only thing that knows about a download this component did
   *  not start. A snapshot with nothing running is deliberately NOT a reset — callers decide that. */
  const applyPullSnapshot = useCallback(
    (snap: PullSnapshot | null) => {
      if (!snap?.running) return false;
      markPulling(snap.model);
      setPullProg({
        status: snap.status,
        completed_bytes: snap.completed_bytes,
        total_bytes: snap.total_bytes,
        done: false,
      });
      return true;
    },
    [markPulling],
  );

  // A download owned by the BACKEND may be running while this view mounts (the tab router unmounts
  // on every switch): adopt it, and while any pull is marked running keep re-reading the snapshot —
  // it is the source of truth that survives the unmount, and its terminal state carries the error
  // a channel nobody was listening to could not deliver.
  useEffect(() => {
    let cancelled = false;
    void activeLocalPull()
      .then((snap) => {
        if (cancelled) return;
        applyPullSnapshot(snap);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [applyPullSnapshot]);
  useEffect(() => {
    if (pulling === null) return;
    let cancelled = false;
    const id = setInterval(() => {
      void activeLocalPull()
        .then((snap) => {
          if (cancelled || !snap || snap.model !== pulling) return;
          if (snap.running) {
            setPullProg({
              status: snap.status,
              completed_bytes: snap.completed_bytes,
              total_bytes: snap.total_bytes,
              done: false,
            });
            return;
          }
          // Terminal. The locally-started path also lands here if its invoke handler is gone.
          markPulling(null);
          setPullProg(null);
          if (snap.error) onError(snap.error);
          void onReload();
          void onRefreshRecs();
        })
        .catch(() => {});
    }, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pulling]);

  /** Download, once the terms behind this model have been shown and accepted (if they need to be).
   *
   *  Restricted-licence models — Gemma, Llama, the largest Qwen — carry publisher terms rather than
   *  an open-source licence, so PM shows them first. Acceptance is remembered per LICENCE, so
   *  reading the Gemma Terms once covers every Gemma. Open-licence models are never interrupted.
   *
   *  This is disclosure, not enforcement: the download is the user's own Ollama fetching the weights
   *  from the publisher, and they could run `ollama pull` without PM at all. */
  function requestPull(rec: LocalRecommendation, tag: string) {
    const needsTerms = !rec.licence.open && !(recs?.terms_accepted ?? []).includes(rec.licence.id);
    if (needsTerms) {
      setTermsFor({ rec, tag });
      return;
    }
    void pull(tag);
  }

  async function acceptTermsAndPull() {
    const pending = termsFor;
    if (!pending) return;
    const { rec, tag } = pending;
    setTermsFor(null);
    try {
      const accepted = await acceptLocalModelTerms(rec.licence.id);
      if (recs) onRecs({ ...recs, terms_accepted: accepted });
    } catch (e) {
      // The acceptance failed to persist, so the next download of this licence asks again. That is
      // the safe direction: never start the download on the back of a record that wasn't written.
      onError(String(e));
      return;
    }
    await pull(tag);
  }

  async function pull(tag: string) {
    markPulling(tag);
    setPullProg(null);
    onError(null);
    try {
      // The job itself is backend-owned (it survives this view unmounting); the channel is just
      // the low-latency progress feed while we ARE mounted — the 1s snapshot poll is the fallback.
      await pullLocalModel(tag, setPullProg);
      await onReload(); // the model now shows as served / installed
      await onRefreshRecs();
    } catch (e) {
      onError(String(e));
      // The job is backend-owned and the backend refuses a second concurrent pull, so this is the
      // ordinary outcome of clicking a second Download while one runs. The optimistic mark above has
      // already displaced whatever was running; recover it from the snapshot rather than dropping to
      // null, because null tears down the 1s poller (its dependency is `pulling`) and leaves a live
      // download with no progress bar and no Cancel until the view happens to remount.
      applyPullSnapshot(await activeLocalPull().catch(() => null));
    } finally {
      // Per-tag, never unconditional: the re-adoption above may have just put ANOTHER pull on
      // screen, and this reset runs after it.
      if (pullingRef.current === tag) {
        markPulling(null);
        setPullProg(null);
      }
    }
  }

  return (
    <div
      id="sec-localai-models"
      data-settings-section
      data-help="settings-localai-models"
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel
        align="baseline"
        action={
          !loading &&
          recs &&
          recs.curated.length > 0 && (
            <span className="shrink-0 text-[0.6875rem] text-ink4">
              {recs.curated.length} in the catalog
            </span>
          )
        }
      >
        Recommended models
      </SectionLabel>
      <Collapsible title="What do these numbers mean?" defaultOpen={false} className="mt-2">
        <NumbersGuide />
      </Collapsible>
      {loading ? (
        <p className="mt-3 text-xs text-ink4">Sizing models against your machine…</p>
      ) : recs && recs.curated.length > 0 ? (
        <div className="mt-3 max-h-80 space-y-2 overflow-y-auto pr-1">
          {recs.curated.map((rec) => (
            <RecommendationCard
              key={rec.repo}
              rec={rec}
              installed={installedRepos.has(rec.repo)}
              canPull={configured && isOllama}
              pullingTag={pulling}
              pullProg={pullProg}
              onPull={(tag) => requestPull(rec, tag)}
              servedTags={servedTags}
              onCancel={() => void cancelLocalPull().catch(() => {})}
              busy={pulling !== null}
            />
          ))}
        </div>
      ) : (
        <p className="mt-3 text-xs text-ink4">No catalog models to show.</p>
      )}
      <ConfirmDialog
        open={termsFor !== null}
        title={
          termsFor ? `${termsFor.rec.display_name} is under the ${termsFor.rec.licence.name}` : ""
        }
        confirmLabel="Accept and download"
        onConfirm={() => void acceptTermsAndPull()}
        onClose={() => setTermsFor(null)}
      >
        {termsFor && (
          <>
            <p>{termsFor.rec.licence.summary}</p>
            <p className="mt-2">
              <a
                href={termsFor.rec.licence.url}
                target="_blank"
                rel="noreferrer noopener"
                className="underline decoration-dotted underline-offset-2"
              >
                Read the full terms
              </a>
              .
            </p>
            <p className="mt-2 text-ink4">
              PM doesn't download the weights — your own Ollama fetches them from the publisher, and
              PM can't enforce these terms either way. Accepting here records that you've read them.
              PM won't ask again for another model under the same licence.
            </p>
          </>
        )}
      </ConfirmDialog>
      <p className="mt-3 text-xs text-ink4">
        Local models don't appear in Settings → AI &amp; Models → Usage &amp; cost — that ledger
        tracks only your paid cloud (OpenRouter) calls. Running a model on your own machine has no
        per-use cost to count.
      </p>
      {recs && (
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <label className="text-xs text-ink3" htmlFor="localai-cadence">
            Tell me when a better-fitting model appears
          </label>
          <Select
            id="localai-cadence"
            value={recs.cadence}
            onChange={(e) => onCadence(e.target.value)}
            className="w-auto text-xs"
          >
            <option value="on-catalog-update">When PM's model list is updated</option>
            <option value="weekly">Weekly</option>
            <option value="monthly">Monthly</option>
            <option value="manual">Never — I'll check myself</option>
          </Select>
        </div>
      )}
    </div>
  );
}

function NumbersGuide() {
  const items: Array<[string, string]> = [
    [
      "Fit",
      "Whether the model runs comfortably, only with a shrunk context, or is too big for now.",
    ],
    [
      "Quant",
      "How much the weights are compressed. Lower (e.g. Q4) is smaller and faster; higher (Q6/Q8) is more faithful but heavier.",
    ],
    [
      "Context",
      "How much text the model can consider at once. PM shrinks this to fit your memory when it has to, down to a floor.",
    ],
    [
      "q8_0 KV",
      "The running memory for the conversation is sized at f16 by default. When a card shows “q8_0 KV”, PM compressed that cache (near-lossless) so the model keeps a longer context or a higher-quality quant instead of shrinking either.",
    ],
    [
      "Speed",
      "A rough tokens-per-second estimate for how fast replies stream on your machine — higher is snappier.",
    ],
    [
      "Memory",
      "About how much RAM (or VRAM) the model needs loaded. It must sit under what you have free, with headroom.",
    ],
    [
      "MoE (mixture of experts)",
      "A large model where only a few billion parameters fire per word. It runs at the speed of that small active part, but its whole weight still has to fit in memory — so a MoE is fast for its size, not lighter to load. Cards show both the total and the active size.",
    ],
    [
      "Two ways to run (with a graphics card)",
      "When your best-quality fit is larger than your graphics memory, PM also shows a faster config that fits inside your GPU — usually a smaller quant and a shorter context, but replies stream much quicker. Both are yours to choose; PM never switches for you.",
    ],
  ];
  return (
    <dl className="mt-1 space-y-1.5 text-xs">
      {items.map(([k, v]) => (
        <div key={k}>
          <dt className="inline font-medium text-ink2">{k}: </dt>
          <dd className="inline text-ink4">{v}</dd>
        </div>
      ))}
      <p className="pt-1 text-ink4">
        Numbers are estimates. Memory assumes an f16 KV cache by default; where a card shows “q8_0
        KV”, PM sized it on a compressed (near-lossless) cache to keep a larger context or quant.
        Your real speed and memory depend on your runner and settings.
      </p>
    </dl>
  );
}

/** How to get a model PM can't download for you.
 *
 *  Honest per runner rather than one command pretending to be universal: the three name models three
 *  different ways, and the same weights are `qwen2.5:7b-instruct-q4_K_M` to Ollama, `…@q4_k_m` to LM
 *  Studio and `user/repo:Q4_K_M` to llama-server. Pasting one into another gets you nothing. So PM
 *  prints the command it can stand behind and describes the route for the two it can't. */
function ModelInstallHint({
  repo,
  quant,
  rungs,
  shardedQuant,
}: {
  repo: string;
  quant: string | null;
  /** Every way to run this model that PM can name, one per rung the card shows. A split card offers
   *  two genuinely different files; printing only one of them is what stranded the GPU rung. */
  rungs: { label: string; tag: string }[];
  shardedQuant: boolean;
}) {
  const cmd = installCommand("llama-server", repo, quant);
  return (
    <div className="mt-2 space-y-1.5">
      {cmd && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
            {cmd}
          </code>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void navigator.clipboard?.writeText(cmd)}
          >
            Copy
          </Button>
        </div>
      )}
      {rungs.map((r) => (
        <div key={r.tag} className="flex items-center gap-2">
          {rungs.length > 1 && (
            <span className="shrink-0 text-[0.625rem] text-ink4">{r.label}</span>
          )}
          <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
            {`ollama pull ${r.tag}`}
          </code>
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => void navigator.clipboard?.writeText(`ollama pull ${r.tag}`)}
          >
            Copy
          </Button>
        </div>
      ))}
      <p className="text-[0.6875rem] text-ink4">
        That command downloads and serves it in one step. In LM Studio, paste{" "}
        <span className="font-mono text-ink3">{repo}</span> into the Discover tab's search.
        {shardedQuant
          ? " Ollama can't fetch this quantization — it ships as split files, which Ollama won't pull. A smaller one of the same model will work."
          : ""}
      </p>
    </div>
  );
}

function RecommendationCard({
  rec,
  installed,
  canPull,
  pullingTag,
  pullProg,
  onPull,
  onCancel,
  busy,
  servedTags,
}: {
  rec: LocalRecommendation;
  installed: boolean;
  /** Whether PM can drive a download at all here — an Ollama endpoint is connected. Card-level;
   *  whether a given RUNG has something to fetch is a separate question, answered per rung. */
  canPull: boolean;
  /** The tag downloading right now, anywhere in the list, or null. */
  pullingTag: string | null;
  pullProg: PullProgress | null;
  onPull: (tag: string) => void;
  onCancel: () => void;
  busy: boolean;
  /** Model ids the endpoint already serves, lower-cased. An `hf.co/...` pull is served under the
   *  tag it was pulled with (measured against a live Ollama 0.33), so a rung can be matched exactly
   *  rather than by repo — which said "Installed" for a quant that was neither rung.  */
  servedTags: Set<string>;
}) {
  const f = rec.fit;
  const ramTarget = { tag: rec.ollama_pull, sharded: rec.sharded_quant };
  const gpuTarget = rec.gpu_pull;
  const isSplit = rec.gpu.kind === "split";
  const pulling =
    pullingTag !== null && (pullingTag === rec.ollama_pull || pullingTag === gpuTarget?.tag);

  /** One rung's own action: it is already here, PM can fetch it, or neither (the commands below). */
  const rungAction = (t: { tag: string | null } | null): ReactNode => {
    const tag = t?.tag ?? null;
    if (!tag) return null;
    if (servedTags.has(tag.toLowerCase()))
      return <span className="text-[0.625rem] font-medium text-st-quick">Installed</span>;
    if (!canPull) return null;
    return (
      <Button variant="secondary" size="sm" onClick={() => onPull(tag)} disabled={busy}>
        {pullingTag === tag ? "Downloading\u2026" : "Download"}
      </Button>
    );
  };
  // MoE when fewer params are active per token than the model holds (matches the catalog's own rule).
  const isMoe = rec.active_parameters_b + 0.01 < rec.parameters_b;
  const pct =
    pullProg && pullProg.total_bytes
      ? Math.min(100, Math.round((100 * (pullProg.completed_bytes ?? 0)) / pullProg.total_bytes))
      : null;
  return (
    <div className="rounded-[var(--radius-sm)] border border-border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-sm font-medium text-ink">{rec.display_name}</span>
            <FitBadge verdict={f.verdict} />
            {isMoe && <span className="text-[0.625rem] text-ink4">MoE</span>}
            {/* No "vision" chip, though `rec.multimodal` still says so. This row is what PM will do
                with the model, and PM cannot send it an image: chat messages carry a plain string,
                so no picture reaches any model, cloud or local. PM reads images through the sidecar
                instead. Advertising it here made a heavier model look more capable for PM's purposes
                than a lighter one, which is the opposite of true. A chip has no room for the caveat,
                which is itself the argument for leaving it out. */}
            {rec.reasoning && <span className="text-[0.625rem] text-ink4">reasoning</span>}
            {rec.role_hint && (
              <span className="text-[0.625rem] text-ink4">suits {rec.role_hint}</span>
            )}
            {/* Every row says what its weights are under. A restricted licence is the one worth
                catching the eye, so it takes the attention colour the rest of the chips don't. */}
            <a
              href={rec.licence.url}
              target="_blank"
              rel="noreferrer noopener"
              className={`text-[0.625rem] underline decoration-dotted underline-offset-2 ${
                rec.licence.open ? "text-ink4" : "text-st-due"
              }`}
            >
              {rec.licence.name}
            </a>
          </div>
          {rec.gpu.kind === "split" ? (
            <div className="mt-1 space-y-1">
              <div className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
                <span>
                  {rec.parameters_b}B{isMoe ? " total" : ""}
                </span>
                {isMoe && <span>{rec.active_parameters_b}B active</span>}
              </div>
              <p className="text-[0.625rem] text-ink4">
                Two ways to run it here — same model, lighter settings for speed:
              </p>
              {/* Each rung carries its OWN action, because each names its own file. The card held
                  one button wired to the Highest-quality rung, plus a caption admitting the faster
                  rung could not be fetched — which was also FALSE whenever the two rungs differ
                  only in context or KV precision, a split `gpu_fit` produces by design. */}
              <ConfigRow label="Highest quality" fit={f} action={rungAction(ramTarget)} />
              <ConfigRow
                label="Fastest on GPU"
                fit={rec.gpu.fit}
                action={gpuTarget?.same_file ? undefined : rungAction(gpuTarget)}
              />
              {gpuTarget?.same_file ? (
                <p className="text-[0.625rem] text-ink4">
                  Both rows are the same file — the difference is the settings PM runs it with.
                </p>
              ) : (
                gpuTarget?.sharded && (
                  <p className="text-[0.625rem] text-ink4">
                    PM can't fetch the Fastest-on-GPU file: that quant ships as split parts, which
                    Ollama's download route refuses. The command below still works.
                  </p>
                )
              )}
            </div>
          ) : (
            <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
              <span>
                {rec.parameters_b}B{isMoe ? " total" : ""}
              </span>
              {isMoe && <span>{rec.active_parameters_b}B active</span>}
              {f.quant && <span>{f.quant}</span>}
              {f.context != null && <span>{(f.context / 1024).toFixed(0)}k ctx</span>}
              {f.kv === "q8_0" && <span>q8_0 KV</span>}
              {f.est_tokens_per_sec != null && (
                <span>~{f.est_tokens_per_sec.toFixed(0)} tok/s</span>
              )}
              {f.est_memory_gb != null && <span>{formatGib(f.est_memory_gb)}</span>}
            </div>
          )}
        </div>
        <div className="shrink-0">
          {/* A split card's actions live on its rows, beside the config each one fetches. */}
          {isSplit ? null : installed ? (
            <span className="text-xs font-medium text-st-quick">Installed</span>
          ) : canPull && rec.ollama_pull ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => rec.ollama_pull && onPull(rec.ollama_pull)}
              disabled={busy || f.verdict === "stay_on_cloud"}
            >
              {pulling ? "Downloading\u2026" : "Download"}
            </Button>
          ) : null}
        </div>
      </div>

      {pulling && (
        <div className="mt-2">
          {/* The shared per-depth progress surface: shimmer while the total is unknown (the
              manifest/verify phases used to render a FULL bar, which reads as "done"), percent
              once bytes flow. The status line stays — it is a status readout, never folded. */}
          <IngestProgress
            processed={pct ?? 0}
            total={pct != null ? 100 : null}
            label={`Downloading ${rec.display_name}`}
            mode="percent"
          />
          <div className="mt-1 flex items-center justify-between gap-2">
            <p className="min-w-0 truncate font-mono text-[0.625rem] text-ink4">
              {pullProg?.status ?? "starting…"}
              {pullProg?.total_bytes
                ? ` · ${formatBytes(pullProg.completed_bytes)} / ${formatBytes(pullProg.total_bytes)}`
                : ""}
            </p>
            <Button variant="tertiary" size="sm" onClick={onCancel}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {!installed &&
        (() => {
          // Every way to get this model that PM can name. This block used to be DELETED the moment
          // an Ollama endpoint connected — taking the `llama-server` line, which is for a
          // different runner entirely, with it, and on a split card removing the only route to the
          // second rung at exactly the moment the user had finished setting PM up. It now always
          // exists; it just folds away once PM can do the work for you.
          const rungs = [
            { label: "Highest quality", tag: rec.ollama_pull },
            ...(isSplit && !gpuTarget?.same_file
              ? [{ label: "Fastest on GPU", tag: gpuTarget?.tag ?? null }]
              : []),
          ].filter((r): r is { label: string; tag: string } => !!r.tag);
          const hint = (
            <ModelInstallHint
              repo={rec.repo}
              quant={f.quant}
              rungs={rungs}
              shardedQuant={rec.sharded_quant}
            />
          );
          return canPull ? (
            <Collapsible title="Install it another way" defaultOpen={false} className="mt-2">
              {hint}
            </Collapsible>
          ) : (
            hint
          );
        })()}

      {rec.gpu.kind === "split"
        ? // Each Split row states its own caveat (and KV chip) via ConfigRow — nothing shared to add.
          null
        : f.notes.length > 0 && (
            <p className="mt-1.5 text-[0.6875rem] text-ink4">{f.notes.join(" ")}</p>
          )}
    </div>
  );
}
