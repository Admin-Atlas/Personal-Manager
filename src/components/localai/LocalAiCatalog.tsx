// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

import type {
  LocalFitResult,
  LocalPick,
  LocalRecommendation,
  LocalRecommendations,
  PullProgress,
} from "../../lib/types";
import { formatBytes, formatGib } from "../../lib/format";
import { useDepth } from "../../theme";
import { IngestProgress } from "../IngestProgress";
import type { RunnerName } from "../../lib/workbenchGuide";
import { ConfigRow, FitBadge, TokenChip } from "./fitDisplay";
import { recCardId, TUNING_TITLE, useLocate } from "./locate";
import { hfServeCommand } from "./readiness";
import { sectionHelp, sectionLabel } from "./sections";
import { SPEED_LIST_NOTE, speedShort } from "./speedWords";
import type { ModelPull } from "./usePull";
import { Button, Callout, Collapsible, SectionInfo, SectionLabel, Select } from "../ui";

/**
 * "All models" — every model in PM's list sized against this machine, in the backend's order, and the
 * one-click pull.
 *
 * Folded behind "Show all … models": the start card carries PM's pick, and this is the list to choose
 * from yourself. What is never folded is what decides how to read it — why there is no Download here
 * (the gating hint), and what every speed figure is.
 *
 * The download itself is the tab's (`usePull`), handed down as `pull`: the job is backend-owned (it
 * survives the tab unmounting), and what this section shows is the view of it — which card is
 * marked, and what the progress bar says. The licence dialog that has to be answered before a
 * restricted model is fetched is the hook's too, and renders once, at the tab.
 *
 * One Download per tag: while the start card's step 2 offers PM's pick, a rung here with the pick's
 * file offers no second button for it — and a download's progress shows once, on the start card for
 * the pick, on its card otherwise. Only a rung that IS the pick — its file, context and cache — says
 * "PM's pick": the pick is judged at the context PM sizes it for and the cards at the model's
 * trained one, so the same file is often on a rung here at another config, and the card's band line
 * says what the pick is instead. While the list is folded, the progress a card would show is shown
 * above the fold instead: the tab unmounts on every switch and the fold comes back closed, and a
 * multi-gigabyte download with its only Cancel inside a closed fold is a download nothing on the
 * page says is running.
 */
export function LocalAiCatalog({
  recs,
  loading,
  configured,
  isOllama,
  runner = null,
  servedTags,
  installedRepos,
  pull,
  pickDownloadTag = null,
  pickProgressShown = false,
  open,
  onOpenChange,
  onCadence,
  error,
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  configured: boolean;
  /** Whether the connected server is an Ollama — the only runner PM can pull into. */
  isOllama: boolean;
  /** Which server is connected, by its port, for the gating hint. */
  runner?: RunnerName | null;
  servedTags: Set<string>;
  installedRepos: Set<string>;
  /** The one model download, from the tab's `usePull`. */
  pull: ModelPull;
  /** The tag the start card's step 2 is offering to download right now, or null. */
  pickDownloadTag?: string | null;
  /** The start card is showing the pick's download progress, so its card doesn't too. */
  pickProgressShown?: boolean;
  /** The "Show all … models" fold, held by the tab so a pointer can open it — and so this section
   *  knows whether a card's progress can be seen. */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCadence: (cadence: string) => void;
  /** Something in this section went wrong — a download it asked for, the cadence — said here
   *  rather than at the top of the tab. */
  error?: string | null;
}) {
  const { pulling, pullProg } = pull;
  const { showMeta } = useDepth();
  const locate = useLocate();
  const pick = recs?.pick;
  const count = recs?.curated.length ?? 0;
  // A running download's progress is this section's unless the start card is showing it (PM's pick,
  // while step 2 is on it). It goes on the card that offers the file while the list is open, and
  // above the fold while it is closed — or always, for a download no card here offers.
  const progressHere =
    pulling !== null && !(pickProgressShown && pick?.kind === "catalogue" && pick.tag === pulling);
  const pulledRec =
    pulling === null
      ? null
      : (recs?.curated.find((r) => r.ollama_pull === pulling || r.gpu_pull?.tag === pulling) ??
        null);
  const progressAboveFold = progressHere && (!open || pulledRec === null);
  // Never folded: why the list offers no Download is a gating hint, and the doctrine never folds
  // those.
  const gating = !configured
    ? "PM can download these straight into Ollama once it's connected. Until then, each model's “How to get it” has the commands for your own server."
    : !isOllama
      ? `PM can only download into an Ollama on its usual port (11434), and your server ${
          runner === "LM Studio"
            ? "is LM Studio"
            : runner === "llama-server"
              ? "is llama-server"
              : "isn't one"
        }, so each model's “How to get it” has the steps instead.`
      : null;

  return (
    <div
      id="sec-localai-models"
      data-settings-section
      data-help={sectionHelp("sec-localai-models")}
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel
        align="baseline"
        action={
          showMeta &&
          !loading &&
          count > 0 && (
            <span className="shrink-0 text-[0.6875rem] text-ink4">
              {count} sized for this machine
            </span>
          )
        }
      >
        {sectionLabel("sec-localai-models")}
      </SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}
      {gating && <p className="mt-1.5 text-xs text-ink4">{gating}</p>}
      {/* Never folded: it is what every speed on the cards is, and an estimate read as a measurement
          on this computer is the misreading it exists to stop. */}
      <p className="mt-1.5 text-xs text-ink4">{SPEED_LIST_NOTE}</p>
      <Collapsible title="What do these numbers mean?" defaultOpen={false} className="mt-2">
        <NumbersGuide />
      </Collapsible>
      {/* Never folded: a status readout, and the only Cancel there is while the list is closed. */}
      {progressAboveFold && pulling !== null && (
        <PullProgressRow
          name={pulledRec?.display_name ?? pulling}
          pullProg={pullProg}
          startedAt={pull.startedAt}
          onCancel={pull.cancel}
          action={
            pulledRec && (
              <Button
                variant="tertiary"
                size="sm"
                onClick={() => locate?.(`rec:${pulledRec.repo}`)}
              >
                Show it
              </Button>
            )
          }
        />
      )}
      {loading ? (
        <p className="mt-3 text-xs text-ink4">Sizing models against your machine…</p>
      ) : recs && count > 0 ? (
        <Collapsible
          title={
            // "Show all 1 models" would read like a typo; one model is just "Show the model".
            (count === 1 ? "Show the model" : `Show all ${count} models`) +
            (pick?.kind === "nothing" ? " anyway" : "")
          }
          defaultOpen={false}
          open={open}
          onOpenChange={onOpenChange}
          className="mt-3"
        >
          {/* No inner scroller: one scrolling pane, so the wheel and the rail's scroll-spy both
              move the whole tab. */}
          <div className="mt-2 space-y-2">
            {recs.curated.map((rec) => (
              <RecommendationCard
                key={rec.repo}
                rec={rec}
                installed={rec.ollama_pull != null && servedTags.has(rec.ollama_pull.toLowerCase())}
                otherBuild={otherBuild(rec, recs, servedTags, installedRepos)}
                canPull={configured && isOllama}
                pullingTag={pulling}
                pullProg={pullProg}
                startedAt={pull.startedAt}
                progressHere={progressHere && !progressAboveFold}
                onPull={(tag) => pull.requestPull(rec, tag, "models")}
                servedTags={servedTags}
                onCancel={pull.cancel}
                busy={pulling !== null}
                pickTag={pickDownloadTag}
                pickFit={pick?.kind === "catalogue" && pick.repo === rec.repo ? pick.fit : null}
                band={bandLine(rec, pick)}
              />
            ))}
          </div>
        </Collapsible>
      ) : (
        <p className="mt-3 text-xs text-ink4">No catalog models to show.</p>
      )}
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
      <SectionInfo title="Does a local model cost anything?">
        <p>
          Local models don't appear in Settings → AI &amp; Models → Usage &amp; cost — that ledger
          tracks only your paid cloud (OpenRouter) calls. Running a model on your own machine has no
          per-use cost to count.
        </p>
      </SectionInfo>
    </div>
  );
}

/** "Your server has X, another build of this model": the server serves this repo, but neither of
 *  the files this card offers — so it keeps its Download, and says what is there instead. */
function otherBuild(
  rec: LocalRecommendation,
  recs: LocalRecommendations,
  servedTags: Set<string>,
  installedRepos: Set<string>,
): string | null {
  if (!installedRepos.has(rec.repo)) return null;
  const tags = [rec.ollama_pull, rec.gpu_pull?.tag]
    .filter((t): t is string => !!t)
    .map((t) => t.toLowerCase());
  if (tags.some((t) => servedTags.has(t))) return null;
  const id = recs.installed.find((m) => m.matched_repo === rec.repo)?.id;
  return id ? `Your server has ${id}, another build of this model.` : null;
}

/** One way to run a model: its file (the quant), its context and its cache. */
type RunConfig = Pick<LocalFitResult, "quant" | "context" | "kv">;

/** The same way to run a model — the file, the context and the cache all agree. A tag names only
 *  the file, so two rungs (or a rung and the pick) can share one and still run it differently. */
function sameConfig(a: RunConfig, b: RunConfig): boolean {
  return a.quant === b.quant && a.context === b.context && a.kv === b.kv;
}

/** The configs a card's rungs show: the highest-quality one, and a split's GPU one. */
function rungFits(rec: LocalRecommendation): RunConfig[] {
  return rec.gpu.kind === "split" ? [rec.fit, rec.gpu.fit] : [rec.fit];
}

/** "32k", from a token count. */
function kTokens(n: number): string {
  return `${(n / 1024).toFixed(0)}k`;
}

/**
 * PM's pick is this model at a config no rung shows, so the card says what the pick is and where.
 *
 * Matched on the config, never the tag: the pick is judged at the context PM sizes it for (32k, or
 * less for a model made for less, better_fit.rs `pick_context`) and the cards at the model's trained
 * one, so the pick's own file is often on a rung here at a longer context and a compressed cache —
 * the dev laptop's pick, gemma 4 12b, is Q3_K_M at 32k on f16, and the same Q3_K_M file at 64k on
 * q8_0 is the card's GPU rung.
 *
 * The reason is the one that really separates them. When a rung has the pick's file, or the pick is
 * the highest-quality build at its own context (`rung: "quality"`), it is the context; otherwise it
 * is the step down `pick.rung` says PM took — to keep the room it leaves free on the card ("gpu"), to
 * be quick enough for chat on the card ("chat"), or to be quick enough from memory ("speed"). A pick
 * that matches no rung at the model's full context differs only in being a build Ollama can fetch,
 * which the card's best file need not be.
 */
function bandLine(rec: LocalRecommendation, pick: LocalPick | undefined): string | null {
  if (pick?.kind !== "catalogue" || pick.repo !== rec.repo) return null;
  const rungs = rungFits(rec);
  if (rungs.some((f) => sameConfig(f, pick.fit))) return null;
  const { quant, context, kv } = pick.fit;
  const sameFile = quant != null && rungs.some((f) => f.quant === quant);
  const capped = context != null && context < rec.context_length;
  const build = sameFile
    ? `this card's ${quant} file`
    : `this model as ${quant ?? "another build"}`;
  const cache = (k: LocalFitResult["kv"]) =>
    k === "q8_0" ? "a compressed (q8_0) cache" : "an f16 cache";
  const as = `PM's pick is ${build}${context != null ? ` at a ${kTokens(context)} context` : ""} on ${cache(kv)}`;
  const fitsWhat = pick.basis === "gpu" ? "your graphics card" : "your free memory";
  const why =
    capped && (sameFile || pick.rung === "quality")
      ? `the build that fits ${fitsWhat} at the context PM sizes it for`
      : pick.rung === "gpu"
        ? "so it fits your graphics card with the room PM keeps free"
        : pick.rung === "chat"
          ? "the build quick enough for chat on your graphics card"
          : pick.rung === "speed"
            ? "the build quick enough for PM's background work from memory"
            : "the best build of it that fits and that Ollama can download";
  // What the card itself shows, so the comparison names a figure the reader can see: the row with
  // the pick's file, or else how the card sizes the model as a whole.
  // With both rows on the same file, the one that runs where the pick does: on a card, its row.
  const row = sameFile
    ? (pick.basis === "gpu" ? [...rungs].reverse() : rungs).find((f) => f.quant === quant)
    : undefined;
  const full = !capped
    ? ""
    : row?.context != null
      ? ` — this card's ${quant} row is sized for a ${kTokens(row.context)} context on ${cache(row.kv)}`
      : ` — this card sizes the model for as much of its ${kTokens(rec.context_length)} context as fits`;
  return `${as}, ${why}${full}. It's under ${sectionLabel("sec-localai-start")}.`;
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
      "How fast replies stream, in tokens a second (a token is roughly three-quarters of a word). It's an estimate, not a measurement on this computer. On a graphics card PM divides the card's memory speed by how much of the model it reads for each token, worked out from the model file, and scales that by how fast ten builds of eight models really ran on one laptop graphics card PM tested: there it came within about 12% of all but one of them, and within about a quarter of that one (gemma 3 4b, which ran slower than PM expected). That laptop runs its card on a reduced power budget for most replies, so on a desktop card, or a laptop that keeps its card at full power, replies may well come faster than this: at full power, that laptop's own card was about 1.2 to 1.4 times as fast. A long conversation replies slower than a fresh one. From system memory it's a rough guide in either direction, and on chips that share memory with the processor PM doesn't estimate it yet.",
    ],
    [
      "Memory",
      "About how much RAM (or VRAM) the model needs loaded. It must sit under what you have free, with headroom.",
    ],
    [
      "MoE (mixture of experts)",
      "A large model where only a few billion parameters fire per word. That makes it quicker than an ordinary model of its full size, but its whole weight still has to fit in memory — so a MoE is fast for its size, not lighter to load. On a graphics card it isn't as quick as an ordinary model the size of its active part: going by one published report, PM halves its estimate for a MoE there, since PM hasn't timed one itself — so take that figure with a pinch of salt. Cards show both the total and the active size.",
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
        Memory figures are designed never to come in under a real load — between about 2% and 11%
        above it in PM's checks — so a model PM says fits should fit. Memory assumes an f16 cache
        unless a card shows “q8_0 KV”, where PM sized it on a compressed (near-lossless) cache to
        keep a larger context or quant — your server needs that setting too, and the context the
        card shows. Each model's commands say what to set, and once your server is connected,{" "}
        {sectionLabel("sec-localai-endpoint")}'s “{TUNING_TITLE}” has the steps. Your real speed and
        memory depend on your server and its settings.
      </p>
    </dl>
  );
}

/** One way to run a model the card shows: its label, the Ollama tag that fetches it (null when PM
 *  has no tag to give), and the config PM sized it at. */
interface Rung {
  label: string;
  tag: string | null;
  fit: RunConfig;
}

/** How to get a model PM can't download for you.
 *
 *  Honest per runner rather than one command pretending to be universal: the three name models three
 *  different ways, and the same weights are `qwen2.5:7b-instruct-q4_K_M` to Ollama, `…@q4_k_m` to LM
 *  Studio and `user/repo:Q4_K_M` to llama-server. Pasting one into another gets you nothing. So PM
 *  prints the command it can stand behind and describes the route for the two it can't.
 *
 *  llama-server takes a Hugging Face repo id directly (`-hf <user>/<model>[:quant]`, its documented
 *  form). LM Studio's `lms get` documentation never says it accepts one, so PM points at the Discover
 *  tab, which does. Ollama's line is the catalogue's own `hf.co/<repo>:<QUANT>` tag, written by the
 *  generator only after it checked the registry, and never composed here: a tag made up in the view
 *  would be a guess wearing a verified tag's clothes.
 *
 *  And each says the config PM sized it at, because that is what the card's figures are for.
 *  llama-server's line carries it (the same `hfServeCommand` the start card prints): without
 *  `--ctx-size`, a current build loads the model's whole trained context, which is not what the card
 *  measured. An `ollama pull` can't carry it — Ollama runs a model at the context it is set to — so
 *  the hint says the number to set instead. */
function ModelInstallHint({
  repo,
  rungs,
  shardedQuant,
  pickElsewhere,
}: {
  repo: string;
  /** Every way to run this model that PM can name, one per rung the card shows. A split card offers
   *  two genuinely different configs; printing only one of them is what stranded the GPU rung. */
  rungs: Rung[];
  shardedQuant: boolean;
  /** PM's pick is this model at a config no rung shows (the band line), so these commands are the
   *  card's, and say so rather than read as the pick's. */
  pickElsewhere: boolean;
}) {
  // One llama-server line per distinct config: a split whose rungs share a file still runs it two
  // ways, at two contexts or caches, and those are different commands.
  const serve: { label: string; cmd: string }[] = [];
  for (const r of rungs) {
    const cmd = hfServeCommand(repo, r.fit);
    if (!serve.some((x) => x.cmd === cmd)) serve.push({ label: r.label, cmd });
  }
  const pulls = rungs.filter((r): r is Rung & { tag: string } => !!r.tag);
  const { ollama, lmStudio } = settingsFor(rungs);
  return (
    <div className="mt-2 space-y-1.5">
      {serve.map((x) => (
        <CommandRow key={x.cmd} label={serve.length > 1 ? x.label : null} cmd={x.cmd} />
      ))}
      {pulls.map((r) => (
        <CommandRow
          key={r.tag}
          label={pulls.length > 1 ? r.label : null}
          cmd={`ollama pull ${r.tag}`}
        />
      ))}
      <p className="text-[0.6875rem] text-ink4">
        {serve.length > 1 ? "Each llama-server line downloads" : "The llama-server line downloads"}{" "}
        the model and serves it in one step, with the settings PM sized it for.
        {ollama &&
          pulls.length > 0 &&
          ` An ollama pull only fetches the file: Ollama runs every model at the one context it is set to, so ${ollama} for it to run the way PM sized it.`}{" "}
        In LM Studio, paste <span className="font-mono text-ink3">{repo}</span> into the Discover
        tab's search{lmStudio ? `, and ${lmStudio}` : ""}.
        {shardedQuant
          ? " Ollama can't fetch this quantization — it ships as split files, which Ollama won't pull. A smaller one of the same model will work."
          : ""}
        {pickElsewhere &&
          ` These are for this card's ${rungs.length > 1 ? "rows" : "settings"}, not PM's pick, which is sized differently — its own steps are under ${sectionLabel("sec-localai-start")}.`}
      </p>
    </div>
  );
}

/** What Ollama and LM Studio need set for the rungs to run as sized — the context, and the
 *  compressed cache where PM sized on one — or nulls when every rung is at what a server starts with
 *  (a 4k context on an f16 cache). One number when the rungs agree; the rows, when they don't. */
function settingsFor(rungs: Rung[]): { ollama: string | null; lmStudio: string | null } {
  const changes = rungs.some((r) => r.fit.kv === "q8_0" || (r.fit.context ?? 0) > 4096);
  if (!changes || rungs.length === 0) return { ollama: null, lmStudio: null };
  const [first] = rungs;
  const uniform = rungs.every(
    (r) => r.fit.context === first.fit.context && r.fit.kv === first.fit.kv,
  );
  const lmCache = "the K and V cache quantization at Q8_0 (with Flash Attention on)";
  if (!uniform)
    return {
      ollama:
        "set that to the context on the row you choose, and its cache to q8_0 if the row shows “q8_0 KV”,",
      lmStudio: `load it with the context on the row you choose — and, if the row shows “q8_0 KV”, ${lmCache}`,
    };
  const { context, kv } = first.fit;
  const q8 = kv === "q8_0";
  if (context == null)
    return q8
      ? { ollama: "set its cache to q8_0", lmStudio: `load it with ${lmCache}` }
      : { ollama: null, lmStudio: null };
  return {
    ollama: `set that to ${context}${q8 ? " and its cache to q8_0" : ""}`,
    lmStudio: `load it with a context length of ${context}${q8 ? ` and ${lmCache}` : ""}`,
  };
}

/** A command to copy, with the rung it is for when the card has more than one. */
function CommandRow({ label, cmd }: { label: string | null; cmd: string }) {
  return (
    <div className="flex items-center gap-2">
      {label && <span className="shrink-0 text-[0.625rem] text-ink4">{label}</span>}
      <code className="min-w-0 flex-1 truncate rounded-[var(--radius-sm)] bg-surface px-2 py-1 font-mono text-[0.6875rem] text-ink3">
        {cmd}
      </code>
      <Button variant="tertiary" size="sm" onClick={() => void navigator.clipboard?.writeText(cmd)}>
        Copy
      </Button>
    </div>
  );
}

/** A download's progress, its status and its Cancel — on its card, or above the folded list. */
function PullProgressRow({
  name,
  pullProg,
  startedAt,
  onCancel,
  action,
}: {
  name: string;
  pullProg: PullProgress | null;
  startedAt: number | null;
  onCancel: () => void;
  /** Beside Cancel: a way to the card, when it is folded away. */
  action?: ReactNode;
}) {
  const { showMeta } = useDepth();
  const pct =
    pullProg && pullProg.total_bytes
      ? Math.min(100, Math.round((100 * (pullProg.completed_bytes ?? 0)) / pullProg.total_bytes))
      : null;
  return (
    <div className="mt-2">
      {/* The shared per-depth progress surface: shimmer while the total is unknown (the
          manifest/verify phases used to render a FULL bar, which reads as "done"), percent once
          bytes flow. The status word stays at every Depth — it is a status readout, never folded;
          the byte counts are detail. */}
      <IngestProgress
        processed={pct ?? 0}
        total={pct != null ? 100 : null}
        label={`Downloading ${name}`}
        mode="percent"
        startedAt={startedAt ?? undefined}
      />
      <div className="mt-1 flex items-center justify-between gap-2">
        <p className="min-w-0 truncate font-mono text-[0.625rem] text-ink4">
          {pullProg?.status ?? "starting…"}
          {showMeta && pullProg?.total_bytes
            ? ` · ${formatBytes(pullProg.completed_bytes)} / ${formatBytes(pullProg.total_bytes)}`
            : ""}
        </p>
        <span className="flex shrink-0 items-center gap-1.5">
          {action}
          <Button variant="tertiary" size="sm" onClick={onCancel}>
            Cancel
          </Button>
        </span>
      </div>
    </div>
  );
}

function RecommendationCard({
  rec,
  installed,
  otherBuild,
  canPull,
  pullingTag,
  pullProg,
  startedAt,
  progressHere,
  onPull,
  onCancel,
  busy,
  servedTags,
  pickTag,
  pickFit,
  band,
}: {
  rec: LocalRecommendation;
  /** The card's own file is being served — the exact tag, not just the model. */
  installed: boolean;
  /** The server has this model, but as a build neither rung offers. */
  otherBuild: string | null;
  /** Whether PM can drive a download at all here — an Ollama endpoint is connected. Card-level;
   *  whether a given RUNG has something to fetch is a separate question, answered per rung. */
  canPull: boolean;
  /** The tag downloading right now, anywhere in the list, or null. */
  pullingTag: string | null;
  pullProg: PullProgress | null;
  startedAt: number | null;
  /** This card shows the running download's progress — not the start card (the pick's), and not the
   *  line above the folded list. */
  progressHere: boolean;
  onPull: (tag: string) => void;
  onCancel: () => void;
  busy: boolean;
  /** Model ids the endpoint already serves, lower-cased. An `hf.co/...` pull is served under the
   *  tag it was pulled with (measured against a live Ollama 0.33), so a rung can be matched exactly
   *  rather than by repo — which said "Installed" for a quant that was neither rung.  */
  servedTags: Set<string>;
  /** The tag the start card is offering to download as PM's pick, or null. */
  pickTag: string | null;
  /** The config PM's pick is sized at, when the pick is this model; null otherwise. */
  pickFit: RunConfig | null;
  /** What PM's pick is, when it is this model at a config no rung shows. */
  band: string | null;
}) {
  const { showMeta } = useDepth();
  const locate = useLocate();
  const f = rec.fit;
  const ramTarget = { tag: rec.ollama_pull, sharded: rec.sharded_quant };
  const gpuTarget = rec.gpu_pull;
  const isSplit = rec.gpu.kind === "split";
  // Its progress shows here only when it isn't shown elsewhere (the start card, or above the folded
  // list); the button says "Downloading…" either way, because that is what the file is doing.
  const pulling =
    progressHere &&
    pullingTag !== null &&
    (pullingTag === rec.ollama_pull || pullingTag === gpuTarget?.tag);
  const showIt = (
    <Button variant="tertiary" size="sm" onClick={() => locate?.("sec-localai-start")}>
      Show it
    </Button>
  );

  const pickChip = (
    <span className="flex items-center gap-1.5">
      <TokenChip token="--accent">PM's pick</TokenChip>
      {showIt}
    </span>
  );
  /** Whether a rung IS PM's pick, while the start card offers its download: the pick's file, at the
   *  pick's context and cache. The same file at another config is not, whatever its tag says. */
  const isPick = (tag: string | null, fit: RunConfig) =>
    tag !== null && tag === pickTag && pickFit !== null && sameConfig(fit, pickFit);

  /** One rung's own action: it is already here, it is PM's pick (whose Download is on the start
   *  card), PM can fetch it, or neither (the commands below). A rung with the pick's file at another
   *  config offers nothing: the file's one Download is the start card's, and the band line says so. */
  const rungAction = (t: { tag: string | null } | null, fit: RunConfig): ReactNode => {
    const tag = t?.tag ?? null;
    if (!tag) return null;
    if (servedTags.has(tag.toLowerCase()))
      return <span className="text-[0.625rem] font-medium text-st-quick">Installed</span>;
    if (tag === pickTag) return isPick(tag, fit) ? pickChip : null;
    if (!canPull) return null;
    return (
      <Button variant="secondary" size="sm" onClick={() => onPull(tag)} disabled={busy}>
        {pullingTag === tag ? "Downloading\u2026" : "Download"}
      </Button>
    );
  };
  // MoE when fewer params are active per token than the model holds (matches the catalog's own rule).
  const isMoe = rec.active_parameters_b + 0.01 < rec.parameters_b;
  return (
    <div id={recCardId(rec.repo)} className="rounded-[var(--radius-sm)] border border-border p-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <span className="text-sm font-medium text-ink">{rec.display_name}</span>
            <FitBadge verdict={f.verdict} />
            {showMeta && isMoe && <span className="text-[0.625rem] text-ink4">MoE</span>}
            {/* No "vision" chip, though `rec.multimodal` still says so. This row is what PM will do
                with the model, and PM cannot send it an image: chat messages carry a plain string,
                so no picture reaches any model, cloud or local. PM reads images through the sidecar
                instead. Advertising it here made a heavier model look more capable for PM's purposes
                than a lighter one, which is the opposite of true. A chip has no room for the caveat,
                which is itself the argument for leaving it out. */}
            {showMeta && rec.reasoning && (
              <span className="text-[0.625rem] text-ink4">reasoning</span>
            )}
            {showMeta && rec.role_hint && (
              <span className="text-[0.625rem] text-ink4">suits {rec.role_hint}</span>
            )}
            {/* Every row says what its weights are under, in the same quiet ink: a restricted
                licence is said in words beside the link, not by colouring the link like an error. */}
            <span className="text-[0.625rem] text-ink4">
              <a
                href={rec.licence.url}
                target="_blank"
                rel="noreferrer noopener"
                className="text-ink4 underline decoration-dotted underline-offset-2"
              >
                {rec.licence.name}
              </a>
              {!rec.licence.open && " · its own terms"}
            </span>
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
              <ConfigRow label="Highest quality" fit={f} action={rungAction(ramTarget, f)} />
              {/* The same file as the first row has its one action there — unless this row is the
                  pick's own config, which is what the chip marks. */}
              <ConfigRow
                label="Fastest on GPU"
                fit={rec.gpu.fit}
                action={
                  gpuTarget?.same_file
                    ? isPick(rec.ollama_pull, rec.gpu.fit)
                      ? pickChip
                      : undefined
                    : rungAction(gpuTarget, rec.gpu.fit)
                }
              />
              {gpuTarget?.same_file ? (
                <p className="text-[0.625rem] text-ink4">
                  Both rows are the same file — the difference is the settings your server runs it
                  with.
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
          ) : rec.ollama_pull && rec.ollama_pull === pickTag ? (
            // The pick's file: marked only when the card runs it the pick's way; otherwise its one
            // Download is the start card's, and the band line says what the pick is.
            isPick(rec.ollama_pull, f) ? (
              pickChip
            ) : null
          ) : canPull && rec.ollama_pull ? (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => rec.ollama_pull && onPull(rec.ollama_pull)}
              disabled={busy || f.verdict === "stay_on_cloud"}
            >
              {pullingTag === rec.ollama_pull ? "Downloading\u2026" : "Download"}
            </Button>
          ) : null}
        </div>
      </div>

      {otherBuild && <p className="mt-1.5 text-[0.6875rem] text-ink4">{otherBuild}</p>}
      {band && (
        <div className="mt-1.5 flex flex-wrap items-center gap-2">
          <p className="min-w-0 flex-1 text-[0.6875rem] text-ink4">{band}</p>
          {showIt}
        </div>
      )}

      {pulling && (
        <PullProgressRow
          name={rec.display_name}
          pullProg={pullProg}
          startedAt={startedAt}
          onCancel={onCancel}
        />
      )}

      {!installed &&
        (() => {
          // Every way to get this model that PM can name. Always folded: the card is the summary,
          // and the commands are for the reader who has decided. "Install it another way" when PM
          // can download it for you, "How to get it" when it can't. A split's GPU rung is its own
          // config even when it is the same file — its Ollama tag is then the first rung's, so only
          // its llama-server line differs.
          const rungs: Rung[] = [
            { label: "Highest quality", tag: rec.ollama_pull, fit: f },
            ...(rec.gpu.kind === "split"
              ? [
                  {
                    label: "Fastest on GPU",
                    tag: gpuTarget?.same_file ? null : (gpuTarget?.tag ?? null),
                    fit: rec.gpu.fit,
                  },
                ]
              : []),
          ];
          return (
            <Collapsible
              title={canPull ? "Install it another way" : "How to get it"}
              defaultOpen={false}
              className="mt-2"
            >
              <ModelInstallHint
                repo={rec.repo}
                rungs={rungs}
                shardedQuant={rec.sharded_quant}
                pickElsewhere={band !== null}
              />
            </Collapsible>
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
