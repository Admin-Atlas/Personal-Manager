// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The two readings of an endpoint address that copy leans on: which server it is, and whether it is
// on this computer. The second is a privacy claim — "on this computer" said of a LAN server would be
// false about where someone's chats go — so it is pinned on the near misses, not just the easy case.

import { describe, expect, it } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  DetectedEndpoint,
  LocalDiskSource,
  LocalFitResult,
  LocalGpuResidency,
  LocalInstalledModel,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalRecommendation,
  LocalRecommendations,
  LocalServedModel,
  LocalTestResult,
  PowerRoleView,
  PowerView,
} from "../../lib/types";
import {
  assignPlan,
  COPY_COLLISIONS,
  isLoopback,
  onDiskHow,
  overall,
  primaryOf,
  rightNow,
  runnerOf,
  SAME_SECTION_POINTERS,
  standing,
  steps,
  whereOf,
  type ReadinessInput,
} from "./readiness";
import { sectionLabel } from "./sections";

describe("runnerOf", () => {
  it("names the three servers by the ports PM probes", () => {
    expect(runnerOf("http://127.0.0.1:11434")).toBe("Ollama");
    expect(runnerOf("http://localhost:1234/v1")).toBe("LM Studio");
    expect(runnerOf("http://192.168.1.20:8080")).toBe("llama-server");
  });

  it("parses the port rather than matching text", () => {
    // ":114341" is not 11434, and "11434" in a path is not a port.
    expect(runnerOf("http://localhost:114341")).toBeNull();
    expect(runnerOf(":114341")).toBeNull();
    expect(runnerOf("http://localhost:9000/11434")).toBeNull();
  });

  it("names nothing it can't place", () => {
    expect(runnerOf("http://localhost:9000")).toBeNull();
    expect(runnerOf("not a url")).toBeNull();
    expect(runnerOf("")).toBeNull();
    expect(runnerOf(null)).toBeNull();
    expect(runnerOf(undefined)).toBeNull();
  });
});

describe("isLoopback", () => {
  it("is true only for this computer", () => {
    expect(isLoopback("http://localhost:11434")).toBe(true);
    expect(isLoopback("http://LOCALHOST:11434")).toBe(true);
    expect(isLoopback("http://127.0.0.1:11434")).toBe(true);
    expect(isLoopback("http://127.1:11434")).toBe(true);
    expect(isLoopback("http://[::1]:8080")).toBe(true);
  });

  it("never mistakes a name or another machine for this one", () => {
    expect(isLoopback("http://127.example.com:11434")).toBe(false);
    expect(isLoopback("http://localhost.example.com:11434")).toBe(false);
    expect(isLoopback("http://192.168.1.20:11434")).toBe(false);
    expect(isLoopback("https://my-server.tailnet.ts.net")).toBe(false);
    expect(isLoopback("not a url")).toBe(false);
    expect(isLoopback(null)).toBe(false);
  });

  it("words where a model runs from it", () => {
    expect(whereOf("http://127.0.0.1:11434")).toBe("on this computer");
    expect(whereOf("http://192.168.1.20:11434")).toBe("on your model server");
    expect(whereOf(null)).toBe("on your model server");
  });
});

// ── The start card ─────────────────────────────────────────────────────────────────────────────
//
// Everything the start card says comes from `standing` / `steps` / `overall` / `rightNow`, so these
// are where its honesty is pinned: a sentence about where a job's requests go is worded from the
// backend's `effective` route and nothing else, the card has one "next" step at most, and no string
// here repeats a sentence a section already says — a test that looks for one section's words must
// keep finding exactly one.

const role = (over: Partial<PowerRoleView> = {}): PowerRoleView => ({
  route: "unchanged",
  blocked: null,
  local_model: null,
  effective: "nothing",
  cloud_key: "absent",
  ...over,
});

const status = (
  over: Partial<LocalLlmStatus> = {},
  power: Partial<PowerView> = {},
): LocalLlmStatus => ({
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
  power: { ...INERT_POWER_VIEW, ...power },
  ...over,
});

/** Both roles' routes at once. */
const routes = (chat: Partial<PowerRoleView>, background: Partial<PowerRoleView> = chat) =>
  status({}, { chat: role(chat), background: role(background) });

const cfg = (over: Partial<LocalLlmConfig> = {}): LocalLlmConfig => ({
  base_url: "http://127.0.0.1:11434",
  chat_model: "",
  background_model: "",
  chat_routing: "cloud",
  background_routing: "cloud",
  has_token: false,
  ...over,
});

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

const QWEN = "bartowski/Qwen2.5-7B-Instruct-GGUF";
const QWEN_TAG = `hf.co/${QWEN}:Q5_K_M`;

/** A pick on a card like the dev laptop's (7.96 GB at 384 GB/s): Qwen2.5 7B at Q5_K_M on the card,
 *  5.1 GB to download. */
const catalogue = (over: Partial<Extract<LocalPick, { kind: "catalogue" }>> = {}): LocalPick => ({
  kind: "catalogue",
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  rung: "gpu",
  tag: QWEN_TAG,
  fit: fit(),
  download_gb: 5.07,
  basis: "gpu",
  also_have: null,
  passed_over: null,
  ...over,
});

const qwenRec = (over: Partial<LocalRecommendation> = {}): LocalRecommendation => ({
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  architecture: "qwen2",
  role_hint: "chat",
  parameters_b: 7.62,
  active_parameters_b: 7.62,
  context_length: 32768,
  multimodal: false,
  reasoning: null,
  ollama_pull: `hf.co/${QWEN}:Q8_0`,
  sharded_quant: false,
  gpu_pull: { tag: QWEN_TAG, sharded: false, same_file: false },
  licence: {
    id: "apache-2.0",
    name: "Apache License 2.0",
    url: "https://www.apache.org/licenses/LICENSE-2.0",
    open: true,
    summary: "A permissive open-source licence.",
  },
  fit: fit({ quant: "Q8_0", verdict: "comfortable", est_memory_gb: 10.04, speed_basis: "system" }),
  gpu: { kind: "split", fit: fit() },
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
    vram_gb: 7.96,
    vram_source: "nvidia-smi",
    gpu_bandwidth_gbps: 384,
    unified_memory: false,
    is_wsl: false,
    notes: [],
  },
  reserve_gb: 2,
  gpu_reserve_gb: 1,
  chat_speed: { floor_tps: 30, reply_tokens: 300, reply_secs: 10 },
  catalog_version: 4,
  catalog_generated_at: "2026-09-30",
  endpoint_configured: true,
  cadence: "monthly",
  rescan_due: false,
  curated: [qwenRec()],
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
  pick: catalogue(),
  live_available_ram_gb: 20,
  ...over,
});

const input = (over: Partial<ReadinessInput> = {}): ReadinessInput => ({
  config: cfg(),
  status: status(),
  served: [],
  servedLoaded: true,
  recs: recs(),
  recsLoading: false,
  detected: null,
  detecting: false,
  pull: { tag: null, pct: null },
  lastPulledTag: null,
  tests: { running: null },
  justAssigned: false,
  residency: undefined,
  ...over,
});

const OLLAMA: DetectedEndpoint = { url: "http://127.0.0.1:11434", label: "Ollama", models: [] };
const LMS: DetectedEndpoint = { url: "http://127.0.0.1:1234", label: "LM Studio", models: [] };
const LLAMA: DetectedEndpoint = { url: "http://127.0.0.1:8080", label: "llama-server", models: [] };

const served = (...ids: string[]): LocalServedModel[] =>
  ids.map((id) => ({ id, embedding: false }));

/** A served model's row. `measured`: its figures are for the user's own file at the context the
 *  server serves — an Ollama tag's real size — rather than the catalogue's guess at them.
 *  `spills_gpu`: the backend showed it doesn't fit the card — the only ground for "runs from system
 *  memory". */
const installed = (
  id: string,
  over: Partial<LocalFitResult> = {},
  measured = true,
  spills_gpu = false,
  card_unused = false,
): LocalInstalledModel => ({
  id,
  matched_repo: QWEN,
  fit: fit(over),
  measured,
  spills_gpu,
  card_unused,
  under_chat_floor_tps: null,
});

/** PM's pick as a model the user already has. */
const ownedPick = (
  over: Partial<Extract<LocalPick, { kind: "owned" }>> = {},
): Extract<LocalPick, { kind: "owned" }> => ({
  kind: "owned",
  id: "q.gguf",
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  served: false,
  source: "hugging_face",
  path: "/m/q.gguf",
  shards: 1,
  measured: true,
  fit: fit(),
  basis: "gpu",
  passed_over: null,
  ...over,
});

/** Chat and background both on `model`, both running on it with the given route. */
const both = (model: string, effective: PowerRoleView["effective"], routing = "local") =>
  input({
    config: cfg({
      chat_model: model,
      background_model: model,
      chat_routing: routing,
      background_routing: routing,
    }),
    status: routes({ effective, cloud_key: "present" }),
    served: served(model),
  });

describe("standing — where the user stands, first match wins", () => {
  it("says it is still reading before the config arrives", () => {
    expect(standing(input({ config: null }))).toBe("Reading your local AI setup…");
  });

  it("says the read failed rather than reading forever", () => {
    // Nothing reads the config again until the tab does, so "Reading…" would never end.
    const failed = input({ config: null, configError: "the keychain is locked" });
    expect(standing(failed)).toBe(
      "PM couldn't read your local AI setup, so it can't say where things stand.",
    );
    const [server, ...rest] = steps(failed);
    expect(server).toMatchObject({
      state: "attention",
      line: `PM couldn't read your local AI setup — what went wrong is under ${sectionLabel("sec-localai-endpoint")}.`,
      action: { kind: "locate", to: "sec-localai-endpoint", label: "What to check" },
    });
    for (const s of rest) expect(s.state).toBe("waiting");
    expect(overall(steps(failed), failed)).toEqual({ label: "Needs attention", token: "--st-due" });
    // Once a read lands, the old failure says nothing.
    expect(standing({ ...input(), configError: "earlier" })).not.toContain("couldn't read");
  });

  it("names a server it found, before anything is connected", () => {
    const fresh = { config: cfg({ base_url: null }) };
    expect(standing(input({ ...fresh, detected: [OLLAMA] }))).toBe(
      "Ollama is running on this computer, but PM isn't connected to it yet.",
    );
    // Ollama first: it is the one PM can download into.
    expect(standing(input({ ...fresh, detected: [LMS, OLLAMA] }))).toBe(
      "Ollama and LM Studio are running on this computer, but PM isn't connected to either yet.",
    );
    expect(standing(input({ ...fresh, detected: [LLAMA, LMS, OLLAMA] }))).toBe(
      "Ollama, LM Studio and llama-server are running on this computer, but PM isn't connected to any of them yet.",
    );
  });

  it("says a runner looks installed when its folder is there, readable or not", () => {
    const fresh = { config: cfg({ base_url: null }), detected: [] };
    // LM Studio can be open on screen with its server off — how it starts — so "isn't running"
    // would be false of the app the user is looking at.
    expect(standing(input({ ...fresh, recs: recs({ disk_sources_present: ["lm_studio"] }) }))).toBe(
      "LM Studio looks installed here, but its server isn't on, so PM can't use it yet.",
    );
    expect(
      standing(
        input({
          ...fresh,
          recs: recs({ disk_blocked: [{ source: "ollama", path: "/usr/share/ollama" }] }),
        }),
      ),
    ).toBe("Ollama looks installed here, but it isn't running, so PM can't use it yet.");
  });

  it("says what a fresh setup really uses, from where its requests go", () => {
    const fresh = { config: cfg({ base_url: null }), detected: [] };
    expect(
      standing(input({ ...fresh, status: routes({ effective: "cloud", cloud_key: "present" }) })),
    ).toBe(
      "No local model is set up on this computer yet, so PM uses your cloud model for everything.",
    );
    expect(standing(input({ ...fresh, status: routes({ effective: "nothing" }) }))).toBe(
      "No local model is set up on this computer yet, and there's no cloud key either, so PM has no AI model to use yet.",
    );
    // A job set to Cloud with no key of its own, beside one that has one: nothing more to say.
    expect(
      standing(
        input({
          ...fresh,
          status: routes({ effective: "cloud", cloud_key: "present" }, { effective: "nothing" }),
        }),
      ),
    ).toBe("No local model is set up on this computer yet.");
  });

  it("says a job left on a local model has nothing to answer with until a server is connected", () => {
    // Disconnect keeps the routing. "Nothing" with a key present is that job — not a missing key —
    // and Assign roles shows no rows to say so while nothing is connected.
    const left = {
      config: cfg({ base_url: null, chat_routing: "local" }),
      detected: [],
      status: routes(
        { effective: "nothing", cloud_key: "present" },
        { effective: "cloud", cloud_key: "present" },
      ),
    };
    expect(standing(input(left))).toBe(
      "No local model is set up on this computer yet. Chat is set to run on a local model, so it can't answer until PM is connected to a model server.",
    );
    expect(standing(input({ ...left, detected: [OLLAMA] }))).toBe(
      "Ollama is running on this computer, but PM isn't connected to it yet. Chat is set to run on a local model, so it can't answer until PM is connected to a model server.",
    );
    const bothLeft = input({
      ...left,
      config: cfg({ base_url: null, chat_routing: "local", background_routing: "local" }),
      status: routes({ effective: "nothing", cloud_key: "present" }),
      recs: recs({ disk_sources_present: ["ollama"] }),
    });
    expect(standing(bothLeft)).toBe(
      "Ollama looks installed here, but it isn't running, so PM can't use it yet. Chat and background work are set to run on a local model, so they can't answer until PM is connected to a model server.",
    );
  });

  it("is checking once connected and before the status arrives", () => {
    expect(standing(input({ status: null }))).toBe("Checking your model server…");
  });

  it("says what an unreachable or paused server means for each job", () => {
    const down = (
      over: Partial<LocalLlmStatus>,
      chat: Partial<PowerRoleView>,
      bg: Partial<PowerRoleView>,
    ) => standing(input({ status: { ...routes(chat, bg), ...over } }));
    expect(
      down({ reachable: false }, { effective: "local_then_cloud" }, { effective: "local_only" }),
    ).toBe(
      "PM can't reach your model server right now. Chat is using your cloud model until it's back. Background work can't answer until it's back.",
    );
    expect(
      down(
        { in_cooldown: true, reachable: false },
        { effective: "local_only" },
        { effective: "local_only" },
      ),
    ).toBe(
      "PM has paused calls to your model server for a moment, after several failed in a row. Chat and background work can't answer until it's back.",
    );
    expect(down({ reachable: false }, { effective: "cloud" }, { effective: "cloud" })).toBe(
      "PM can't reach your model server right now.",
    );
  });

  it("tells an empty server from one holding only embedders", () => {
    expect(standing(input({ served: [] }))).toBe("Your model server is running, and it's empty.");
    expect(standing(input({ served: [{ id: "nomic-embed-text", embedding: true }] }))).toBe(
      "Your model server is running, but it only has embedding models, which can't answer chat or do background work.",
    );
  });

  it("says a ready server that nothing uses is unused", () => {
    expect(
      standing(
        input({
          served: served("qwen2.5:7b"),
          status: routes({ effective: "cloud", cloud_key: "present" }),
        }),
      ),
    ).toBe("Your model server is ready, but PM isn't using it for anything yet.");
  });

  it("says 'on this computer' only of a loopback address", () => {
    expect(standing(both("qwen2.5:7b", "local_only"))).toBe(
      "Chat and background work run on qwen2.5:7b, on this computer, and never use the cloud.",
    );
    const lan = both("qwen2.5:7b", "local_then_cloud", "local-then-cloud");
    lan.config = cfg({ ...lan.config, base_url: "http://192.168.1.20:11434" });
    expect(standing(lan)).toBe(
      "Chat and background work run on qwen2.5:7b, on your model server, with your cloud model as a fallback.",
    );
  });

  it("words each job from its own route, never from its routing alone", () => {
    const i = input({
      config: cfg({
        chat_model: "qwen2.5:7b",
        chat_routing: "local-then-cloud",
        background_routing: "local",
      }),
      served: served("qwen2.5:7b"),
      // Background is set to Local with no model: it answers nothing — it does NOT quietly use the
      // cloud, which is the claim this section must never make.
      status: routes(
        { effective: "local_then_cloud", cloud_key: "present" },
        { effective: "nothing", cloud_key: "present" },
      ),
    });
    expect(standing(i)).toBe(
      "Chat runs on qwen2.5:7b, on this computer, with your cloud model as a fallback. Background work has nothing to answer with — no local model is chosen.",
    );
    const keyless = input({
      config: cfg({ chat_model: "qwen2.5:7b", chat_routing: "local" }),
      served: served("qwen2.5:7b"),
      status: routes({ effective: "local_only" }, { effective: "nothing" }),
    });
    expect(standing(keyless)).toBe(
      "Chat runs on qwen2.5:7b, on this computer, and never uses the cloud. Background work has nothing to answer with — there's no cloud key.",
    );
  });

  it("adds the battery move, a model gone from the server, a model too big for the card, and unreadable keys", () => {
    const i = input({
      config: cfg({
        chat_model: "big:70b",
        background_model: "gone:7b",
        chat_routing: "local-then-cloud",
        background_routing: "local-then-cloud",
      }),
      served: served("big:70b"),
      // A row the backend can send: a measured file over the card, which the backend marks as
      // spilling — the only row the claim is made of.
      recs: recs({ installed: [installed("big:70b", { speed_basis: "system" }, true, true)] }),
      status: routes(
        { effective: "cloud_for_power", cloud_key: "present", route: "cloud" },
        { effective: "unknown", cloud_key: "unreadable" },
      ),
    });
    expect(standing(i)).toBe(
      "Chat runs on big:70b, on this computer, with your cloud model as a fallback. Background work runs on gone:7b, on this computer. On battery, chat is on your cloud model for now. Your server isn't serving gone:7b right now. big:70b runs at least partly from system memory rather than your graphics card — expect slow replies. PM can't read your saved keys right now, so it can't say whether background work would fall back to the cloud.",
    );
  });

  it("says a model runs from system memory only when PM can show it doesn't fit the card", () => {
    // The row's own sizing can't carry the claim. LM Studio has no /api/tags, so its row is the
    // catalogue's guess — Q8_0 at 10 GB, because 20 GB is free — for a Q4_K_M that fits the card.
    // And a measured row is sized f16 first, high by design: the dev laptop serving Qwen2.5 7B Q6_K
    // at a proven 32768 comes to 8.07 GB on f16, over its 7.96 GB card, and 7.25 GB on the card on
    // the q8_0 cache PM's own tuning says to set. Only `spills_gpu` — the server holding less of it on
    // the card than its size, or the file over the whole card even on q8_0 — says it.
    const say = (pick: LocalPick, measured: boolean, spills: boolean, card = true) =>
      standing({
        ...both("qwen2.5-7b-instruct", "local_only"),
        config: cfg({
          base_url: "http://127.0.0.1:1234",
          chat_model: "qwen2.5-7b-instruct",
          background_model: "qwen2.5-7b-instruct",
          chat_routing: "local",
          background_routing: "local",
        }),
        recs: recs({
          ...(card ? {} : { hardware: { ...recs().hardware, vram_gb: null } }),
          pick,
          installed: [
            installed(
              "qwen2.5-7b-instruct",
              { quant: "Q8_0", est_memory_gb: 10.04, speed_basis: "system" },
              measured,
              spills,
            ),
          ],
        }),
      });
    const clean =
      "Chat and background work run on qwen2.5-7b-instruct, on this computer, and never use the cloud.";
    expect(say(catalogue(), false, false)).toBe(clean);
    expect(say(catalogue(), true, false)).toBe(clean);
    // Shown to spill, it is said.
    expect(say(catalogue(), true, true)).toBe(
      `${clean} qwen2.5-7b-instruct runs at least partly from system memory rather than your graphics card — expect slow replies.`,
    );
    // Never of the pick itself, which was judged against the card — the card beside it says it fits.
    expect(
      say(
        ownedPick({ id: "qwen2.5-7b-instruct", served: true, source: null, path: null }),
        true,
        true,
      ),
    ).toBe(clean);
    // And never without a separate card to spill off.
    expect(say(catalogue(), true, true, false)).toBe(clean);
  });

  it("says when the server isn't using the card at all, even of PM's own pick", () => {
    const say = (pick: LocalPick, card = true) =>
      standing({
        ...both("qwen2.5-7b-instruct", "local_only"),
        config: cfg({
          base_url: "http://127.0.0.1:11434",
          chat_model: "qwen2.5-7b-instruct",
          background_model: "qwen2.5-7b-instruct",
          chat_routing: "local",
          background_routing: "local",
        }),
        recs: recs({
          ...(card ? {} : { hardware: { ...recs().hardware, vram_gb: null } }),
          pick,
          installed: [installed("qwen2.5-7b-instruct", {}, true, false, true)],
        }),
      });
    const clean =
      "Chat and background work run on qwen2.5-7b-instruct, on this computer, and never use the cloud.";
    const cardless = `${clean} Your server isn't using your graphics card, so qwen2.5-7b-instruct runs entirely from system memory — expect slow replies. Check that it can see the card; a different model won't help.`;
    expect(say(catalogue())).toBe(cardless);
    expect(
      say(ownedPick({ id: "qwen2.5-7b-instruct", served: true, source: null, path: null })),
    ).toBe(cardless);
    expect(say(catalogue(), false)).toBe(clean);
  });
});

describe("steps — four, in order, with at most one next", () => {
  /** A spread of the states the spec's table walks through. */
  const STATES: Record<string, ReadinessInput> = {
    loading: input({ config: null }),
    fresh: input({ config: cfg({ base_url: null }), detected: [] }),
    looking: input({ config: cfg({ base_url: null }), detecting: true }),
    detected: input({ config: cfg({ base_url: null }), detected: [LMS, OLLAMA] }),
    checking: input({ status: null }),
    unreachable: input({ status: status({ reachable: false }) }),
    cooldown: input({ status: status({ reachable: false, in_cooldown: true }) }),
    unlisted: input({ servedLoaded: false }),
    empty: input({ served: [] }),
    pulling: input({ pull: { tag: QWEN_TAG, pct: 40 } }),
    otherPull: input({ pull: { tag: "hf.co/x/y:Q4_K_M", pct: 10 } }),
    servedUnassigned: input({ served: served(QWEN_TAG), lastPulledTag: QWEN_TAG }),
    assigned: { ...both(QWEN_TAG, "local_only"), justAssigned: true },
    returning: both(QWEN_TAG, "local_only"),
    smallWindow: {
      ...both(QWEN_TAG, "local_only"),
      status: {
        ...routes({ effective: "local_only" }),
        served_window: 4096,
        served_window_proven: true,
        window_source: "slots",
      },
    },
    noPick: input({ recs: recs({ pick: undefined }), served: [] }),
    nothing: input({
      recs: recs({
        pick: { kind: "nothing", reason: "too_slow", basis: "system", system_fallback: false },
      }),
      served: [],
    }),
    readFailed: input({ config: null, configError: "the keychain is locked" }),
    onBattery: { ...both(QWEN_TAG, "cloud_for_power", "local-then-cloud"), justAssigned: true },
    ownFile: { ...both("my-finetune", "local_only"), recs: recs({ pick: ownedPick() }) },
    modelGone: { ...both("gone:7b", "local_only"), served: [] },
    reconnected: input({
      config: cfg({ chat_routing: "local-then-cloud", background_routing: "local-then-cloud" }),
      served: served(QWEN_TAG),
      status: routes({ effective: "cloud", cloud_key: "present" }),
    }),
  };

  it("never has more than one next, and it is the first step still to do", () => {
    for (const [name, i] of Object.entries(STATES)) {
      const list = steps(i);
      const next = list.filter((s) => s.state === "next");
      expect(next.length, name).toBeLessThanOrEqual(1);
      if (next.length === 1) {
        const first = list.find(
          (s) => s.state !== "done" && s.state !== "optional" && s.state !== "checking",
        );
        expect(next[0].n, name).toBe(first?.n);
      }
      // A primary only ever on the next step, and only for the four kinds that do the step.
      for (const s of list) {
        const p = primaryOf(s);
        if (p) {
          expect(s.state, name).toBe("next");
          expect(["connect", "download", "assign", "test"], name).toContain(p.kind);
        }
      }
      expect(list.filter((s) => primaryOf(s)).length, name).toBeLessThanOrEqual(1);
    }
  });

  it("makes the rest wait while there is no server", () => {
    for (const name of ["fresh", "unreachable", "cooldown", "checking"]) {
      const [, ...rest] = steps(STATES[name]);
      for (const s of rest) {
        expect(s.state, `${name} step ${s.n}`).toBe("waiting");
        expect(s.line).toBe("After step 1.");
        expect(s.action).toBeNull();
      }
    }
  });

  it("step 1 reads only the status once connected — never the probe", () => {
    const [server] = steps(input({ detected: [LMS] }));
    expect(server.state).toBe("done");
    expect(server.line).toBe("Your server is answering at http://127.0.0.1:11434.");
    const [down] = steps(STATES.unreachable);
    expect(down.state).toBe("attention");
    expect(down.line).toBe(
      "PM can't reach your server at http://127.0.0.1:11434 — usually that means it isn't running.",
    );
    expect(down.action).toMatchObject({
      kind: "locate",
      to: "sec-localai-endpoint",
      label: "What to check",
    });
    expect(steps(STATES.cooldown)[0].line).toBe(
      "PM paused calls to your server after several failed in a row, and tries again by itself.",
    );
  });

  it("step 1 offers the server it found, Ollama first", () => {
    const [server] = steps(STATES.detected);
    expect(server.state).toBe("next");
    expect(server.action).toEqual({ kind: "connect", url: OLLAMA.url, label: "Connect to Ollama" });
    expect(server.secondary).toEqual([
      { kind: "connect", url: LMS.url, label: "Connect to LM Studio instead" },
    ]);
    expect(server.notes).toEqual(["Ollama is the only one PM can download models into for you."]);
    expect(server.setup).toBeNull();
  });

  it("step 1 shows the guide for the runner it can see, or Ollama's, with a way to look again", () => {
    const [none] = steps(STATES.fresh);
    expect(none.setup).toBe("Ollama");
    expect(none.action).toBeNull();
    expect(none.secondary).toEqual([{ kind: "detect", label: "Look now" }]);
    expect(none.notes).toEqual([
      "PM looks for a running server every half minute while this tab is open.",
    ]);
    const [lms] = steps(
      input({
        config: cfg({ base_url: null }),
        detected: [],
        recs: recs({ disk_sources_present: ["lm_studio"] }),
      }),
    );
    expect(lms.setup).toBe("LM Studio");
    // Opening the app isn't enough: its server starts off, and PM would never notice.
    expect(lms.line).toBe(
      "LM Studio looks installed — PM found its model folder — but its server isn't answering. Open LM Studio and switch its server on (the toggle at the top of its Developer tab), and PM notices within about half a minute.",
    );
    const [ollama] = steps(
      input({
        config: cfg({ base_url: null }),
        detected: [],
        recs: recs({ disk_sources_present: ["ollama"] }),
      }),
    );
    expect(ollama.line).toBe(
      "Ollama looks installed — PM found its model folder — but it isn't answering. Start it, and PM notices within about half a minute.",
    );
    expect(steps(STATES.looking)[0]).toMatchObject({
      state: "checking",
      line: "Looking for a model server on this computer…",
    });
  });

  it("step 2 offers the pick's download on an Ollama, naming its size and its licence's terms", () => {
    const model = steps(STATES.empty)[1];
    expect(model.title).toBe("Get the model");
    expect(model.action).toEqual({
      kind: "download",
      repo: QWEN,
      tag: QWEN_TAG,
      label: "Download (5.1 GB)",
    });
    expect(model.line).toBe(
      "A 5.1 GB download. PM expects it to reply at about 71 tok/s on your graphics card, so an answer of a few paragraphs takes about 4 seconds. Your Ollama fetches it from Hugging Face — PM doesn't download anything itself.",
    );
    expect(model.tag).toBe(QWEN_TAG);
    const restricted = recs({
      curated: [
        qwenRec({
          licence: { id: "qwen", name: "Qwen License", url: "https://x", open: false, summary: "" },
        }),
      ],
    });
    expect(steps(input({ served: [], recs: restricted }))[1].line).toContain(
      " Its licence has its own terms, which PM shows you before the download starts.",
    );
    expect(
      steps(input({ served: [], recs: { ...restricted, terms_accepted: ["qwen"] } }))[1].line,
    ).not.toContain("own terms");
  });

  it("step 2 gives every other server the command PM sized, since it can't download into them", () => {
    const on = (base_url: string) => steps(input({ served: [], config: cfg({ base_url }) }))[1];
    const lms = on("http://127.0.0.1:1234");
    expect(lms.action).toBeNull();
    expect(lms.command).toBe(QWEN);
    expect(lms.line).toBe(
      "PM can't download into LM Studio. In LM Studio's Discover tab, search for the name below, download its Q5_K_M file, then load it.",
    );
    const llama = on("http://127.0.0.1:8080");
    expect(llama.command).toBe(
      `llama-server -hf ${QWEN}:Q5_K_M --ctx-size 32768 -np 1 -fa on -ctk q8_0 -ctv q8_0`,
    );
    const other = on("http://127.0.0.1:9000");
    expect(other.line).toBe(
      `PM can only download into an Ollama on its usual port (11434). Get ${QWEN} at Q5_K_M into your server — with llama-server, that's:`,
    );
    expect(other.command).toBe(llama.command);
    expect(other.action).toBeNull();
  });

  it("step 2 says how fast PM expects the pick to reply, and how long an answer takes", () => {
    const withFit = (over: Partial<LocalFitResult>, base_url = "http://127.0.0.1:11434") =>
      steps(
        input({
          served: [],
          config: cfg({ base_url }),
          recs: recs({ pick: catalogue({ fit: fit(over) }) }),
        }),
      )[1];
    // From system memory: PM's rough guide, not a claim about a card.
    expect(withFit({ speed_basis: "system", est_tokens_per_sec: 12.4 }).line).toBe(
      "A 5.1 GB download. From system memory, PM's rough guide puts it at about 12 tok/s, so an answer of a few paragraphs takes about 24 seconds. Your Ollama fetches it from Hugging Face — PM doesn't download anything itself.",
    );
    // Shared memory gets no number, so no sentence: exactly the line without one.
    const plain =
      "A 5.1 GB download. Your Ollama fetches it from Hugging Face — PM doesn't download anything itself.";
    expect(withFit({ speed_basis: "shared" }).line).toBe(plain);
    expect(withFit({ speed_basis: null, est_tokens_per_sec: null }).line).toBe(plain);
    // An unrecognised card is still a card.
    expect(withFit({ speed_basis: "gpu_typical" }).line).toContain(
      "PM expects it to reply at about 71 tok/s on your graphics card",
    );
    // Never under one second, and singular when it is one.
    expect(withFit({ est_tokens_per_sec: 300 }).line).toContain(
      "so an answer of a few paragraphs takes about 1 second.",
    );
    // The button stays the size alone.
    expect(withFit({}).action).toMatchObject({ label: "Download (5.1 GB)" });

    // The servers PM can't download into: the speed comes first among the notes, and the line and
    // command are the ones about getting the file.
    const speed =
      "PM expects it to reply at about 71 tok/s on your graphics card, so an answer of a few paragraphs takes about 4 seconds.";
    for (const url of ["http://127.0.0.1:1234", "http://127.0.0.1:8080", "http://127.0.0.1:9000"]) {
      const step = steps(input({ served: served("gemma3:4b"), config: cfg({ base_url: url }) }))[1];
      expect(step.notes, url).toEqual([
        speed,
        `Your server already has 1 other model — you can give one a job under ${sectionLabel("sec-localai-roles")} instead.`,
      ]);
      expect(step.line, url).not.toContain("tok/s");
      expect(withFit({ speed_basis: "shared" }, url).notes, url).toEqual([]);
    }

    // A pick that is a file on this computer, not yet served: the speed is its one note.
    const onDisk = steps(
      input({
        served: [],
        config: cfg({ base_url: "http://127.0.0.1:8080" }),
        recs: recs({ pick: ownedPick() }),
      }),
    )[1];
    expect(onDisk.notes).toEqual([speed]);
    expect(
      steps(
        input({
          served: [],
          config: cfg({ base_url: "http://127.0.0.1:8080" }),
          recs: recs({ pick: ownedPick({ fit: fit({ speed_basis: "shared" }) }) }),
        }),
      )[1].notes,
    ).toEqual([]);
  });

  it("step 2 says when the server already has something else to use", () => {
    const model = steps(input({ served: served("gemma3:4b") }))[1];
    expect(model.notes).toEqual([
      `Your server already has 1 other model — you can give one a job under ${sectionLabel("sec-localai-roles")} instead.`,
    ]);
  });

  it("step 2 shows the pick's own download in place of its button, and another one only by name", () => {
    const pulling = steps(STATES.pulling)[1];
    expect(pulling).toMatchObject({ state: "next", progress: true, action: null, line: null });
    expect(pulling.notes).toEqual(["You can leave this tab — the download carries on."]);
    const other = steps(STATES.otherPull)[1];
    expect(other.progress).toBe(false);
    expect(other.action).toBeNull();
    expect(other.line).toBe(
      `Downloading hf.co/x/y:Q4_K_M under ${sectionLabel("sec-localai-models")} — PM can fetch its pick when that finishes.`,
    );
  });

  it("step 2 is done for a returning user whose jobs run on a model other than the pick", () => {
    // Someone who chose gemma on purpose must not be told "Setting up" for as long as they keep it,
    // with a primary Download pointed at a model they didn't pick.
    const own = both("gemma3:4b", "local_only");
    const list = steps(own);
    expect(list[1]).toMatchObject({ state: "done", action: null });
    expect(list[1].line).toContain("PM's pick for this computer, if you'd like to try it");
    expect(overall(list, own)?.label).toBe("Set up");
  });

  it("step 2 is done the same way when the pick is a file on disk", () => {
    // A llama-server user serving their own fine-tune, with a catalogue GGUF in the Hugging Face
    // cache that became the pick: the serve command would replace the model they chose.
    const llama = {
      ...both("my-finetune", "local_only"),
      config: cfg({
        base_url: "http://127.0.0.1:8080",
        chat_model: "my-finetune",
        background_model: "my-finetune",
        chat_routing: "local",
        background_routing: "local",
      }),
      recs: recs({ pick: ownedPick() }),
    };
    const list = steps(llama);
    expect(list[1]).toMatchObject({ state: "done", action: null, command: null });
    expect(list[1].line).toBe(
      `Your jobs already run on a model your server has. Qwen2.5 7B Instruct is PM's pick for this computer, if you'd like to try it — it's under ${sectionLabel("sec-localai-downloaded")} too.`,
    );
    expect(overall(list, llama)?.label).toBe("Set up");
    // With nothing at work on it yet, serving the file is the step.
    const idle = steps({ ...llama, config: cfg({ base_url: "http://127.0.0.1:8080" }) })[1];
    expect(idle).toMatchObject({
      state: "next",
      line: "It's a file on this computer. llama-server can serve it as it is:",
    });
  });

  it("step 2 counts a job as at work only on a model the server is serving", () => {
    // The user removed the bound model (`ollama rm`): its route still reads Local only — the route
    // only knows a model is chosen — but the server is empty, and the pick's download is exactly
    // what it needs.
    const gone = (servedNow: LocalServedModel[]) =>
      input({
        config: cfg({
          chat_model: "gone:7b",
          background_model: "gone:7b",
          chat_routing: "local",
          background_routing: "local",
        }),
        served: servedNow,
        status: routes({ effective: "local_only" }),
      });
    const empty = steps(gone([]));
    expect(empty[1]).toMatchObject({
      state: "next",
      action: { kind: "download", tag: QWEN_TAG, label: "Download (5.1 GB)" },
    });
    expect(primaryOf(empty[1])).not.toBeNull();
    expect(empty[2].line).toBe(
      "Chat is set to gone:7b, which your server isn't serving right now.",
    );
    const other = steps(gone(served("other:3b")))[1];
    expect(other.state).toBe("next");
    expect(other.line).not.toContain("already run on a model your server has");
    // The same for a pick that is a file on disk.
    expect(steps({ ...gone([]), recs: recs({ pick: ownedPick() }) })[1].state).toBe("next");
  });

  it("step 2 is done once the server has the pick", () => {
    expect(steps(STATES.servedUnassigned)[1]).toMatchObject({
      state: "done",
      line: "Your server has it.",
    });
    const owned = recs({
      pick: {
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
        passed_over: null,
      },
    });
    expect(steps(input({ recs: owned, served: served("qwen2.5:latest") }))[1].state).toBe("done");
  });

  it("step 2 says how to serve a pick that is a file on this computer", () => {
    // A file on disk is an owned option only for a server that could serve it (local_ai.rs
    // `runner_can_serve`): Ollama's folder for Ollama, LM Studio's for LM Studio, any other file for
    // llama-server — each on its own port on this computer. So that is the endpoint step 2 sees.
    const SERVER: Record<LocalDiskSource, string> = {
      ollama: "http://127.0.0.1:11434",
      lm_studio: "http://127.0.0.1:1234",
      hugging_face: "http://127.0.0.1:8080",
      folder: "http://127.0.0.1:8080",
    };
    const disk = (source: LocalDiskSource, path: string, shards = 1) =>
      steps(
        input({
          served: [],
          config: cfg({ base_url: SERVER[source] }),
          recs: recs({
            pick: {
              kind: "owned",
              id: "q.gguf",
              repo: QWEN,
              display_name: "Qwen2.5 7B Instruct",
              served: false,
              source,
              path,
              shards,
              measured: true,
              fit: fit(),
              basis: "gpu",
              passed_over: null,
            },
          }),
        }),
      )[1];
    expect(disk("hugging_face", "/m/q.gguf")).toMatchObject({
      line: "It's a file on this computer. llama-server can serve it as it is:",
      command: 'llama-server -m "/m/q.gguf" --ctx-size 32768 -np 1 -fa on -ctk q8_0 -ctv q8_0',
    });
    // A path PM can't quote gets no command rather than one that breaks.
    expect(disk("folder", '/m/"odd".gguf').command).toBeNull();
    expect(disk("folder", "/m/q-00001-of-00003.gguf", 3)).toMatchObject({
      line: "It's split into 3 files, so PM can't give you a one-line command to serve it.",
      command: null,
    });
    // Step 2 is only ever shown connected, and a file in LM Studio's or Ollama's folder only counts
    // for a server on that runner's port — so that runner is connected, and isn't serving it.
    expect(disk("lm_studio", "/m/q.gguf")).toMatchObject({
      line: "It's in LM Studio. Load it there — PM sees it within about half a minute.",
      command: null,
    });
    expect(disk("ollama", "/m/blobs").line).toBe(
      "It's in an Ollama folder on this computer, but the Ollama PM is connected to isn't serving it — it probably keeps its models somewhere else.",
    );
  });

  it("words a file on disk for a server that isn't connected yet, for Already on this device", () => {
    const f = fit();
    expect(onDiskHow("ollama", 1, "/m/blobs", f).line).toBe(
      "It's in Ollama's folder — once Ollama is connected, it shows up by itself.",
    );
    expect(onDiskHow("lm_studio", 1, "/m/q.gguf", f).line).toBe(
      "It's in LM Studio. Load it there and switch LM Studio's server on — PM sees it within about half a minute.",
    );
    expect(onDiskHow("ollama", 1, "/m/blobs", f, "http://127.0.0.1:11434").line).not.toContain(
      "once Ollama is connected",
    );
  });

  it("words a file for the server PM is really connected to, which may be another runner's", () => {
    // Already on this device lists every runner's unserved files whatever is connected — only PM's
    // pick is limited to the connected runner's (local_ai.rs `runner_can_serve`). So "the Ollama PM
    // is connected to" is said only while that is this computer's Ollama, and "PM sees it" of an LM
    // Studio file only while LM Studio is the server.
    const f = fit();
    const ollama = (url: string | null) => onDiskHow("ollama", 1, "/m/blobs", f, url).line;
    const lms = (url: string | null) => onDiskHow("lm_studio", 1, "/m/q.gguf", f, url).line;
    const CONNECTED_OLLAMA =
      "It's in an Ollama folder on this computer, but the Ollama PM is connected to isn't serving it — it probably keeps its models somewhere else.";
    const CONNECTED_LMS =
      "It's in LM Studio. Load it there — PM sees it within about half a minute.";

    // Its own runner, on this computer: the connected words.
    expect(ollama("http://127.0.0.1:11434")).toBe(CONNECTED_OLLAMA);
    expect(ollama("http://localhost:11434")).toBe(CONNECTED_OLLAMA);
    expect(lms("http://127.0.0.1:1234")).toBe(CONNECTED_LMS);

    // Another runner: says which, and what using the file would take.
    expect(ollama("http://127.0.0.1:1234")).toBe(
      "It's in Ollama's folder on this computer, but PM is connected to LM Studio — once this computer's Ollama is connected instead, it shows up by itself.",
    );
    expect(ollama("http://127.0.0.1:8080")).toBe(
      "It's in Ollama's folder on this computer, but PM is connected to llama-server — once this computer's Ollama is connected instead, it shows up by itself.",
    );
    expect(lms("http://127.0.0.1:11434")).toBe(
      "It's in LM Studio, but PM is connected to Ollama. To use it, load it in LM Studio, switch LM Studio's server on and connect PM to that instead.",
    );
    // The same runner somewhere else is not this computer's.
    expect(ollama("http://192.168.1.20:11434")).toBe(
      "It's in Ollama's folder on this computer, but PM is connected to the Ollama on your model server — once this computer's Ollama is connected instead, it shows up by itself.",
    );
    // A port PM doesn't know is no runner's own.
    expect(lms("http://127.0.0.1:9000")).toBe(
      "It's in LM Studio, but PM is connected to a server that isn't LM Studio on its usual port (1234). To use it, load it in LM Studio, switch LM Studio's server on and connect PM to that instead.",
    );
    for (const url of [
      "http://127.0.0.1:1234",
      "http://127.0.0.1:8080",
      "http://192.168.1.20:11434",
    ])
      expect(ollama(url)).not.toBe(CONNECTED_OLLAMA);
    for (const url of [
      "http://127.0.0.1:11434",
      "http://127.0.0.1:8080",
      "http://192.168.1.20:1234",
    ])
      expect(lms(url)).not.toBe(CONNECTED_LMS);
    // Any other file is llama-server's to serve, whatever is connected.
    expect(onDiskHow("hugging_face", 1, "/m/q.gguf", f, "http://127.0.0.1:11434").line).toBe(
      "It's a file on this computer. llama-server can serve it as it is:",
    );
  });

  it("doesn't diagnose a file's own server from a look that got no answer", () => {
    // With the connected server down, every file of its runner is listed, served or not, so
    // "isn't serving it" would be a guess — and for LM Studio, switching the server on is the
    // step that's missing.
    const f = fit();
    expect(onDiskHow("ollama", 1, "/m/blobs", f, "http://127.0.0.1:11434", false).line).toBe(
      "It's in Ollama's folder on this computer, and the Ollama PM is connected to isn't answering — once it's running, this shows up by itself.",
    );
    expect(onDiskHow("lm_studio", 1, "/m/q.gguf", f, "http://127.0.0.1:1234", false).line).toBe(
      "It's in LM Studio, and the LM Studio server PM is connected to isn't answering. Load it there and switch LM Studio's server on — PM sees it within about half a minute.",
    );
    // Another runner's file reads the same whether or not the connected server answered.
    expect(onDiskHow("ollama", 1, "/m/blobs", f, "http://127.0.0.1:1234", false).line).toBe(
      onDiskHow("ollama", 1, "/m/blobs", f, "http://127.0.0.1:1234", true).line,
    );
  });

  it("step 2 is 'Choose a model' without a pick, and sends the reader to All models", () => {
    const absent = steps(STATES.noPick)[1];
    expect(absent.title).toBe("Choose a model");
    expect(absent.line).toBe(
      `Choose one under ${sectionLabel("sec-localai-models")}; each says how it would run here.`,
    );
    expect(absent.action).toEqual({
      kind: "locate",
      to: "catalog",
      label: `Go to ${sectionLabel("sec-localai-models")}`,
    });
    const nothing = steps(STATES.nothing)[1];
    expect(nothing.title).toBe("Choose a model");
    expect(nothing.line).toBe(
      `PM isn't picking a model for this computer — the reason is just above. You can still choose one under ${sectionLabel("sec-localai-models")}; each says how it would run here.`,
    );
    expect(
      steps(input({ recs: recs({ pick: undefined }), served: served("a", "b") }))[1],
    ).toMatchObject({
      state: "done",
      line: "Your server has 2 models you can use.",
    });
    expect(
      steps(
        input({ recs: recs({ pick: undefined }), served: [{ id: "nomic", embedding: true }] }),
      )[1].line,
    ).toBe(
      "Your server only has embedding models, which can't hold a conversation. It needs a chat model too.",
    );
  });

  it("step 3 offers to put the model just downloaded on both jobs", () => {
    const roles = steps(STATES.servedUnassigned)[2];
    expect(roles.state).toBe("next");
    expect(roles.action).toMatchObject({
      kind: "assign",
      label: "Use Qwen2.5 7B Instruct for both",
    });
    const plan = roles.action?.kind === "assign" ? roles.action.plan : null;
    expect(plan?.models).toEqual({ chat: QWEN_TAG, background: QWEN_TAG });
    // No key in the fixture: both go Local only.
    expect(plan?.routing).toEqual({ chat: "local", background: "local" });
    expect(roles.line).toBe(plan?.sentence);
  });

  it("step 3 never offers an embedder, and without a candidate points at Assign roles", () => {
    // No pick, so step 2 is done by the server's own models and step 3 is the one to do.
    const roles = steps(
      input({
        recs: recs({ pick: undefined }),
        served: [...served("a", "b"), { id: "embed", embedding: true }],
      }),
    )[2];
    expect(roles.action).toEqual({
      kind: "locate",
      to: "sec-localai-roles",
      label: `Go to ${sectionLabel("sec-localai-roles")}`,
    });
    expect(roles.line).toBe(
      `Choose which model answers chat and which does background work under ${sectionLabel("sec-localai-roles")}.`,
    );
    const single = steps(
      input({
        recs: recs({ pick: undefined }),
        served: [{ id: "embed", embedding: true }, ...served("only:1b")],
      }),
    )[2];
    expect(single.action).toMatchObject({ kind: "assign", label: "Use only:1b for both" });
  });

  it("step 3 catches the traps: a local job with no model, a model the server lost, two that swap", () => {
    const noModel = steps(
      input({
        config: cfg({ chat_routing: "local-then-cloud" }),
        served: served("a"),
        status: routes({ effective: "cloud", cloud_key: "present" }),
      }),
    )[2];
    expect(noModel.state).toBe("attention");
    expect(noModel.line).toBe(
      "Chat is set to run on your server but has no model chosen, so it uses your cloud model.",
    );
    const nothing = steps(
      input({
        config: cfg({ background_routing: "local" }),
        served: served("a"),
        status: routes({ effective: "nothing" }),
      }),
    )[2];
    expect(nothing.line).toBe(
      "Background work is set to run on your server but has no model chosen, so it has nothing to answer with.",
    );
    const lost = steps(
      input({
        config: cfg({ chat_model: "gone:7b", chat_routing: "local" }),
        served: served("a"),
        status: routes({ effective: "local_only" }, { effective: "cloud", cloud_key: "present" }),
      }),
    )[2];
    expect(lost.line).toBe("Chat is set to gone:7b, which your server isn't serving right now.");
    expect(lost.action).toMatchObject({ kind: "locate", to: "sec-localai-roles" });
    const swap = steps(
      input({
        config: cfg({
          chat_model: "a:14b",
          background_model: "b:7b",
          chat_routing: "local",
          background_routing: "local",
        }),
        served: served("a:14b", "b:7b"),
        status: routes({ effective: "local_only" }),
        recs: recs({
          co_residency: {
            ram: "fits",
            vram: "exceeds",
            combined_gb: 14,
            ram_budget_gb: 30,
            vram_budget_gb: 7,
          },
        }),
      }),
    )[2];
    expect(swap.line).toBe(
      "a:14b and b:7b are too big to keep loaded together, so your server swaps between them, a few seconds each time.",
    );
    // It writes background work's model only, and is not Assign roles' own button name.
    expect(swap.secondary).toEqual([
      {
        kind: "assign",
        label: "Use a:14b for both jobs",
        plan: {
          models: { chat: null, background: "a:14b" },
          routing: { chat: null, background: null },
          sentence: "",
        },
      },
    ]);
  });

  it("step 3 offers the one-click way out of the no-model trap, saying what it writes", () => {
    // Switching servers: Disconnect clears both models and keeps the routing, so the reconnect lands
    // in the trap with a server full of models.
    const reconnected = input({
      config: cfg({ chat_routing: "local-then-cloud", background_routing: "local-then-cloud" }),
      served: served(QWEN_TAG),
      status: routes({ effective: "cloud", cloud_key: "present" }),
    });
    const roles = steps(reconnected)[2];
    expect(roles.state).toBe("attention");
    expect(roles.action).toMatchObject({ kind: "locate", to: "sec-localai-roles" });
    expect(roles.secondary).toHaveLength(1);
    const use = roles.secondary[0];
    expect(use).toMatchObject({ kind: "assign", label: "Use Qwen2.5 7B Instruct for both" });
    const plan = use.kind === "assign" ? use.plan : null;
    expect(plan?.models).toEqual({ chat: QWEN_TAG, background: QWEN_TAG });
    expect(plan?.routing).toEqual({ chat: null, background: null });
    // Said before the click, in one space-separated sentence.
    expect(roles.notes).toEqual([
      "One model for both jobs — chat and background work — so your server only ever holds one. Chat keeps its current setting, Local, fall back to cloud. Background work keeps its current setting, Local, fall back to cloud.",
    ]);
    // Never a primary: the trap is the thing to see.
    expect(primaryOf(roles)).toBeNull();
    // A job already at work keeps its model: nothing is offered over it.
    const working = steps(
      input({
        config: cfg({ chat_model: "a", chat_routing: "local", background_routing: "local" }),
        served: served("a"),
        status: routes({ effective: "local_only" }, { effective: "nothing" }),
      }),
    )[2];
    expect(working.state).toBe("attention");
    expect(working.secondary).toEqual([]);
  });

  it("step 3 says who uses what once it is done", () => {
    // One of PM's own downloads is named as PM's list names it, not by its hf.co tag.
    expect(steps(STATES.returning)[2].line).toBe(
      "Chat and background work both use Qwen2.5 7B Instruct.",
    );
    const split = input({
      config: cfg({
        chat_model: "a",
        background_model: "b",
        chat_routing: "local",
        background_routing: "local",
      }),
      served: served("a", "b"),
      status: routes({ effective: "local_only" }),
    });
    expect(steps(split)[2].line).toBe("Chat uses a; background work uses b.");
    const chatOnly = input({
      config: cfg({ chat_model: "a", chat_routing: "local" }),
      served: served("a"),
      status: routes({ effective: "local_only" }, { effective: "cloud", cloud_key: "present" }),
    });
    expect(steps(chatOnly)[2].line).toBe("Chat uses a; background work uses your cloud model.");
    const bgOnly = input({
      config: cfg({ background_model: "b", background_routing: "local" }),
      served: served("b"),
      status: routes({ effective: "nothing" }, { effective: "local_only" }),
    });
    expect(steps(bgOnly)[2].line).toBe("Background work uses b; chat has nothing to answer with.");
    // Two builds of one model would read as one name twice; then the ids say which is which.
    const twoBuilds = input({
      config: cfg({
        chat_model: QWEN_TAG,
        background_model: `hf.co/${QWEN}:Q8_0`,
        chat_routing: "local",
        background_routing: "local",
      }),
      served: served(QWEN_TAG, `hf.co/${QWEN}:Q8_0`),
      status: routes({ effective: "local_only" }),
    });
    expect(steps(twoBuilds)[2].line).toBe(
      `Chat uses ${QWEN_TAG}; background work uses hf.co/${QWEN}:Q8_0.`,
    );
  });

  it("step 4 asks for a test right after an assignment, and only then", () => {
    expect(steps(STATES.assigned)[3]).toMatchObject({
      state: "next",
      action: { kind: "test", role: "chat", label: "Send a test message" },
    });
    // After a restart nothing is remembered, and nothing nags. Named by model: a test of the other
    // job's different model is still a test since PM started.
    const restart = steps(STATES.returning)[3];
    expect(restart.state).toBe("optional");
    expect(restart.line).toBe(
      "No test of Qwen2.5 7B Instruct since PM started, or since your server changed.",
    );
    expect(restart.secondary).toEqual([
      { kind: "test", role: "chat", label: "Send a test message" },
    ]);
    expect(primaryOf(restart)).toBeNull();
  });

  it("step 4 waits on the reply without ever offering a second test", () => {
    const running = steps({ ...STATES.assigned, tests: { running: "chat" } })[3];
    expect(running).toMatchObject({ state: "checking", action: null, secondary: [] });
    expect(running.line).toBe(
      "Waiting for the reply. The first one includes loading the model, which can take a while.",
    );
  });

  it("step 4 reports the last test against the model it asked", () => {
    const result = (over: Partial<LocalTestResult>): LocalTestResult => ({
      model: QWEN_TAG,
      ok: true,
      reply: "ready",
      elapsed_ms: 2400,
      loaded_for_test: false,
      was_holding: [],
      message: null,
      ...over,
    });
    const passed = steps({
      ...STATES.returning,
      tests: { running: null, chat: { result: result({}), error: null } },
    })[3];
    expect(passed).toMatchObject({ state: "done", line: "The last test replied in 2.4 s." });
    const roomy = steps({
      ...STATES.returning,
      status: {
        ...routes({ effective: "local_only" }),
        served_window: 32768,
        served_window_proven: true,
        window_source: "slots",
      },
      tests: { running: null, chat: { result: result({}), error: null } },
    })[3];
    expect(roomy.line).toBe(
      "The last test replied in 2.4 s. Your server gives it room for 32,768 tokens at a time.",
    );
    const failed = steps({
      ...STATES.returning,
      tests: { running: null, chat: { result: result({ ok: false }), error: null } },
    })[3];
    expect(failed.state).toBe("attention");
    expect(failed.line).toBe(
      `The last test didn't get a usable reply — what happened is under ${sectionLabel("sec-localai-roles")}.`,
    );
    expect(failed.secondary).toEqual([{ kind: "test", role: "chat", label: "Try again" }]);
    // A result about another model says nothing about this one.
    const stale = steps({
      ...STATES.returning,
      tests: { running: null, chat: { result: result({ model: "old" }), error: null } },
    })[3];
    expect(stale.state).toBe("optional");
    // A test asks a model, not a job: background work's Test it of the model chat uses too answers
    // for chat — on its own row, or as the one test the backend remembers across a tab switch.
    const viaBackground = steps({
      ...STATES.returning,
      tests: { running: null, background: { result: result({}), error: null } },
    })[3];
    expect(viaBackground).toMatchObject({ state: "done", line: "The last test replied in 2.4 s." });
    const afterAssign = steps({
      ...STATES.assigned,
      tests: { running: null, background: { result: result({}), error: null } },
    })[3];
    expect(afterAssign.state).toBe("done");
    // Of a different model, it doesn't.
    const otherModel = steps({
      ...STATES.returning,
      tests: { running: null, background: { result: result({ model: "old" }), error: null } },
    })[3];
    expect(otherModel.state).toBe("optional");
  });

  it("step 4 offers a test on battery, without making it the step", () => {
    // Both jobs are on Local, fall back to cloud, and On battery has moved them: step 3 is done,
    // so "After step 3." under it would be false — and a test still asks the local model.
    const battery = both(QWEN_TAG, "cloud_for_power", "local-then-cloud");
    for (const i of [battery, { ...battery, justAssigned: true }]) {
      const list = steps(i);
      expect(list[2].state).toBe("done");
      expect(list[3]).toMatchObject({
        state: "optional",
        line: "Chat and background work are on your cloud model while the battery is low, but a test still asks the local model, and loads it to answer.",
        secondary: [{ kind: "test", role: "chat", label: "Send a test message" }],
      });
      expect(primaryOf(list[3])).toBeNull();
    }
  });

  it("step 4 points at the tuning fold for a small window the server really serves", () => {
    const small = steps(STATES.smallWindow)[3];
    expect(small.state).toBe("attention");
    expect(small.line).toBe(
      "Your server gives the model room for 4,096 tokens at a time, so PM sends background work in smaller pieces. Giving it more room makes that work better.",
    );
    expect(small.action).toEqual({ kind: "locate", to: "tuning", label: "How to raise it" });
    // PM's own floor is not a reading of the server, and neither is the model's trained limit.
    const floor = {
      ...STATES.smallWindow,
      status: { ...STATES.smallWindow.status!, served_window_proven: false },
    };
    expect(steps(floor)[3].state).toBe("optional");
    // A server PM can't name has no tuning fold to open.
    const unnamed = {
      ...STATES.smallWindow,
      config: cfg({ ...STATES.smallWindow.config, base_url: "http://127.0.0.1:9000" }),
    };
    expect(steps(unnamed)[3].action).toMatchObject({ to: "sec-localai-endpoint" });
  });

  it("step 4 words a small window for the job that meets it", () => {
    // Only chat runs locally: the window is chat's model's, and nothing of background work's is cut
    // into pieces for it — what it limits is how long a chat can be.
    const chatOnly = (effective: "local_only" | "local_then_cloud") =>
      steps(
        input({
          config: cfg({
            chat_model: QWEN_TAG,
            chat_routing: effective === "local_only" ? "local" : "local-then-cloud",
          }),
          served: served(QWEN_TAG),
          status: {
            ...routes(
              { effective, cloud_key: "present" },
              { effective: "cloud", cloud_key: "present" },
            ),
            served_window: 4096,
            served_window_proven: true,
            window_source: "slots",
          },
        }),
      )[3];
    const alone = chatOnly("local_only");
    expect(alone.state).toBe("attention");
    expect(alone.line).toBe(
      "Your server gives the model room for 4,096 tokens at a time, so a long chat can be too much for it to answer. Giving it more room fixes that.",
    );
    expect(chatOnly("local_then_cloud").line).toBe(
      "Your server gives the model room for 4,096 tokens at a time, so a long chat can be too much for it to answer, and goes to your cloud model instead. Giving it more room fixes that.",
    );
    expect(alone.line).not.toContain("background work");
  });

  it("step 4 waits on step 3 while no job runs locally", () => {
    expect(steps(STATES.servedUnassigned)[3]).toMatchObject({
      state: "waiting",
      line: "After step 3.",
    });
  });
});

describe("assignPlan — one model on both jobs, the routing each needs", () => {
  const power = (chat: string, background: string) => ({
    chat: { cloud_key: chat },
    background: { cloud_key: background },
  });
  const prefix =
    "One model for both jobs — chat and background work — so your server only ever holds one. ";

  it("falls back to the cloud only for a job that has a key to fall back on", () => {
    const keyed = assignPlan(cfg(), power("present", "present"), "m", true);
    expect(keyed.routing).toEqual({ chat: "local-then-cloud", background: "local-then-cloud" });
    // Said right before the write, so it says every way a request still leaves for the cloud
    // (llm_gateway.rs): the server not answering (or resting, or at an address PM won't send to); a
    // prompt over the window it is proven to serve, while it answers perfectly well; a reply that
    // fails or times out; and On battery's move, once allowed. Never "only if your server stops".
    expect(keyed.sentence).toBe(
      `${prefix}PM sets both to Local, fall back to cloud: your cloud model answers whenever PM can't use your server — it isn't answering, a request is too long for the room your server gives the model, or a reply fails or times out — and when your battery runs low, if you've allowed that under ${sectionLabel("sec-localai-power")}. Choose Local only under ${sectionLabel("sec-localai-roles")} to keep everything on this computer.`,
    );
    expect(keyed.sentence).not.toMatch(/only if/);
    expect(assignPlan(cfg(), power("present", "present"), "m", false).sentence).toContain(
      "to keep everything on your server.",
    );
    const keyless = assignPlan(cfg(), power("absent", "absent"), "m", true);
    expect(keyless.routing).toEqual({ chat: "local", background: "local" });
    expect(keyless.sentence).toBe(
      `${prefix}PM sets both to Local only — there's no cloud key to fall back to.`,
    );
  });

  it("never reads 'can't read the keys' as 'no key'", () => {
    const unreadable = assignPlan(cfg(), power("unreadable", "unreadable"), "m", true);
    expect(unreadable.routing).toEqual({ chat: "local", background: "local" });
    expect(unreadable.sentence).toBe(
      `${prefix}PM can't read your saved keys right now, so it sets chat and background work to Local only.`,
    );
  });

  it("names a mixed outcome role by role", () => {
    expect(assignPlan(cfg(), power("present", "absent"), "m", true).sentence).toBe(
      `${prefix}PM sets chat to Local, fall back to cloud and background work to Local only.`,
    );
  });

  it("leaves a job already routed locally as it is, and says so", () => {
    const plan = assignPlan(cfg({ chat_routing: "local" }), power("present", "present"), "m", true);
    expect(plan.routing).toEqual({ chat: null, background: "local-then-cloud" });
    expect(plan.models).toEqual({ chat: "m", background: "m" });
    expect(plan.sentence).toBe(
      `${prefix}PM sets background work to Local, fall back to cloud. Chat keeps its current setting, Local only.`,
    );
  });
});

describe("overall — the chip beside the heading", () => {
  const chip = (i: ReadinessInput) => overall(steps(i), i)?.label ?? null;
  it("says nothing while PM is still reading", () => {
    expect(chip(input({ config: null }))).toBeNull();
    expect(chip(input({ status: null }))).toBeNull();
  });
  it("puts a problem first, then no server, then a step to do", () => {
    expect(chip(input({ status: status({ reachable: false }) }))).toBe("Needs attention");
    expect(chip(input({ config: cfg({ base_url: null }), detected: [] }))).toBe("Not set up");
    expect(chip(input({ served: [] }))).toBe("Setting up");
    expect(chip(both(QWEN_TAG, "local_only"))).toBe("Set up");
    expect(overall(steps(both(QWEN_TAG, "local_only")), both(QWEN_TAG, "local_only"))?.token).toBe(
      "--st-quick",
    );
  });
});

describe("rightNow — what a returning user came to see", () => {
  const res = (resident: LocalGpuResidency["resident"]): LocalGpuResidency => ({
    resident,
    vram_gb: 7.96,
    dgpu_displays: [],
    policy: "server",
    idle_minutes: 5,
    no_unload_route: false,
  });
  it("says what the server holds, and whether PM may hand it back", () => {
    const base = both(QWEN_TAG, "local_only");
    expect(rightNow({ ...base, residency: undefined }).holding).toBeNull();
    expect(rightNow({ ...base, residency: null }).holding).toBe(
      "Right now PM can't see what your server has loaded.",
    );
    expect(rightNow({ ...base, residency: res([]) }).holding).toBe(
      "Right now nothing is loaded. The next request loads the model, which takes a few seconds.",
    );
    const held = rightNow({
      ...base,
      residency: res([{ model: "q", size_gb: 7, size_vram_gb: 6.5, pm_loaded: true }]),
    });
    expect(held.holding).toBe(
      "Right now your server is holding q — at least 6.5 GB of your 8.0 GB card.",
    );
    expect(held.pmLoaded).toBe(true);
    // No separate card: the memory is the processor's too, and "card" would name hardware that isn't there.
    const mac = rightNow({
      ...base,
      recs: recs({ hardware: { ...recs().hardware, unified_memory: true } }),
      residency: res([{ model: "q", size_gb: 7, size_vram_gb: 6.5, pm_loaded: false }]),
    });
    expect(mac.holding).toBe("Right now your server is holding q in memory.");
    expect(mac.pmLoaded).toBe(false);
  });
  it("summarises On battery, and stays quiet where there is nothing to say", () => {
    const base = both(QWEN_TAG, "local_only");
    expect(
      rightNow({
        ...base,
        status: {
          ...base.status!,
          power: { ...base.status!.power, has_battery: false, source: "ac" },
        },
      }).battery,
    ).toBe("No battery on this computer, so On battery never applies.");
    expect(rightNow(input({ config: cfg({ base_url: null }) })).battery).toBeNull();
  });
});

describe("the card's copy keeps the tab's rules", () => {
  // The lists are readiness.ts's own (`COPY_COLLISIONS`, `SAME_SECTION_POINTERS`), so the pick
  // card's sweep in pickWords.test.ts and the rendered card's in LocalAiStart.test.tsx hold the same
  // rules as this one.

  /** Every string the card can show across a broad spread of states. */
  function everything(): string[] {
    const out: string[] = [];
    const cases: ReadinessInput[] = [
      input({ config: null }),
      input({ config: cfg({ base_url: null }), detected: [] }),
      input({ config: cfg({ base_url: null }), detected: [OLLAMA, LMS, LLAMA] }),
      input({
        config: cfg({ base_url: null }),
        detected: [],
        recs: recs({ disk_sources_present: ["ollama"] }),
      }),
      input({ config: cfg({ base_url: null }), detecting: true }),
      input({ status: null }),
      input({ status: status({ reachable: false }) }),
      input({
        status: { ...routes({ effective: "local_only" }), in_cooldown: true, reachable: false },
      }),
      input({ servedLoaded: false }),
      input({ served: [] }),
      input({ served: [{ id: "e", embedding: true }] }),
      input({ served: [], config: cfg({ base_url: "http://127.0.0.1:1234" }) }),
      input({ served: [], config: cfg({ base_url: "http://127.0.0.1:8080" }) }),
      input({ served: [], config: cfg({ base_url: "http://10.0.0.2:9000" }) }),
      input({ pull: { tag: QWEN_TAG, pct: 4 } }),
      input({ pull: { tag: "hf.co/x:Q4", pct: 4 }, served: served("a") }),
      input({
        served: served(QWEN_TAG),
        lastPulledTag: QWEN_TAG,
        status: routes({ effective: "cloud", cloud_key: "present" }),
      }),
      input({
        served: served(QWEN_TAG),
        lastPulledTag: QWEN_TAG,
        status: routes({ effective: "cloud", cloud_key: "unreadable" }),
      }),
      { ...both(QWEN_TAG, "local_only"), justAssigned: true },
      { ...both(QWEN_TAG, "local_only"), tests: { running: "chat" } },
      {
        ...both(QWEN_TAG, "local_then_cloud", "local-then-cloud"),
        status: {
          ...routes({ effective: "local_then_cloud" }),
          served_window: 2048,
          served_window_proven: true,
          window_source: "slots",
        },
      },
      input({
        config: cfg({ chat_routing: "local" }),
        status: routes({ effective: "nothing" }),
        served: served("a"),
      }),
      input({
        config: cfg({ chat_model: "x", chat_routing: "local" }),
        status: routes({ effective: "local_only" }),
        served: served("a"),
      }),
      input({
        config: cfg({
          chat_model: "a",
          background_model: "b",
          chat_routing: "local",
          background_routing: "local",
        }),
        served: served("a", "b"),
        status: routes({ effective: "local_only" }),
        recs: recs({
          co_residency: {
            ram: "exceeds",
            vram: null,
            combined_gb: 20,
            ram_budget_gb: 14,
            vram_budget_gb: null,
          },
        }),
      }),
      input({ recs: recs({ pick: undefined }), served: [] }),
      input({
        recs: recs({
          pick: { kind: "nothing", reason: "nothing_on_gpu", basis: "gpu", system_fallback: true },
        }),
        served: [],
      }),
      input({ recs: null, recsLoading: false }),
      input({ config: null, configError: "the keychain is locked" }),
      input({
        config: cfg({ base_url: null, chat_routing: "local" }),
        detected: [OLLAMA],
        status: routes({ effective: "nothing", cloud_key: "present" }),
      }),
      input({
        config: cfg({ base_url: null }),
        detected: [],
        recs: recs({ disk_sources_present: ["lm_studio"] }),
      }),
      input({
        config: cfg({ chat_routing: "local-then-cloud", background_routing: "local-then-cloud" }),
        served: served(QWEN_TAG),
        status: routes({ effective: "cloud", cloud_key: "present" }),
      }),
      both(QWEN_TAG, "cloud_for_power", "local-then-cloud"),
      { ...both("my-finetune", "local_only"), recs: recs({ pick: ownedPick() }) },
      { ...both("gone:7b", "local_only"), served: [], recs: recs({ pick: ownedPick() }) },
      {
        ...both("gone:7b", "local_only"),
        served: [],
        recs: recs({ pick: ownedPick({ source: "ollama" }) }),
      },
      {
        ...both("gone:7b", "local_only"),
        served: [],
        recs: recs({ pick: ownedPick({ source: "lm_studio" }) }),
      },
      input({
        config: cfg({ chat_model: QWEN_TAG, chat_routing: "local" }),
        served: served(QWEN_TAG),
        status: {
          ...routes({ effective: "local_only" }, { effective: "cloud", cloud_key: "present" }),
          served_window: 4096,
          served_window_proven: true,
          window_source: "slots",
        },
      }),
    ];
    for (const i of cases) {
      out.push(standing(i));
      for (const s of steps(i)) {
        out.push(s.title, ...(s.line ? [s.line] : []), ...s.notes);
        for (const a of [...(s.action ? [s.action] : []), ...s.secondary]) {
          out.push(a.label);
          if (a.kind === "assign") out.push(a.plan.sentence);
        }
      }
      const now = rightNow({
        ...i,
        residency: {
          resident: [{ model: "q", size_gb: 1, size_vram_gb: 1, pm_loaded: true }],
          vram_gb: 8,
          dgpu_displays: [],
          policy: "server",
          idle_minutes: 5,
          no_unload_route: false,
        },
      });
      out.push(...[now.holding, now.battery].filter((t): t is string => !!t));
      out.push(
        rightNow({ ...i, residency: null }).holding ?? "",
        rightNow({
          ...i,
          residency: {
            resident: [],
            vram_gb: 8,
            dgpu_displays: [],
            policy: "server",
            idle_minutes: 5,
            no_unload_route: false,
          },
        }).holding ?? "",
      );
    }
    return out.filter(Boolean);
  }

  it("repeats no phrase a section owns", () => {
    const strings = everything();
    expect(strings.length).toBeGreaterThan(80);
    for (const s of strings) for (const re of COPY_COLLISIONS) expect(s, `${re}`).not.toMatch(re);
  });

  it("points nowhere by direction, except inside this section", () => {
    for (const s of everything()) {
      const stripped = SAME_SECTION_POINTERS.reduce((t, re) => t.replace(re, ""), s);
      expect(stripped, s).not.toMatch(/\b(above|below)\b/i);
    }
  });

  it("gives its buttons only the names it means to — never a section control's", () => {
    const labels = new Set<string>();
    for (const i of [
      input({ config: cfg({ base_url: null }), detected: [OLLAMA, LMS] }),
      input({ config: cfg({ base_url: null }), detected: [] }),
      input({ served: [] }),
      input({ served: served(QWEN_TAG), lastPulledTag: QWEN_TAG }),
      { ...both(QWEN_TAG, "local_only"), justAssigned: true },
      input({ status: status({ reachable: false }) }),
    ]) {
      for (const s of steps(i))
        for (const a of [...(s.action ? [s.action] : []), ...s.secondary]) labels.add(a.label);
    }
    const allowed = [
      /^Connect to (Ollama|LM Studio|llama-server)( instead)?$/,
      /^Download \(.+\)$/,
      /^Use .+ for both( jobs)?$/,
      /^Send a test message$/,
      /^Try again$/,
      /^Look now$/,
      /^What to check$/,
      /^Go to All models$/,
      /^Go to Assign roles$/,
      /^How to raise it$/,
    ];
    for (const l of labels)
      expect(
        allowed.some((re) => re.test(l)),
        l,
      ).toBe(true);
    // The section controls' own names.
    for (const taken of [
      "Connect",
      "Download",
      "Test it",
      "Release now",
      "Auto-detect a local server",
      "Check",
    ])
      expect(labels.has(taken), taken).toBe(false);
  });
});
