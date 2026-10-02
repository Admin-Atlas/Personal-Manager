// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The words for PM's pick. The pick ranks on memory and size, never on speed, so the reason given
// for it never mentions speed; and "room to spare" is said only of a Comfortable fit, because a
// Tight one has exactly the room the verdict says it doesn't.

import { describe, expect, it } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  LocalFitResult,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalRecommendation,
  LocalRecommendations,
} from "../../lib/types";
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
    expect(why(catalogue())).toBe(
      "The largest model in PM's list that fits entirely on your graphics card — with only a little room to spare.",
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

  it("admits when the figures are for the version PM would pick, not the file served", () => {
    expect(why(owned({ measured: false }))).toMatch(
      / PM can't see which file your server loaded, so these figures are for the version PM would pick\.$/,
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
  const server = (over: Partial<Parameters<typeof settingsLine>[1]> = {}) => ({
    configured: true,
    runner: "Ollama" as const,
    setupShown: false,
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
        server({ configured: false, runner: null, setupShown: true }),
      ),
    ).toBe(
      "PM sized this for a 32k context. Ollama only runs it that way once that's set — the steps are in step 1 below.",
    );
  });

  it("gives LM Studio the two values, and llama-server's command its due", () => {
    expect(settingsLine(fit(), server({ runner: "LM Studio" }))).toBe(
      "PM sized this for a 32k context with a compressed (q8_0) cache. In LM Studio, set the context length to 32768 and the K and V cache quantization to Q8_0 in the model's load settings.",
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
        {
          id: "gemma3:4b",
          matched_repo: "bartowski/gemma-3-4b-it-GGUF",
          fit: fit({ speed_basis: "gpu_published" }),
        },
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
    const r = recs({
      installed: [
        { id: "big:14b", matched_repo: null, fit: fit({ speed_basis: "system" }) },
        { id: "mystery", matched_repo: null, fit: fit() },
        { id: "gemma3:4b", matched_repo: "bartowski/gemma-3-4b-it-GGUF", fit: fit() },
        { id: "qwen-other", matched_repo: QWEN, fit: fit() },
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
  });
});
