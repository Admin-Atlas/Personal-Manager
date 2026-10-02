// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for PM's pick, as pure readings of the payload: why this model, what it costs this
// machine, and what to set on the server so it runs the way PM sized it.
//
// The pick ranks on memory and size, never on speed (better_fit.rs). On a discrete graphics card
// speed is a gate as well: a build counts only if PM expects it to reply at the chat floor or more
// (`chat_speed`, which the backend owns — copy prints its figure, never a literal). So "why" states
// the floor there and never mentions speed off the card, the figure itself sits among the facts with
// its own qualifier beside it, and a larger model passed over for being too slow is named — by
// `passedOverLine`, or by `inUseLine` when it is the one a job runs on. "Room to spare" is said only
// of a Comfortable fit: a Tight one is a fit, but saying it has room would be the one reassurance
// the verdict exists to withhold.

import { formatGib } from "../../lib/format";
import type {
  LocalChatSpeed,
  LocalDiskSource,
  LocalFitResult,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalPickBasis,
  LocalRecommendations,
} from "../../lib/types";
import { powerOf } from "../../lib/powerRoute";
import type { RunnerName } from "../../lib/workbenchGuide";
import { TUNING_TITLE } from "./locate";
import { serverIgnoresCard, spillsOffCard } from "./readiness";
import { sectionLabel } from "./sections";
import { speedShort } from "./speedWords";

export type ShownPick = Exclude<LocalPick, { kind: "nothing" }>;
export type NothingPick = Extract<LocalPick, { kind: "nothing" }>;
export type CataloguePick = Extract<LocalPick, { kind: "catalogue" }>;

/** "32k". */
function kOf(context: number | null): string {
  return `${((context ?? 0) / 1024).toFixed(0)}k`;
}

/** What the fit leaves free, said only for a fit that has it. */
function room(fit: LocalFitResult): string {
  if (fit.verdict === "comfortable") return ", with room to spare";
  if (fit.verdict === "tight") return " — with only a little room to spare";
  return "";
}

/** Where a model of this basis fits, for the owned sentences. */
const FITS_IN: Record<LocalPickBasis, string> = {
  gpu: "entirely on your graphics card",
  shared: "the memory this computer shares between its processor and graphics",
  system: "the memory you have free",
};

const FOLDER: Record<LocalDiskSource, string> = {
  ollama: "Ollama's folder",
  lm_studio: "LM Studio's folder",
  hugging_face: "your Hugging Face folder",
  folder: "the folder you added",
};

/** Whether the pick is the model both jobs already run on locally. */
export function pickInUse(
  pick: ShownPick,
  config: LocalLlmConfig | null,
  status: LocalLlmStatus | null,
  servedIds: ReadonlySet<string>,
): boolean {
  if (!config || !status) return false;
  // Served, either way: a job bound to a model the server isn't serving runs on nothing, whatever
  // its route says — and an owned pick may be a file on disk.
  const id = pick.kind === "owned" ? pick.id.toLowerCase() : pick.tag.toLowerCase();
  if (!((pick.kind === "owned" && pick.served) || servedIds.has(id))) return false;
  const power = powerOf(status);
  const local = (e: string | undefined) => e === "local_only" || e === "local_then_cloud";
  return (
    (config.chat_model ?? "").toLowerCase() === id &&
    (config.background_model ?? "").toLowerCase() === id &&
    local(power.chat.effective) &&
    local(power.background.effective)
  );
}

/** The line above the pick's name. */
export function eyebrow(pick: ShownPick, inUse: boolean): string {
  if (inUse) return "In use — PM's pick for this computer";
  if (pick.kind === "owned") return "PM's pick for this computer — you already have it";
  return "PM's pick for this computer";
}

/** "30", the chat floor as the copy prints it. */
function floorOf(speed: LocalChatSpeed): string {
  return speed.floor_tps.toFixed(0);
}

/** Why this model, from its verdict and what it was judged against. Speed only on the graphics
 *  card, where it is part of the rule: the floor, never the pick's own figure.
 *
 *  "Fits entirely on your graphics card" is a claim about one context: every model is judged at
 *  the context PM sizes it for (better_fit.rs `pick_context`: 32k tokens, less for a model made for
 *  less, more for one the server is proven to run with more), and a larger one that only fits with
 *  less than that is passed over — so the sentence says which context. */
export function why(pick: ShownPick, speed: LocalChatSpeed): string {
  const r = room(pick.fit);
  const floor = floorOf(speed);
  let text: string;
  if (pick.kind === "catalogue") {
    switch (pick.basis) {
      case "gpu":
        text = `The largest model in PM's list that fits entirely on your graphics card at the context PM sizes it for${r}, and that PM expects to reply at ${floor} tok/s or more.`;
        break;
      case "shared":
        text = `The largest model in PM's list that fits the memory this computer shares between its processor and graphics${r}, and that PM's cautious estimate says is quick enough for its background work.`;
        break;
      default:
        text = `Without a separate graphics card, models run from system memory. This is the largest in PM's list that fits what you have free${r}, and that PM's cautious estimate says is quick enough for its background work.`;
    }
  } else {
    const where = pick.served
      ? "It's on your server"
      : `It's already on this computer, in ${FOLDER[pick.source ?? "folder"]}`;
    text =
      pick.basis === "gpu"
        ? `${where}, it fits entirely on your graphics card${r}, PM expects it to reply at ${floor} tok/s or more, and nothing in PM's list that does both is at least 15% larger.`
        : `${where}, it fits ${FITS_IN[pick.basis]}${r}, and nothing in PM's list that fits is at least 15% larger.`;
  }
  if (pick.kind === "owned" && !pick.measured)
    text +=
      " PM can't see which file your server loaded, so these figures are for the largest version of it in PM's list.";
  return text;
}

/** The facts row, plus the two Depth-revealed extras: the parameter count (standard and up) and
 *  the reserve arithmetic behind the memory figure (Power). Every figure the pick is justified by —
 *  memory against the card or free memory, quant, context, cache, speed — is in `row`. */
export function facts(
  pick: ShownPick,
  recs: LocalRecommendations,
): { row: string[]; params: string | null; reserve: string } {
  const f = pick.fit;
  const mem = formatGib(f.est_memory_gb);
  const free = formatGib(recs.live_available_ram_gb);
  const vram = formatGib(recs.hardware.vram_gb);
  const row = [
    pick.basis === "gpu"
      ? `${mem} of your ${vram} graphics card`
      : pick.basis === "shared"
        ? `${mem} of shared memory`
        : `${mem} of the ${free} free`,
  ];
  if (f.quant) row.push(f.quant);
  if (f.context != null) row.push(`${kOf(f.context)} context`);
  if (f.kv === "q8_0") row.push("compressed cache (q8_0)");
  const speed = speedShort(f);
  if (speed) row.push(speed);
  const rec = recs.curated.find((r) => r.repo === pick.repo);
  const params = rec
    ? `${rec.parameters_b}B parameters${
        rec.active_parameters_b + 0.01 < rec.parameters_b
          ? `, ${rec.active_parameters_b}B active`
          : ""
      }`
    : null;
  const reserve =
    pick.basis === "gpu"
      ? `Sized against ${vram} less the ${formatGib(recs.gpu_reserve_gb)} PM keeps free on the card.`
      : `Sized against ${free} free, less the ${formatGib(recs.reserve_gb)} PM keeps for everything else.`;
  return { row, params, reserve };
}

/**
 * What the server has to be set to for the model to run the way PM sized it — or null when PM sized
 * it at a server's usual settings (a 4k context on an f16 cache), with nothing to change.
 *
 * Worded for the server it is about, and pointing only at steps that are on the page:
 *   * `runner`: the connected server by its port — or, before anything is connected, the one PM
 *     found running, which step 1 offers to connect;
 *   * `setup`: the server guide step 1 is showing while nothing is connected or found — Ollama's
 *     carries its two settings as steps, LM Studio's has no number to set, so its line gives them;
 *   * Model server's tuning fold exists only once something is connected, so before then it is
 *     where the steps will be, not where they are;
 *   * `commandShown`: step 2 is showing a llama-server command, which already carries the settings.
 */
export function settingsLine(
  fit: Pick<LocalFitResult, "context" | "kv">,
  server: {
    configured: boolean;
    runner: RunnerName | null;
    setup: RunnerName | null;
    commandShown: boolean;
  },
): string | null {
  const longContext = (fit.context ?? 0) > 4096;
  const cache = fit.kv === "q8_0";
  if (!longContext && !cache) return null;
  const both = longContext && cache;
  const sized = `PM sized this for a ${kOf(fit.context)} context${
    cache ? " with a compressed (q8_0) cache" : ""
  }`;
  const fold = `under ${sectionLabel("sec-localai-endpoint")}, in “${TUNING_TITLE}”`;
  // Before connecting, the guide step 1 is showing is the server the reader is about to set up.
  const runner = server.configured ? server.runner : (server.setup ?? server.runner);
  if (runner === "Ollama") {
    const where = server.configured
      ? fold
      : server.setup === "Ollama"
        ? "in step 1 below"
        : `${fold}, once you've connected`;
    return `${sized}. Ollama only runs it that way once ${both ? "both are" : "that's"} set — the steps are ${where}.`;
  }
  if (runner === "LM Studio") {
    // LM Studio's V cache compresses only with Flash Attention on. Model server's tuning step says
    // so too, but only once something is connected — before that, this line is all there is.
    return `${sized}. In LM Studio, set the context length to ${fit.context ?? ""}${
      cache ? " and the K and V cache quantization to Q8_0, with Flash Attention on," : ""
    } in the model's load settings.`;
  }
  if (runner === "llama-server" && server.commandShown) {
    return `${sized} — the command in step 2 includes ${both ? "both settings" : "that setting"}.`;
  }
  return `${sized}; your server needs ${both ? "both settings" : "that setting"} too, or the model may not fit.`;
}

/** The download and the disk it lands on, with a warning when it doesn't fit there. */
export function diskLine(
  pick: CataloguePick,
  recs: LocalRecommendations,
): { line: string; over: string | null } {
  const disk = recs.hardware.disk_free_gb;
  return {
    line: `${formatGib(pick.download_gb)} download · ${formatGib(disk)} free on disk`,
    over:
      disk != null && pick.download_gb > disk
        ? `That's more than the ${formatGib(disk)} free on this computer's disk.`
        : null,
  };
}

/** Why PM isn't picking a model — a full sentence, with what the reader can still do. `hasCloud`:
 *  some cloud key exists, so "keep using your cloud model" is advice and not a fiction.
 *  `too_slow_for_chat` is a card that holds models but, by PM's estimate, runs none at the chat
 *  floor — only on a card far slower than any PM lists. */
export function nothingSentence(
  pick: NothingPick,
  recs: LocalRecommendations,
  hasCloud: boolean,
): string {
  const allModels = sectionLabel("sec-localai-models");
  const meanwhile = hasCloud
    ? `Keep using your cloud model for now, or choose one yourself under ${allModels}`
    : `You can still choose one yourself under ${allModels}`;
  const vram = formatGib(recs.hardware.vram_gb);
  switch (pick.reason) {
    case "too_slow_for_chat":
      return `Every model in PM's list that fits your ${vram} graphics card would reply at under ${floorOf(recs.chat_speed)} tok/s by PM's estimate, too slow for PM to pick one for chat. ${meanwhile}, where each says how fast PM expects it to be.`;
    case "nothing_on_gpu":
      return pick.system_fallback
        ? `Nothing in PM's list fits your ${vram} graphics card with the room PM keeps free. Some would run from system memory instead, several times slower, and PM doesn't pick one of those on a computer with a graphics card. You can still choose one yourself under ${allModels}, where each says how it would run.`
        : `Nothing in PM's list fits your ${vram} graphics card with the room PM keeps free, and the models that fit system memory would reply too slowly for PM's background work to finish in time, by PM's cautious estimate. ${meanwhile}.`;
    case "too_slow":
      return `Every model PM could fit here would run from ${
        pick.basis === "shared" ? "this computer's shared memory" : "system memory"
      } too slowly for PM's background work to finish in time, by PM's cautious estimate. ${meanwhile} — it may be fine for chat.`;
    case "too_little_memory":
      return `Nothing in PM's list fits the ${formatGib(recs.live_available_ram_gb)} of memory free right now, after the ${formatGib(recs.reserve_gb)} PM keeps for everything else. Closing some apps and pressing Check again may change that.`;
  }
}

/** The parameter count of a model the user has, by its served id or on-disk name, through the
 *  catalogue entry it matched. */
function paramsOfOwned(id: string, recs: LocalRecommendations): number | null {
  const repo =
    recs.installed.find((m) => m.id === id)?.matched_repo ??
    recs.on_disk.find((m) => m.name === id)?.matched_repo ??
    null;
  return repo ? (recs.curated.find((r) => r.repo === repo)?.parameters_b ?? null) : null;
}

/** "You already have X, which fits too", when the user has a model that lost to the pick only on
 *  the 15% rule. */
export function alsoHaveLine(pick: CataloguePick, recs: LocalRecommendations): string | null {
  const have = pick.also_have;
  if (!have) return null;
  const p = recs.curated.find((r) => r.repo === pick.repo)?.parameters_b ?? null;
  const q = paramsOfOwned(have.id, recs);
  const sizes = p != null && q != null ? ` (${p}B parameters against ${q}B)` : "";
  return `You already have ${have.display_name}, which fits too — ${pick.display_name} is at least 15% larger${sizes}, so it's the one PM picks.`;
}

/**
 * The larger model PM passed over because it expects it to reply under the chat floor on this card
 * (better_fit.rs `passed_over`) — the user's own copy of it when they have one. null when nothing
 * was, or when that copy is the model a job runs on (`bound`): `inUseLine` says it then, as the
 * model in use.
 */
export function passedOverLine(
  pick: ShownPick,
  recs: LocalRecommendations,
  bound: string | null,
): string | null {
  const po = pick.passed_over;
  if (!po) return null;
  const have = po.have;
  if (have && bound && have.id.toLowerCase() === bound.toLowerCase()) return null;
  const slow = `PM expects it to reply at about ${po.est_tokens_per_sec.toFixed(0)} tok/s here — under the ${floorOf(recs.chat_speed)} tok/s it wants for chat`;
  if (have)
    return `You already have ${have.display_name}, which is larger and also fits your graphics card, but ${slow}.`;
  return `${po.display_name} is larger and also fits your graphics card, but ${slow}. It's under ${sectionLabel("sec-localai-models")} if you'd rather have the larger model.`;
}

/**
 * When a job already runs on a different local model than the pick: what the difference is. null
 * when the bound model is the pick, or when PM can't say something true about the pair.
 */
export function inUseLine(
  pick: ShownPick,
  bound: string | null,
  recs: LocalRecommendations,
): string | null {
  if (!bound) return null;
  const b = bound.toLowerCase();
  if (b === (pick.kind === "owned" ? pick.id : pick.tag).toLowerCase()) return null;
  const row = recs.installed.find((m) => m.id === bound);
  // A server not using the card at all runs any model from system memory, the pick included, so it
  // is said without pointing at the pick.
  if (pick.basis === "gpu" && serverIgnoresCard(recs, bound))
    return `You're using ${bound}, which runs entirely from system memory because your server isn't using your graphics card.`;
  // Only when the backend showed it doesn't fit the card (`spills_gpu`, via `spillsOffCard`) — never
  // from the row's sizing, which is f16 first and high by design.
  if (pick.basis === "gpu" && spillsOffCard(recs, bound))
    return `You're using ${bound}, which runs at least partly from system memory rather than your graphics card, so it replies slowly. PM's pick is sized to fit on the card.`;
  // The model in use is the larger one PM passed over for speed: it fits the card, so the reason it
  // isn't the pick is the chat floor, and the line says that rather than comparing sizes.
  const po = pick.passed_over;
  if (po?.have && po.have.id.toLowerCase() === b) {
    const m = pick.fit.est_tokens_per_sec?.toFixed(0);
    return `You're using ${bound}, which is larger, but PM expects it to reply at about ${po.est_tokens_per_sec.toFixed(0)} tok/s here — under the ${floorOf(recs.chat_speed)} tok/s it wants for chat.${
      m != null ? ` PM's pick should reply at about ${m}.` : ""
    }`;
  }
  // A build PM can show fits the card but is too slow for chat there, which `passed_over` never names
  // when it isn't larger than the pick — most often a heavier build of the pick's own model.
  if (pick.basis === "gpu" && row?.under_chat_floor_tps != null) {
    const n = row.under_chat_floor_tps.toFixed(0);
    const floor = floorOf(recs.chat_speed);
    const m = pick.fit.est_tokens_per_sec?.toFixed(0);
    if (row.matched_repo === pick.repo)
      return `You're using ${bound}, but PM expects that build to reply at about ${n} tok/s here — under the ${floor} tok/s it wants for chat. PM's pick is a quicker build of the same model${m != null ? `, at about ${m}` : ""}.`;
    const p = recs.curated.find((r) => r.repo === pick.repo)?.parameters_b ?? null;
    const q = row.matched_repo
      ? (recs.curated.find((r) => r.repo === row.matched_repo)?.parameters_b ?? null)
      : null;
    const larger = p != null && q != null && q > p ? ", which is larger," : ",";
    return `You're using ${bound}${larger} but PM expects it to reply at about ${n} tok/s here — under the ${floor} tok/s it wants for chat.${m != null ? ` PM's pick should reply at about ${m}.` : ""}`;
  }
  if (row && row.matched_repo === null)
    return `You're using ${bound}, which isn't in PM's list, so PM can't compare the two.`;
  const p = recs.curated.find((r) => r.repo === pick.repo)?.parameters_b ?? null;
  const q = row?.matched_repo
    ? (recs.curated.find((r) => r.repo === row.matched_repo)?.parameters_b ?? null)
    : null;
  if (p != null && q != null && p >= q * 1.15)
    return `You're using ${bound}. PM's pick is at least 15% larger and also fits.`;
  return null;
}

/** "How PM picks", the folded explainer under the start card. The chat-floor sentence needs the
 *  backend's figure, so with no payload (`speed` null) it is left out rather than guessed. */
export function howPmPicks(speed: LocalChatSpeed | null): string {
  const chat = speed
    ? `On a graphics card it must also be quick enough for chat: PM wants at least ${floorOf(speed)} tok/s, enough to write an answer of a few paragraphs in about ${speed.reply_secs.toFixed(0)} seconds, and when a model's best build is slower than that, PM tries its smaller builds before passing it over. Those speeds are PM's own estimates, from your card's published memory speed (or a typical one when PM doesn't recognise the card) and tests on one laptop graphics card, not measurements on this computer. `
    : "";
  return `PM looks for the largest model in its list that runs entirely on your graphics card with the room PM keeps free, at the context PM sizes it for — 32k tokens, less for a model made for less, and more for one your server already runs with more — because a model that spills into system memory replies many times slower. ${chat}On a computer without a separate graphics card, it only considers models its cautious estimate says are quick enough for PM's background work. If you already have a model that passes the same tests and nothing in the list is at least 15% larger, PM points at the one you have. It never downloads or switches anything for you.`;
}
