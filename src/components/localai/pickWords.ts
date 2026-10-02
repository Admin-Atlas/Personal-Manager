// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for PM's pick, as pure readings of the payload: why this model, what it costs this
// machine, and what to set on the server so it runs the way PM sized it.
//
// The pick ranks on memory and size, never on speed (better_fit.rs), so nothing here gives speed as
// a reason — "why" never mentions it, and the speed figure sits among the facts with its own
// qualifier beside it. "Room to spare" is said only of a Comfortable fit: a Tight one is a fit, but
// saying it has room would be the one reassurance the verdict exists to withhold.

import { formatGib } from "../../lib/format";
import type {
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
  const id =
    pick.kind === "owned"
      ? pick.id.toLowerCase()
      : servedIds.has(pick.tag.toLowerCase())
        ? pick.tag.toLowerCase()
        : null;
  if (!id) return false;
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

/** Why this model, from its verdict and what it was judged against. Never speed. */
export function why(pick: ShownPick): string {
  const r = room(pick.fit);
  let text: string;
  if (pick.kind === "catalogue") {
    switch (pick.basis) {
      case "gpu":
        text = `The largest model in PM's list that fits entirely on your graphics card${r}.`;
        break;
      case "shared":
        text = `The largest model in PM's list that fits the memory this computer shares between its processor and graphics${r}, and that PM's cautious estimate says is quick enough for its background work.`;
        break;
      default:
        text = `Without a separate graphics card, models run from system memory. This is the largest in PM's list that fits what you have free${r}, and that PM's cautious estimate says is quick enough for its background work.`;
    }
  } else if (pick.served) {
    text = `It's on your server, it fits ${FITS_IN[pick.basis]}${r}, and nothing in PM's list that fits is at least 15% larger.`;
  } else {
    text = `It's already on this computer, in ${FOLDER[pick.source ?? "folder"]}, it fits ${FITS_IN[pick.basis]}${r}, and nothing in PM's list that fits is at least 15% larger.`;
  }
  if (pick.kind === "owned" && !pick.measured)
    text +=
      " PM can't see which file your server loaded, so these figures are for the version PM would pick.";
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
 * `setupShown`: the start card is showing the server guide (step 1), so Ollama's steps are there;
 * otherwise they are in Model server's tuning fold. `commandShown`: step 2 is showing a llama-server
 * command, which already carries the settings.
 */
export function settingsLine(
  fit: Pick<LocalFitResult, "context" | "kv">,
  server: {
    configured: boolean;
    runner: RunnerName | null;
    setupShown: boolean;
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
  if (!server.configured || server.runner === "Ollama") {
    return `${sized}. Ollama only runs it that way once ${both ? "both are" : "that's"} set — the steps are ${
      server.setupShown
        ? "in step 1 below"
        : `under ${sectionLabel("sec-localai-endpoint")}, in “${TUNING_TITLE}”`
    }.`;
  }
  if (server.runner === "LM Studio") {
    return `${sized}. In LM Studio, set the context length to ${fit.context ?? ""}${
      cache ? " and the K and V cache quantization to Q8_0" : ""
    } in the model's load settings.`;
  }
  if (server.runner === "llama-server" && server.commandShown) {
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
 *  some cloud key exists, so "keep using your cloud model" is advice and not a fiction. */
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
  if (pick.basis === "gpu" && row?.fit.speed_basis === "system")
    return `You're using ${bound}, which is larger than your graphics card's memory, so it runs from system memory. PM's pick fits on the card.`;
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
