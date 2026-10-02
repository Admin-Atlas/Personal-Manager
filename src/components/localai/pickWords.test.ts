// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for PM's pick. The pick ranks on memory and size, never on speed, so the reason given
// for it never mentions speed; and "room to spare" is said only of a Comfortable fit, because a
// Tight one has exactly the room the verdict says it doesn't.

import { describe, expect, it } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  LocalDiskSource,
  LocalFitResult,
  LocalInstalledModel,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalRecommendation,
  LocalRecommendations,
} from "../../lib/types";
import type { RunnerName } from "../../lib/workbenchGuide";
import {
  alsoHaveLine,
  diskLine,
  eyebrow,
  facts,
  inUseLine,
  nothingSentence,
  pickInUse,
  settingsLine,
  why,
  type CataloguePick,
  type NothingPick,
  type ShownPick,
} from "./pickWords";
import { COPY_COLLISIONS, SAME_SECTION_POINTERS } from "./readiness";
import { sectionLabel } from "./sections";

const QWEN = "bartowski/Qwen2.5-7B-Instruct-GGUF";

const fit = (over: Partial<LocalFitResult> = {}): LocalFitResult => ({
  verdict: "tight",
  quant: "Q5_K_M",
  context: 32768,
  kv: "q8_0",
  est_memory_gb: 6.63,
  est_tokens_per_sec: 71,
  speed_basis: "gpu_published",
  notes: [],
  ...over,
});

const catalogue = (over: Partial<CataloguePick> = {}): CataloguePick => ({
  kind: "catalogue",
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  rung: "gpu",
  tag: `hf.co/${QWEN}:Q5_K_M`,
  fit: fit(),
  download_gb: 5.07,
  basis: "gpu",
  also_have: null,
  ...over,
});

const owned = (over: Partial<Extract<LocalPick, { kind: "owned" }>> = {}): ShownPick => ({
  kind: "owned",
  id: "qwen2.5:latest",
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  served: true,
  source: null,
  path: null,
  shards: 1,
  measured: true,
  fit: fit(),
  basis: "gpu",
  ...over,
});

/** A served model's row. `measured`: its figures are for the user's own file at the context the
 *  server serves — the only rows a "runs from system memory" claim may rest on. */
const installed = (
  id: string,
  matched_repo: string | null,
  over: Partial<LocalFitResult> = {},
  measured = true,
): LocalInstalledModel & { measured: boolean } => ({ id, matched_repo, fit: fit(over), measured });

const nothing = (over: Partial<NothingPick> = {}): NothingPick => ({
  kind: "nothing",
  reason: "nothing_on_gpu",
  basis: "gpu",
  system_fallback: true,
  ...over,
});

const rec = (over: Partial<LocalRecommendation> = {}): LocalRecommendation => ({
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  architecture: "qwen2",
  role_hint: null,
  parameters_b: 7.62,
  active_parameters_b: 7.62,
  context_length: 32768,
  multimodal: false,
  reasoning: null,
  ollama_pull: `hf.co/${QWEN}:Q8_0`,
  sharded_quant: false,
  gpu_pull: null,
  licence: {
    id: "apache-2.0",
    name: "Apache License 2.0",
    url: "https://x",
    open: true,
    summary: "",
  },
  fit: fit(),
  gpu: { kind: "single" },
  ...over,
});

const recs = (over: Partial<LocalRecommendations> = {}): LocalRecommendations => ({
  hardware: {
    platform: "linux",
    total_ram_gb: 32,
    available_ram_gb: 20,
    cpu_brand: null,
    cpu_cores: null,
    cpu_threads: null,
    disk_free_gb: 200,
    gpu_name: "RTX 5060 Laptop",
    gpu_vendor: "nvidia",
    vram_gb: 8,
    vram_source: null,
    gpu_bandwidth_gbps: 384,
    unified_memory: false,
    is_wsl: false,
    notes: [],
  },
  reserve_gb: 2,
  gpu_reserve_gb: 1,
  catalog_version: 4,
  catalog_generated_at: "2026-09-30",
  endpoint_configured: true,
  cadence: "monthly",
  rescan_due: false,
  curated: [
    rec(),
    rec({
      repo: "bartowski/gemma-3-4b-it-GGUF",
      display_name: "gemma 3 4b it",
      parameters_b: 3.88,
    }),
  ],
  installed: [],
  on_disk: [],
  disk_sources_present: [],
  disk_blocked: [],
  endpoint_inventory: null,
  co_residency: null,
  disk_found: 0,
  disk_truncated: false,
  scan_dir: null,
  terms_accepted: [],
  live_available_ram_gb: 12,
  ...over,
});

describe("why — the reason for the pick", () => {
  it("names what it fits, by basis", () => {
    // At which context: a larger model that fits the card only with less than PM sizes for is
    // passed over, so "the largest that fits entirely" without it is false.
    expect(why(catalogue())).toBe(
      "The largest model in PM's list that fits entirely on your graphics card at the context PM sizes it for — with only a little room to spare.",
    );
    expect(why(catalogue({ basis: "shared", fit: fit({ verdict: "comfortable" }) }))).toBe(
      "The largest model in PM's list that fits the memory this computer shares between its processor and graphics, with room to spare, and that PM's cautious estimate says is quick enough for its background work.",
    );
    expect(why(catalogue({ basis: "system" }))).toBe(
      "Without a separate graphics card, models run from system memory. This is the largest in PM's list that fits what you have free — with only a little room to spare, and that PM's cautious estimate says is quick enough for its background work.",
    );
  });

  it("says where a model the user already has is, and why it wins", () => {
    expect(why(owned())).toBe(
      "It's on your server, it fits entirely on your graphics card — with only a little room to spare, and nothing in PM's list that fits is at least 15% larger.",
    );
    const folders = {
      ollama: "Ollama's folder",
      lm_studio: "LM Studio's folder",
      hugging_face: "your Hugging Face folder",
      folder: "the folder you added",
    } as const;
    for (const [source, words] of Object.entries(folders)) {
      expect(
        why(owned({ served: false, source: source as keyof typeof folders, basis: "system" })),
      ).toBe(
        `It's already on this computer, in ${words}, it fits the memory you have free — with only a little room to spare, and nothing in PM's list that fits is at least 15% larger.`,
      );
    }
    expect(why(owned({ basis: "shared" }))).toContain(
      "it fits the memory this computer shares between its processor and graphics",
    );
  });

  it("admits when the figures are for the largest version in PM's list, not the file served", () => {
    // The backend judges an unmeasured served model at the heaviest build PM lists for it — the
    // cautious reading — so that is what the hedge has to name.
    expect(why(owned({ measured: false }))).toMatch(
      / PM can't see which file your server loaded, so these figures are for the largest version of it in PM's list\.$/,
    );
    expect(why(owned({ measured: true }))).not.toContain("can't see which file");
  });

  it("never gives speed as a reason, and says 'room to spare' only of a Comfortable fit", () => {
    const all: ShownPick[] = [];
    for (const verdict of ["comfortable", "tight", "halved_context"] as const)
      for (const basis of ["gpu", "shared", "system"] as const)
        all.push(
          catalogue({ basis, fit: fit({ verdict }) }),
          owned({ basis, fit: fit({ verdict }) }),
          owned({ basis, served: false, source: "ollama", fit: fit({ verdict }) }),
        );
    for (const p of all) {
      const text = why(p);
      expect(text).not.toMatch(/tok\/s|speed|faster|slower/i);
      expect(text.includes(", with room to spare")).toBe(p.fit.verdict === "comfortable");
    }
  });
});

describe("facts — every figure the pick rests on", () => {
  it("measures it against the card, the shared memory or the free memory", () => {
    const r = recs();
    expect(facts(catalogue(), r).row).toEqual([
      "6.6 GB of your 8.0 GB graphics card",
      "Q5_K_M",
      "32k context",
      "compressed cache (q8_0)",
      "up to 71 tok/s",
    ]);
    expect(
      facts(catalogue({ basis: "shared", fit: fit({ speed_basis: "shared" }) }), r).row,
    ).toEqual([
      "6.6 GB of shared memory",
      "Q5_K_M",
      "32k context",
      "compressed cache (q8_0)",
      "speed not estimated",
    ]);
    // Free memory is the figure the verdicts used, read live — not the cached scan's.
    expect(facts(catalogue({ basis: "system", fit: fit({ kv: "f16" }) }), r).row[0]).toBe(
      "6.6 GB of the 12.0 GB free",
    );
  });

  it("adds the parameter count and the reserve arithmetic as detail", () => {
    const r = recs({
      curated: [rec({ repo: QWEN, parameters_b: 35, active_parameters_b: 3 })],
    });
    expect(facts(catalogue(), r).params).toBe("35B parameters, 3B active");
    expect(facts(catalogue(), recs()).params).toBe("7.62B parameters");
    expect(facts(catalogue(), r).reserve).toBe(
      "Sized against 8.0 GB less the 1.0 GB PM keeps free on the card.",
    );
    expect(facts(catalogue({ basis: "system" }), r).reserve).toBe(
      "Sized against 12.0 GB free, less the 2.0 GB PM keeps for everything else.",
    );
  });

  it("warns when the download is bigger than the disk", () => {
    expect(diskLine(catalogue(), recs())).toEqual({
      line: "5.1 GB download · 200.0 GB free on disk",
      over: null,
    });
    const small = recs({ hardware: { ...recs().hardware, disk_free_gb: 3 } });
    expect(diskLine(catalogue(), small).over).toBe(
      "That's more than the 3.0 GB free on this computer's disk.",
    );
  });
});

describe("settingsLine — what the server has to be set to", () => {
  const server = (
    over: Partial<Parameters<typeof settingsLine>[1]> = {},
  ): Parameters<typeof settingsLine>[1] => ({
    configured: true,
    runner: "Ollama",
    setup: null,
    commandShown: false,
    ...over,
  });

  it("says nothing when PM sized it at a server's usual settings", () => {
    expect(settingsLine(fit({ context: 4096, kv: "f16" }), server())).toBeNull();
  });

  it("points Ollama at the steps, wherever they are shown", () => {
    expect(settingsLine(fit(), server())).toBe(
      `PM sized this for a 32k context with a compressed (q8_0) cache. Ollama only runs it that way once both are set — the steps are under ${sectionLabel("sec-localai-endpoint")}, in “Settings PM's numbers assume”.`,
    );
    expect(
      settingsLine(
        fit({ kv: "f16" }),
        server({ configured: false, runner: null, setup: "Ollama" }),
      ),
    ).toBe(
      "PM sized this for a 32k context. Ollama only runs it that way once that's set — the steps are in step 1 below.",
    );
  });

  it("never points at steps that aren't on the page yet", () => {
    // An Ollama found running but not yet connected: step 1 shows no guide, and Model server's
    // tuning fold only appears once something is connected.
    expect(settingsLine(fit(), server({ configured: false, runner: "Ollama", setup: null }))).toBe(
      `PM sized this for a 32k context with a compressed (q8_0) cache. Ollama only runs it that way once both are set — the steps are under ${sectionLabel("sec-localai-endpoint")}, in “Settings PM's numbers assume”, once you've connected.`,
    );
    // LM Studio installed but idle: step 1 shows LM Studio's guide, which has no number to set —
    // so the line gives the values, rather than pointing at Ollama's steps that aren't showing.
    const idle = settingsLine(
      fit(),
      server({ configured: false, runner: null, setup: "LM Studio" }),
    );
    expect(idle).toBe(
      "PM sized this for a 32k context with a compressed (q8_0) cache. In LM Studio, set the context length to 32768 and the K and V cache quantization to Q8_0, with Flash Attention on, in the model's load settings.",
    );
    // Found running, not connected, the same.
    expect(
      settingsLine(fit(), server({ configured: false, runner: "LM Studio", setup: null })),
    ).toBe(idle);
    // Nothing found yet, and no guide showing: no pointer at all.
    expect(
      settingsLine(fit({ kv: "f16" }), server({ configured: false, runner: null, setup: null })),
    ).toBe(
      "PM sized this for a 32k context; your server needs that setting too, or the model may not fit.",
    );
  });

  it("gives LM Studio the two values, and llama-server's command its due", () => {
    // A quantized V cache needs Flash Attention on: half of a two-part setting is a model that
    // won't load the way the card says.
    expect(settingsLine(fit(), server({ runner: "LM Studio" }))).toBe(
      "PM sized this for a 32k context with a compressed (q8_0) cache. In LM Studio, set the context length to 32768 and the K and V cache quantization to Q8_0, with Flash Attention on, in the model's load settings.",
    );
    expect(settingsLine(fit({ kv: "f16" }), server({ runner: "LM Studio" }))).toBe(
      "PM sized this for a 32k context. In LM Studio, set the context length to 32768 in the model's load settings.",
    );
    expect(settingsLine(fit(), server({ runner: "llama-server", commandShown: true }))).toBe(
      "PM sized this for a 32k context with a compressed (q8_0) cache — the command in step 2 includes both settings.",
    );
    // No command on screen to point at: say what the server needs instead.
    expect(settingsLine(fit({ kv: "f16" }), server({ runner: "llama-server" }))).toBe(
      "PM sized this for a 32k context; your server needs that setting too, or the model may not fit.",
    );
    expect(settingsLine(fit(), server({ runner: null }))).toBe(
      "PM sized this for a 32k context with a compressed (q8_0) cache; your server needs both settings too, or the model may not fit.",
    );
  });
});

describe("nothingSentence — why PM isn't picking", () => {
  const all = recs();
  it("says each reason, and what the reader can still do", () => {
    expect(nothingSentence(nothing(), all, true)).toBe(
      "Nothing in PM's list fits your 8.0 GB graphics card with the room PM keeps free. Some would run from system memory instead, several times slower, and PM doesn't pick one of those on a computer with a graphics card. You can still choose one yourself under All models, where each says how it would run.",
    );
    expect(nothingSentence(nothing({ system_fallback: false }), all, true)).toBe(
      "Nothing in PM's list fits your 8.0 GB graphics card with the room PM keeps free, and the models that fit system memory would reply too slowly for PM's background work to finish in time, by PM's cautious estimate. Keep using your cloud model for now, or choose one yourself under All models.",
    );
    expect(nothingSentence(nothing({ reason: "too_slow", basis: "system" }), all, true)).toBe(
      "Every model PM could fit here would run from system memory too slowly for PM's background work to finish in time, by PM's cautious estimate. Keep using your cloud model for now, or choose one yourself under All models — it may be fine for chat.",
    );
    expect(nothingSentence(nothing({ reason: "too_slow", basis: "shared" }), all, true)).toContain(
      "would run from this computer's shared memory too slowly",
    );
    expect(
      nothingSentence(nothing({ reason: "too_little_memory", basis: "system" }), all, true),
    ).toBe(
      "Nothing in PM's list fits the 12.0 GB of memory free right now, after the 2.0 GB PM keeps for everything else. Closing some apps and pressing Check again may change that.",
    );
  });

  it("never tells a keyless user to keep using a cloud model they don't have", () => {
    for (const p of [nothing({ system_fallback: false }), nothing({ reason: "too_slow" })]) {
      const said = nothingSentence(p, all, false);
      expect(said).not.toMatch(/cloud model/);
      expect(said).toContain("You can still choose one yourself under All models");
    }
  });
});

describe("the lines about models the user already has", () => {
  it("names the one that lost only on size, with both sizes", () => {
    const r = recs({
      installed: [
        installed("gemma3:4b", "bartowski/gemma-3-4b-it-GGUF", { speed_basis: "gpu_published" }),
      ],
    });
    const pick = catalogue({
      also_have: { id: "gemma3:4b", display_name: "gemma 3 4b it", served: true },
    });
    expect(alsoHaveLine(pick, r)).toBe(
      "You already have gemma 3 4b it, which fits too — Qwen2.5 7B Instruct is at least 15% larger (7.62B parameters against 3.88B), so it's the one PM picks.",
    );
    expect(alsoHaveLine(catalogue(), r)).toBeNull();
  });

  it("says why a model in use differs from the pick, only when it can say something true", () => {
    // A row the backend can really send: "system" figures only ever come from a catalogue match, and
    // only measured ones — the user's own file, at the context the server serves — describe it.
    const r = recs({
      installed: [
        installed("big:14b", "bartowski/gemma-3-4b-it-GGUF", { speed_basis: "system" }),
        installed("mystery", null),
        installed("gemma3:4b", "bartowski/gemma-3-4b-it-GGUF"),
        installed("qwen-other", QWEN),
      ],
    });
    expect(inUseLine(catalogue(), "big:14b", r)).toBe(
      "You're using big:14b, which is larger than your graphics card's memory, so it runs from system memory. PM's pick fits on the card.",
    );
    expect(inUseLine(catalogue(), "mystery", r)).toBe(
      "You're using mystery, which isn't in PM's list, so PM can't compare the two.",
    );
    expect(inUseLine(catalogue(), "gemma3:4b", r)).toBe(
      "You're using gemma3:4b. PM's pick is at least 15% larger and also fits.",
    );
    // The same model at another quant is not 15% smaller, and PM won't say it is.
    expect(inUseLine(catalogue(), "qwen-other", r)).toBeNull();
    expect(inUseLine(catalogue(), `hf.co/${QWEN}:Q5_K_M`, r)).toBeNull();
    expect(inUseLine(catalogue(), null, r)).toBeNull();
  });

  it("never says a model runs from system memory on the catalogue's guess about it", () => {
    // LM Studio and llama-server have no /api/tags, so PM can't see which file they loaded: the row
    // is the catalogue's best quant for the memory free right now (Q8_0 at 10 GB with 20 GB free),
    // while the user's own Q4_K_M sits on the card.
    const guessed = recs({
      installed: [installed("qwen2.5-7b-instruct", QWEN, { speed_basis: "system" }, false)],
    });
    expect(inUseLine(catalogue(), "qwen2.5-7b-instruct", guessed)).toBeNull();
  });
});

describe("eyebrow — the line above the name", () => {
  const cfg = (model: string): LocalLlmConfig => ({
    base_url: "http://127.0.0.1:11434",
    chat_model: model,
    background_model: model,
    chat_routing: "local",
    background_routing: "local",
    has_token: false,
  });
  const running = (effective: "local_only" | "cloud"): LocalLlmStatus => ({
    configured: true,
    reachable: true,
    in_cooldown: false,
    cooldown_remaining_s: 0,
    probed_now: false,
    chat_local_model: null,
    background_local_model: null,
    served_window: null,
    served_window_proven: false,
    window_source: null,
    chat_answering: false,
    background_answering: false,
    chat_loaded: null,
    background_loaded: null,
    chat_released: false,
    background_released: false,
    power: {
      ...INERT_POWER_VIEW,
      chat: { ...INERT_POWER_VIEW.chat, effective },
      background: { ...INERT_POWER_VIEW.background, effective },
    },
  });

  it("says whether the user has it, and whether it is the one in use", () => {
    expect(eyebrow(catalogue(), false)).toBe("PM's pick for this computer");
    expect(eyebrow(owned(), false)).toBe("PM's pick for this computer — you already have it");
    expect(eyebrow(owned(), true)).toBe("In use — PM's pick for this computer");
  });

  it("calls it in use only when both jobs really run on it", () => {
    const ids = new Set(["qwen2.5:latest"]);
    expect(pickInUse(owned(), cfg("qwen2.5:latest"), running("local_only"), ids)).toBe(true);
    expect(pickInUse(owned(), cfg("qwen2.5:latest"), running("cloud"), ids)).toBe(false);
    expect(pickInUse(owned(), cfg("other"), running("local_only"), ids)).toBe(false);
    // A catalogue pick counts once the server serves its tag.
    const tag = `hf.co/${QWEN}:Q5_K_M`;
    expect(
      pickInUse(catalogue(), cfg(tag), running("local_only"), new Set([tag.toLowerCase()])),
    ).toBe(true);
    expect(pickInUse(catalogue(), cfg(tag), running("local_only"), new Set())).toBe(false);
    // An owned pick that is a file on disk runs nothing until the server serves it — whatever the
    // route says of a job bound to its name.
    const onDisk = owned({ id: "q.gguf", served: false, source: "hugging_face" });
    expect(pickInUse(onDisk, cfg("q.gguf"), running("local_only"), new Set())).toBe(false);
    expect(pickInUse(onDisk, cfg("q.gguf"), running("local_only"), new Set(["q.gguf"]))).toBe(true);
  });
});

describe("the pick card's copy keeps the tab's rules", () => {
  // The same two rules readiness.test.ts holds the steps to, over every string this file can make:
  // no phrase another section owns, and no "above"/"below" pointing out of the section.
  function everything(): string[] {
    const out: string[] = [];
    const r = recs({
      installed: [
        installed("big:14b", "bartowski/gemma-3-4b-it-GGUF", { speed_basis: "system" }),
        installed("mystery", null),
        installed("gemma3:4b", "bartowski/gemma-3-4b-it-GGUF"),
      ],
    });
    const sources: LocalDiskSource[] = ["ollama", "lm_studio", "hugging_face", "folder"];
    const picks: ShownPick[] = [];
    for (const verdict of ["comfortable", "tight"] as const)
      for (const basis of ["gpu", "shared", "system"] as const) {
        picks.push(
          catalogue({ basis, fit: fit({ verdict }) }),
          catalogue({
            basis,
            fit: fit({ verdict }),
            also_have: { id: "gemma3:4b", display_name: "gemma 3 4b it", served: true },
          }),
          owned({ basis, fit: fit({ verdict }) }),
          owned({ basis, fit: fit({ verdict }), measured: false }),
          ...sources.map((source) =>
            owned({ basis, fit: fit({ verdict }), served: false, source }),
          ),
        );
      }
    const small = recs({ hardware: { ...recs().hardware, disk_free_gb: 1 } });
    for (const p of picks) {
      out.push(why(p), eyebrow(p, true), eyebrow(p, false));
      const f = facts(p, r);
      out.push(...f.row, f.reserve, ...(f.params ? [f.params] : []));
      if (p.kind === "catalogue") {
        const d = diskLine(p, small);
        out.push(d.line, ...(d.over ? [d.over] : []), alsoHaveLine(p, r) ?? "");
      }
      for (const bound of ["big:14b", "mystery", "gemma3:4b"])
        out.push(inUseLine(p, bound, r) ?? "");
    }
    const runners: (RunnerName | null)[] = ["Ollama", "LM Studio", "llama-server", null];
    for (const kv of ["q8_0", "f16"] as const)
      for (const configured of [true, false])
        for (const runner of runners)
          for (const setup of runners)
            for (const commandShown of [true, false])
              out.push(
                settingsLine(fit({ kv }), { configured, runner, setup, commandShown }) ?? "",
              );
    for (const reason of ["nothing_on_gpu", "too_slow", "too_little_memory"] as const)
      for (const basis of ["gpu", "shared", "system"] as const)
        for (const system_fallback of [true, false])
          for (const hasCloud of [true, false])
            out.push(nothingSentence(nothing({ reason, basis, system_fallback }), r, hasCloud));
    return out.filter(Boolean);
  }

  it("repeats no phrase a section owns", () => {
    const strings = everything();
    expect(strings.length).toBeGreaterThan(200);
    for (const s of strings) for (const re of COPY_COLLISIONS) expect(s, `${re}`).not.toMatch(re);
  });

  it("points nowhere by direction, except inside this section", () => {
    for (const s of everything()) {
      const stripped = SAME_SECTION_POINTERS.reduce((t, re) => t.replace(re, ""), s);
      expect(stripped, s).not.toMatch(/\b(above|below)\b/i);
    }
  });
});
