// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

import type { LocalRecommendation, LocalRecommendations, PullProgress } from "../../lib/types";
import { formatBytes, formatGib } from "../../lib/format";
import { IngestProgress } from "../IngestProgress";
import { installCommand } from "../../lib/workbenchGuide";
import { ConfigRow, FitBadge } from "./fitDisplay";
import { sectionLabel } from "./sections";
import { SPEED_LIST_NOTE, speedShort } from "./speedWords";
import type { ModelPull } from "./usePull";
import { Button, Callout, Collapsible, SectionLabel, Select } from "../ui";

/**
 * "Recommended models" — the curated catalog sized against this machine, and the one-click pull.
 *
 * The download itself is the tab's (`usePull`), handed down as `pull`: the job is backend-owned (it
 * survives the tab unmounting), and what this section shows is the view of it — which card is
 * marked, and what the progress bar says. The licence dialog that has to be answered before a
 * restricted model is fetched is the hook's too, and renders once, at the tab.
 */
export function LocalAiCatalog({
  recs,
  loading,
  configured,
  isOllama,
  servedTags,
  installedRepos,
  pull,
  onCadence,
  error,
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  configured: boolean;
  /** Whether the connected server is an Ollama — the only runner PM can pull into. */
  isOllama: boolean;
  servedTags: Set<string>;
  installedRepos: Set<string>;
  /** The one model download, from the tab's `usePull`. */
  pull: ModelPull;
  onCadence: (cadence: string) => void;
  /** Something in this section went wrong — a download it asked for, the cadence — said here
   *  rather than at the top of the tab. */
  error?: string | null;
}) {
  const { pulling, pullProg } = pull;

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
        {sectionLabel("sec-localai-models")}
      </SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}
      {/* Never folded: it is what every speed on the cards below it is, and a ceiling read as a
          forecast is the misreading it exists to stop. */}
      <p className="mt-1.5 text-xs text-ink4">{SPEED_LIST_NOTE}</p>
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
              onPull={(tag) => pull.requestPull(rec, tag, "models")}
              servedTags={servedTags}
              onCancel={pull.cancel}
              busy={pulling !== null}
            />
          ))}
        </div>
      ) : (
        <p className="mt-3 text-xs text-ink4">No catalog models to show.</p>
      )}
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
      "How fast replies stream, in tokens a second (a token is roughly three-quarters of a word). It's an estimate, not a measurement: PM divides the memory speed of whatever the model runs from by how much of the model it reads for each token. On a graphics card PM recognises, that makes it a ceiling — in PM's own checks on one laptop graphics card, real replies came 10–35% slower. From system memory it's a rough guide in either direction, and on chips that share memory with the processor PM doesn't estimate it yet.",
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
        Memory figures are designed to run a little high — about 11% above a real load when PM
        measured one — so a model PM says fits should fit. Memory assumes an f16 cache unless a card
        shows “q8_0 KV”, where PM sized it on a compressed (near-lossless) cache to keep a larger
        context or quant — your server needs that setting too (
        {sectionLabel("sec-localai-endpoint")}, “Settings PM's numbers assume”). Your real speed and
        memory depend on your server and its settings.
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
              {speedShort(f) && <span>{speedShort(f)}</span>}
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
