// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The Local AI tab (#296/#297). At 1,200+ lines it was the largest untested surface in the epic,
// and it is where two of the close-out defects hid: a stored endpoint token with no way to remove
// it, and embedding models offered as assignable chat models.
//
// These tests cover the branches where a WRONG state misleads the user about what PM has or will
// do — a token that is still in the keychain after you thought you removed it, a model PM will let
// you assign but cannot answer with. Not render-coverage for its own sake.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  LocalLlmConfig,
  LocalLlmStatus,
  LocalRecommendation,
  LocalRecommendations,
  LocalCoResidency,
  LocalServedModel,
  LocalTestResult,
} from "../../lib/types";

const activeLocalPull = vi.fn();
const cancelLocalPull = vi.fn();
const checkLocalLlmEndpoint = vi.fn();
const clearLocalLlmEndpoint = vi.fn();
const clearLocalLlmToken = vi.fn();
const dismissLocalBetterFit = vi.fn();
const localGpuResidency = vi.fn();
const releaseLocalGpu = vi.fn();
const getLocalReleasePolicy = vi.fn();
const setLocalReleasePolicy = vi.fn();
const getTrayEnabled = vi.fn();
const setTrayEnabled = vi.fn();
const getLocalLlmConfig = vi.fn();
const listLocalLlmModels = vi.fn();
const localBetterFitNotice = vi.fn();
const localHardwareScan = vi.fn();
const localLlmStatus = vi.fn();
const localModelRecommendations = vi.fn();
const testLocalLlm = vi.fn();
const activeLocalTest = vi.fn();
const probeLocalLlmPorts = vi.fn();
const pullLocalModel = vi.fn();
const acceptLocalModelTerms = vi.fn();
const setLocalLlmEndpoint = vi.fn();
const setLocalLlmRoleModel = vi.fn();
const setLocalLlmRouting = vi.fn();
const setLocalLlmToken = vi.fn();
const setLocalModelRescanCadence = vi.fn();
const setLocalModelScanDir = vi.fn();
const setLocalPowerPolicy = vi.fn();
const keepLocalOnBattery = vi.fn();
const localAiSettingsAreDefault = vi.fn();
const resetLocalAiSettings = vi.fn();

// A factory REPLACES the whole module, so every function the component imports must appear here or
// it is `undefined` at module-eval. The tab and its sections import every one of these.
vi.mock("../../lib/ipc", () => ({
  activeLocalPull: () => activeLocalPull(),
  cancelLocalPull: () => cancelLocalPull(),
  checkLocalLlmEndpoint: (...a: unknown[]) => checkLocalLlmEndpoint(...a),
  clearLocalLlmEndpoint: () => clearLocalLlmEndpoint(),
  clearLocalLlmToken: () => clearLocalLlmToken(),
  dismissLocalBetterFit: () => dismissLocalBetterFit(),
  // The lifecycle section's four, plus the tray row it shares with General. This factory replaces
  // the WHOLE module, so anything the tab imports and this omits is `undefined` at module eval and
  // takes every test in the file down with an unhelpful "is not a function".
  localGpuResidency: () => localGpuResidency(),
  releaseLocalGpu: () => releaseLocalGpu(),
  getLocalReleasePolicy: () => getLocalReleasePolicy(),
  setLocalReleasePolicy: (...a: unknown[]) => setLocalReleasePolicy(...a),
  getTrayEnabled: () => getTrayEnabled(),
  setTrayEnabled: (...a: unknown[]) => setTrayEnabled(...a),
  getLocalLlmConfig: () => getLocalLlmConfig(),
  listLocalLlmModels: () => listLocalLlmModels(),
  localBetterFitNotice: () => localBetterFitNotice(),
  localHardwareScan: (...a: unknown[]) => localHardwareScan(...a),
  localLlmStatus: () => localLlmStatus(),
  localModelRecommendations: () => localModelRecommendations(),
  testLocalLlm: (...a: unknown[]) => testLocalLlm(...a),
  activeLocalTest: () => activeLocalTest(),
  // The push signal the tab now also listens on, so the "answering right now" hint beside each role
  // is not a coin flip on a 30 s tick. A no-op subscription here — the tests drive `localLlmStatus`
  // directly — but it must EXIST, or the factory hands the component `undefined` and every test in
  // the file dies at module eval.
  onLocalLlmStatus: () => Promise.resolve(() => {}),
  probeLocalLlmPorts: () => probeLocalLlmPorts(),
  pullLocalModel: (...a: unknown[]) => pullLocalModel(...a),
  acceptLocalModelTerms: (...a: unknown[]) => acceptLocalModelTerms(...a),
  setLocalLlmEndpoint: (...a: unknown[]) => setLocalLlmEndpoint(...a),
  setLocalLlmRoleModel: (...a: unknown[]) => setLocalLlmRoleModel(...a),
  setLocalLlmRouting: (...a: unknown[]) => setLocalLlmRouting(...a),
  setLocalLlmToken: (...a: unknown[]) => setLocalLlmToken(...a),
  setLocalModelRescanCadence: (...a: unknown[]) => setLocalModelRescanCadence(...a),
  setLocalModelScanDir: (...a: unknown[]) => setLocalModelScanDir(...a),
  // The On battery section's two (#432) — and its consent ask, which imports the same pair.
  setLocalPowerPolicy: (...a: unknown[]) => setLocalPowerPolicy(...a),
  keepLocalOnBattery: (...a: unknown[]) => keepLocalOnBattery(...a),
  // The tab's "Reset to defaults" footer (#445).
  localAiSettingsAreDefault: () => localAiSettingsAreDefault(),
  resetLocalAiSettings: () => resetLocalAiSettings(),
}));

// The folder picker is only reached by a click no test here makes, but the module is imported at
// eval time, so it is stubbed rather than left to touch a Tauri plugin under jsdom.
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

// `useTheme` is stubbed so the tab's <Button>/<Select> primitives don't need the full ThemeProvider
// (which pulls in IPC).
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { CHANGELOG } from "../../lib/changelog";
import { SETTING_SAVED_EVENT } from "../../lib/settingsSaved";
import { LocalAiSettings } from "./LocalAiSettings";
import { sectionLabel } from "./sections";

afterEach(cleanup);

const cfg = (over: Partial<LocalLlmConfig> = {}): LocalLlmConfig => ({
  base_url: "http://127.0.0.1:11434",
  chat_model: "llama3.2:1b",
  background_model: "",
  chat_routing: "local",
  background_routing: "cloud",
  has_token: false,
  ...over,
});

const recs = (): LocalRecommendations => ({
  hardware: {
    platform: "windows",
    total_ram_gb: 16,
    available_ram_gb: 9,
    cpu_brand: "Test CPU",
    cpu_cores: 8,
    cpu_threads: 16,
    disk_free_gb: 200,
    gpu_name: null,
    gpu_vendor: null,
    vram_gb: null,
    vram_source: null,
    gpu_bandwidth_gbps: null,
    unified_memory: false,
    is_wsl: false,
    notes: [],
  },
  reserve_gb: 2,
  gpu_reserve_gb: 1,
  chat_speed: { floor_tps: 30, reply_tokens: 300, reply_secs: 10 },
  catalog_version: 1,
  catalog_generated_at: "2026-07-22",
  endpoint_configured: true,
  cadence: "on-catalog-update",
  rescan_due: false,
  curated: [],
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
  live_available_ram_gb: 9,
});

const served = (...models: LocalServedModel[]) => models;

// TYPED, unlike the inline object it replaces: the untyped mock silently drifted a release behind
// the Rust struct (four fields short), which is exactly the drift a fixture exists to catch.
const statusFix = (over: Partial<LocalLlmStatus> = {}): LocalLlmStatus => ({
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
  power: INERT_POWER_VIEW,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  getLocalLlmConfig.mockResolvedValue(cfg());
  listLocalLlmModels.mockResolvedValue(served({ id: "llama3.2:1b", embedding: false }));
  localModelRecommendations.mockResolvedValue(recs());
  localBetterFitNotice.mockResolvedValue(null);
  localLlmStatus.mockResolvedValue(statusFix());
  localGpuResidency.mockResolvedValue({
    resident: [],
    vram_gb: 8,
    dgpu_displays: [],
    policy: "server",
    idle_minutes: 5,
    no_unload_route: false,
  });
  releaseLocalGpu.mockResolvedValue(0);
  getLocalReleasePolicy.mockResolvedValue({
    policy: "server",
    idle_minutes: 5,
    battery_idle_minutes: 0,
  });
  setLocalReleasePolicy.mockResolvedValue(undefined);
  setLocalPowerPolicy.mockResolvedValue(undefined);
  keepLocalOnBattery.mockResolvedValue(undefined);
  localAiSettingsAreDefault.mockResolvedValue(false);
  resetLocalAiSettings.mockResolvedValue(undefined);
  getTrayEnabled.mockResolvedValue(false);
  setTrayEnabled.mockResolvedValue(undefined);
  activeLocalPull.mockResolvedValue(null);
  activeLocalTest.mockResolvedValue(null);
  cancelLocalPull.mockResolvedValue(true);
  clearLocalLlmToken.mockResolvedValue(undefined);
  // Nothing found on this computer, unless a test says otherwise.
  probeLocalLlmPorts.mockResolvedValue([]);
});

/** Render and wait for the initial config load to settle. */
async function loaded() {
  const view = render(<LocalAiSettings />);
  await waitFor(() => expect(getLocalLlmConfig).toHaveBeenCalled());
  await screen.findByText(/Connected to/);
  return view;
}

describe("the saved endpoint token", () => {
  it("says a token is stored, and offers a way to remove just that", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ has_token: true }));
    await loaded();

    expect(screen.getByText(/with a saved token/)).toBeTruthy();
    expect(screen.getByRole("button", { name: /forget token/i })).toBeTruthy();
  });

  it("offers nothing to forget when no token is stored", async () => {
    await loaded();

    expect(screen.queryByText(/with a saved token/)).toBeNull();
    expect(screen.queryByRole("button", { name: /forget token/i })).toBeNull();
  });

  it("confirms before forgetting it, and does nothing if you back out", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ has_token: true }));
    await loaded();

    fireEvent.click(screen.getByRole("button", { name: /forget token/i }));
    // Credential material: a single click must not destroy it.
    expect(clearLocalLlmToken).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(clearLocalLlmToken).not.toHaveBeenCalled();
  });

  it("forgets the token and re-reads the config, so the readout cannot go stale", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ has_token: true }));
    await loaded();
    const loadsBefore = getLocalLlmConfig.mock.calls.length;

    fireEvent.click(screen.getByRole("button", { name: /forget token/i }));
    fireEvent.click(screen.getByRole("button", { name: /forget it/i }));

    await waitFor(() => expect(clearLocalLlmToken).toHaveBeenCalledTimes(1));
    // Nothing else refreshes `config`; without this reload the "(with a saved token)" line would
    // stay on screen after the token was gone.
    await waitFor(() => expect(getLocalLlmConfig.mock.calls.length).toBeGreaterThan(loadsBefore));
    // Forgetting the token must NOT take the endpoint or the role assignments with it — that is
    // what Disconnect does, and the whole point of a separate control.
    expect(clearLocalLlmEndpoint).not.toHaveBeenCalled();
  });
});

describe("assigning a model to a role", () => {
  it("offers a served chat model", async () => {
    await loaded();

    const option = screen.getAllByRole("option", { name: "llama3.2:1b" })[0] as HTMLOptionElement;
    expect(option.disabled).toBe(false);
  });

  it("shows an embedding model but will not let you assign it", async () => {
    // Ollama and LM Studio serve embedders from the same endpoint as chat models. Shown-and-
    // disabled rather than hidden: a model visible in Ollama but absent from PM reads as a PM bug.
    listLocalLlmModels.mockResolvedValue(
      served(
        { id: "llama3.2:1b", embedding: false },
        { id: "nomic-embed-text:latest", embedding: true },
      ),
    );
    await loaded();

    const embedder = screen.getAllByRole("option", {
      name: /nomic-embed-text/,
    })[0] as HTMLOptionElement;
    expect(embedder.disabled).toBe(true);
    // The reason travels with the option, not just as a colour.
    expect(embedder.textContent).toContain("embedding model");
  });

  it("explains the gate in unfolded copy, only when there is something gated", async () => {
    await loaded();
    expect(screen.queryByText(/can't be chosen/i)).toBeNull();

    cleanup();
    listLocalLlmModels.mockResolvedValue(
      served(
        { id: "llama3.2:1b", embedding: false },
        { id: "nomic-embed-text:latest", embedding: true },
      ),
    );
    await loaded();
    // The settings doctrine folds prose but never gating hints — "listed but unpickable" is one.
    expect(screen.getByText(/can't be chosen/i)).toBeTruthy();
  });

  describe("two different models on one server", () => {
    // This used to be one unconditional paragraph fired at every pair of distinct local models,
    // regardless of whether they collided — noise on a machine with room to spare, which is how you
    // train someone to ignore the one line that matters. The sum is now done in Rust and the four
    // outcomes read differently. The "there is no question to ask" cases (a role on cloud, the same
    // model twice, a model the endpoint isn't serving) are pinned in local_ai.rs, where the decision
    // now lives — `co_residency` simply arrives null.
    const co = (over: Partial<LocalCoResidency>): LocalCoResidency => ({
      ram: "fits",
      vram: null,
      combined_gb: 12,
      ram_budget_gb: 14,
      vram_budget_gb: null,
      ...over,
    });
    const withCo = async (c: LocalCoResidency | null) => {
      localModelRecommendations.mockResolvedValue({ ...recs(), co_residency: c });
      await loaded();
    };

    it("says nothing at all when both models fit", async () => {
      // Absence is the pass, the same call the served-window line makes.
      await withCo(co({ ram: "fits" }));
      expect(screen.queryByText(/won't both stay loaded/i)).toBeNull();
      expect(screen.queryByText(/can't call it/i)).toBeNull();
    });

    it("describes swapping, not breaking, when they don't both fit", async () => {
      // Ollama's FAQ: it queues and unloads an idle model to make room. So the honest warning is
      // about seconds lost per switch, and the words for a failure must not appear.
      await withCo(co({ ram: "exceeds", combined_gb: 18, ram_budget_gb: 14 }));
      const line = await screen.findByText(/won't both stay loaded/i);
      expect(line).toBeTruthy();
      expect(line.textContent).toMatch(/Nothing breaks/i);
      expect(line.textContent).not.toMatch(/\b(fail|crash|error|out of memory)\b/i);
    });

    it("refuses to call a pair that lands inside its own margin of error", async () => {
      // The memory estimate ran between +1.6% and +11.4% against real loads, and is only claimed to
      // +-15%. Saying "these will not both stay loaded" from inside that band would be a confident
      // wrong warning about a setup that works — worse than the vague prose this replaced.
      await withCo(co({ ram: "too_close", combined_gb: 15, ram_budget_gb: 14 }));
      expect(await screen.findByText(/can't call it/i)).toBeTruthy();
      expect(screen.queryByText(/won't both stay loaded/i)).toBeNull();
    });

    it("says which memory it means, and prefers the graphics card when there is one", async () => {
      // "it takes up all of their gpu twice" is the case people mean. The card is the tighter
      // constraint whenever there is one, so it decides the verdict even when system RAM is fine.
      await withCo(
        co({ ram: "fits", vram: "exceeds", combined_gb: 10, vram_budget_gb: 7, ram_budget_gb: 30 }),
      );
      const line = await screen.findByText(/won't both stay loaded/i);
      expect(line.textContent).toMatch(/on your graphics card/i);
    });

    it("admits when it couldn't size one of them", async () => {
      await withCo(co({ ram: "unknown", combined_gb: null }));
      expect(await screen.findByText(/couldn't size one of them/i)).toBeTruthy();
    });
  });

  it("keeps a saved model selectable even when the endpoint stops serving it", async () => {
    // Otherwise the picker silently drops the user's own choice back to "use cloud".
    listLocalLlmModels.mockResolvedValue(served({ id: "some-other-model", embedding: false }));
    await loaded();

    expect(screen.getAllByRole("option", { name: "llama3.2:1b" })[0]).toBeTruthy();
  });
});

describe("the endpoint form", () => {
  it("hides the connect form once an endpoint is saved", async () => {
    // Documents today's behaviour rather than blessing it: the token field lives in this branch, so
    // a live endpoint has no way to CHANGE its token — only Forget token and reconnect. Worth a
    // failing test the day that is fixed.
    await loaded();

    expect(screen.queryByPlaceholderText(/bearer token/i)).toBeNull();
    expect(screen.queryByRole("button", { name: /^connect$/i })).toBeNull();
  });

  it("shows the connect form when nothing is configured", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    render(<LocalAiSettings />);
    await waitFor(() => expect(getLocalLlmConfig).toHaveBeenCalled());

    expect(await screen.findByRole("button", { name: /auto-detect/i })).toBeTruthy();
    // Nothing is configured, so there is nothing to list — the model pickers must not be shown.
    expect(listLocalLlmModels).not.toHaveBeenCalled();
  });

  it("names both fields by their visible labels, not by their placeholders", async () => {
    // Both labels sat above their Input with no `htmlFor`, so the placeholder was the last-resort
    // accessible name — and a placeholder stops being the name the moment you type. The queries
    // below are the accname computation, which is the only thing that settles this.
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    render(<LocalAiSettings />);

    expect(await screen.findByLabelText("Endpoint URL")).toBeTruthy();
    expect(screen.getByLabelText(/^Token/)).toBeTruthy();
    expect(screen.queryByLabelText("http://localhost:11434")).toBeNull();
  });
});

describe("a server that answers with nothing in it", () => {
  // #790. A freshly installed runner has no models by definition, and until the endpoint check
  // learned to accept that state the Workbench never had to say anything about it — the server
  // simply failed to connect. Now it connects, so every readout that speaks for it has to be true.

  it("says why both role pickers are empty, rather than leaving them bare", async () => {
    listLocalLlmModels.mockResolvedValue([]);
    await loaded();

    expect(await screen.findByText(/isn't serving any models yet/i)).toBeTruthy();
  });

  it("says nothing about what a server serves until the listing has answered", async () => {
    // `served` starts empty, so an unguarded check would flash "this server has no models" at
    // every user with a working endpoint before the listing resolves.
    listLocalLlmModels.mockReturnValue(new Promise(() => {}));
    await loaded();

    expect(screen.queryByText(/isn't serving any models yet/i)).toBeNull();
  });

  it("never claims a server serves nothing when PM could not ask", async () => {
    // A failed listing leaves `served` empty too. "We could not reach it" and "it has nothing"
    // are different facts and only one of them is knowable here.
    listLocalLlmModels.mockRejectedValue(new Error("connection refused"));
    await loaded();

    expect(screen.queryByText(/isn't serving any models yet/i)).toBeNull();
  });

  it("leaves a server that does serve models alone", async () => {
    await loaded();

    expect(screen.queryByText(/isn't serving any models yet/i)).toBeNull();
  });

  it("does not report an empty endpoint as a clean pass", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    checkLocalLlmEndpoint.mockResolvedValue({
      reachable: true,
      normalized_url: "http://127.0.0.1:11434",
      models: [],
      assignable: [],
      posture: "loopback",
      scheme_verdict: "ok",
      exposed_on_network: false,
      message: null,
    });
    render(<LocalAiSettings />);

    fireEvent.change(await screen.findByLabelText("Endpoint URL"), {
      target: { value: "http://127.0.0.1:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^check$/i }));

    // Reachable and serving nothing is a true readout, but on its own it reads as "you are set"
    // at the exact moment there is still a download to do.
    const heading = await screen.findByText(/Reachable . 0 model\(s\)/i);
    expect(heading.getAttribute("style")).toContain("--st-look");
    expect(screen.getByText(/no models in it yet/i)).toBeTruthy();
  });
});

describe("model licence terms", () => {
  // The catalogue ships models under bespoke publisher terms (Gemma, Llama, the largest Qwen 2.5)
  // alongside genuinely open ones. These pin the promise the UI makes about that difference.
  //
  // The tags below are the shape the generator really emits (`hf.co/<repo>:<QUANT>`) and name each
  // fixture's OWN repo and fitted quant — the invariant `pull_target_for` enforces in Rust. An
  // invented literal here would pass against the component while describing a download PM could
  // never perform, which is how this flow went three releases without a single reachable button.
  const model = (over: Partial<LocalRecommendation> = {}): LocalRecommendation => ({
    repo: "bartowski/gemma-2-2b-it-GGUF",
    display_name: "gemma 2 2b it",
    architecture: "gemma2",
    role_hint: "background",
    parameters_b: 2.61,
    active_parameters_b: 2.61,
    context_length: 8192,
    multimodal: false,
    reasoning: null,
    ollama_pull: "hf.co/bartowski/gemma-2-2b-it-GGUF:Q4_K_M",
    sharded_quant: false,
    gpu_pull: null,
    licence: {
      id: "gemma",
      name: "Gemma Terms of Use",
      url: "https://ai.google.dev/gemma/terms",
      open: false,
      summary: "Google's own terms, not an open-source licence.",
    },
    fit: {
      verdict: "comfortable",
      quant: "Q4_K_M",
      context: 8192,
      kv: "f16",
      est_memory_gb: 2.4,
      est_tokens_per_sec: 40,
      speed_basis: null,
      notes: [],
    },
    gpu: { kind: "single" },
    ...over,
  });

  const openModel = () =>
    model({
      repo: "bartowski/Phi-3.5-mini-instruct-GGUF",
      display_name: "Phi 3.5 mini instruct",
      ollama_pull: "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
      licence: {
        id: "mit",
        name: "MIT License",
        url: "https://opensource.org/license/mit",
        open: true,
        summary: "A permissive open-source licence.",
      },
    });

  async function withCurated(curated: LocalRecommendation[], accepted: string[] = []) {
    localModelRecommendations.mockResolvedValue({
      ...recs(),
      curated,
      terms_accepted: accepted,
    });
    return loaded();
  }

  it("labels every model with its licence, whatever the terms", async () => {
    await withCurated([model(), openModel()]);

    expect(await screen.findByText("Gemma Terms of Use")).toBeTruthy();
    expect(screen.getByText("MIT License")).toBeTruthy();
  });

  it("does NOT download a restricted model until the terms are accepted", async () => {
    // The whole point: the click must not reach the runner first and disclose second.
    await withCurated([model()]);

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));

    expect(await screen.findByText(/Google's own terms/)).toBeTruthy();
    expect(pullLocalModel).not.toHaveBeenCalled();
  });

  it("downloads once the terms are accepted, and records the acceptance", async () => {
    acceptLocalModelTerms.mockResolvedValue(["gemma"]);
    pullLocalModel.mockResolvedValue(undefined);
    await withCurated([model()]);

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));
    fireEvent.click(await screen.findByRole("button", { name: /accept and download/i }));

    await waitFor(() =>
      expect(pullLocalModel).toHaveBeenCalledWith(
        "hf.co/bartowski/gemma-2-2b-it-GGUF:Q4_K_M",
        expect.anything(),
      ),
    );
    expect(acceptLocalModelTerms).toHaveBeenCalledWith("gemma");
  });

  it("tells the reset footer about the acceptance, without waiting for the download", async () => {
    // A licence accepted is one of the settings a reset clears, and not a `set_*` command — and the
    // download it starts can run for an hour, so "re-read once it lands" is not soon enough.
    acceptLocalModelTerms.mockResolvedValue(["gemma"]);
    pullLocalModel.mockReturnValue(new Promise(() => {}));
    await withCurated([model()]);
    const before = localAiSettingsAreDefault.mock.calls.length;

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));
    fireEvent.click(await screen.findByRole("button", { name: /accept and download/i }));

    await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());
    expect(localAiSettingsAreDefault.mock.calls.length).toBeGreaterThan(before);
  });

  it("never downloads when recording the acceptance fails", async () => {
    // The safe direction: an acceptance that did not persist must not authorise the download, or
    // the record and the act disagree.
    acceptLocalModelTerms.mockRejectedValue(new Error("db is locked"));
    await withCurated([model()]);

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));
    fireEvent.click(await screen.findByRole("button", { name: /accept and download/i }));

    await waitFor(() => expect(acceptLocalModelTerms).toHaveBeenCalled());
    expect(pullLocalModel).not.toHaveBeenCalled();
  });

  it("does not ask again for a licence already accepted", async () => {
    pullLocalModel.mockResolvedValue(undefined);
    await withCurated([model()], ["gemma"]);

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));

    await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());
    expect(acceptLocalModelTerms).not.toHaveBeenCalled();
  });

  it("never interrupts an open-licence download", async () => {
    pullLocalModel.mockResolvedValue(undefined);
    await withCurated([openModel()]);

    fireEvent.click(await screen.findByRole("button", { name: /download/i }));

    await waitFor(() =>
      expect(pullLocalModel).toHaveBeenCalledWith(
        "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
        expect.anything(),
      ),
    );
    expect(screen.queryByRole("button", { name: /accept and download/i })).toBeNull();
  });
});

describe("a configured endpoint that isn't answering", () => {
  // The chip beside the section label said "Unreachable" or "Cooling down (42s)" and stopped. Two
  // different causes, two different things to do, and neither was said anywhere on the page.
  it("says what to check when PM can't reach the server", async () => {
    localLlmStatus.mockResolvedValue(statusFix({ reachable: false, in_cooldown: false }));
    await loaded();

    expect(await screen.findByText(/PM can't reach it at the moment/)).toBeTruthy();
    expect(screen.queryByText(/resting the connection/)).toBeNull();
  });

  it("explains a cooldown as PM backing off, not as something to fix", async () => {
    localLlmStatus.mockResolvedValue(
      statusFix({ reachable: false, in_cooldown: true, cooldown_remaining_s: 42 }),
    );
    await loaded();

    expect(await screen.findByText(/resting the connection/)).toBeTruthy();
    // The unreachable line would be wrong here: the server may be perfectly fine and PM is simply
    // not asking it yet.
    expect(screen.queryByText(/PM can't reach it at the moment/)).toBeNull();
  });

  it("says nothing extra while the endpoint is healthy", async () => {
    localLlmStatus.mockResolvedValue(statusFix({ reachable: true }));
    await loaded();

    expect(screen.queryByText(/PM can't reach it at the moment/)).toBeNull();
    expect(screen.queryByText(/resting the connection/)).toBeNull();
  });
});

describe("the served-window honesty line", () => {
  // The release's headline promise: a small served window is WARNED about, and an unproven number
  // is never presented as a measurement. Nothing pinned either half until now.
  it("attributes an unproven floor to PM rather than to the user's server", async () => {
    localLlmStatus.mockResolvedValue(
      statusFix({ served_window: 4096, served_window_proven: false, window_source: "default" }),
    );
    await loaded();

    // The SUBJECT is PM, not the user's server. An unproven floor is PM's own number, and the copy
    // used to say "Your server is serving 4,096 (PM's floor)" — putting PM's guess in the server's
    // mouth. It also must not read as an estimate: the two unproven rungs are wrong in opposite
    // directions, and this one is the under-estimate.
    expect(await screen.findByText(/PM is sizing its work for/)).toBeTruthy();
    expect(screen.queryByText(/Your server is serving/)).toBeNull();
  });

  it("says it hasn't measured yet instead of rendering nothing at all", async () => {
    // THE regression test for this card. The whole block was gated on `served_window != null`, so
    // on a fresh install — the one moment the warning exists for — PM showed nothing. The number is
    // only readable while a model is resident, and nothing loads one until the first local call, so
    // this was the permanent state of every new setup.
    localLlmStatus.mockResolvedValue(statusFix({ served_window: null }));
    await loaded();

    expect(await screen.findByText(/hasn't read your server's context window yet/)).toBeTruthy();
  });

  it("stays quiet about an unmeasured window when no role goes local", async () => {
    // There is no local model to measure, so the line would be answering a question nobody asked.
    getLocalLlmConfig.mockResolvedValue(
      cfg({ chat_routing: "cloud", background_routing: "cloud" }),
    );
    localLlmStatus.mockResolvedValue(statusFix({ served_window: null }));
    await loaded();

    expect(screen.queryByText(/hasn't read your server's context window yet/)).toBeNull();
  });

  it("never presents the model's trained capacity as the window being served", async () => {
    // #792 in miniature, shown to the user. `served_window` reports the cached number RAW while the
    // gateway clamps an unproven one to its floor — so a 32,768 from `models_meta` rendered as a
    // comfortable window, suppressing the warning entirely, while PM was internally sizing to 4,096.
    localLlmStatus.mockResolvedValue(
      statusFix({
        served_window: 32768,
        served_window_proven: false,
        window_source: "models_meta",
      }),
    );
    await loaded();

    expect(await screen.findByText(/that is the model's own limit/)).toBeTruthy();
    expect(screen.queryByText(/Your server is serving/)).toBeNull();
  });

  it("drops the estimate suffix once the number is measured", async () => {
    localLlmStatus.mockResolvedValue(
      statusFix({ served_window: 4096, served_window_proven: true, window_source: "slots" }),
    );
    await loaded();

    expect(await screen.findByText(/Your server is serving/)).toBeTruthy();
    expect(screen.queryByText(/hasn't measured/)).toBeNull();
  });

  it("says nothing at a comfortable window — absence is the pass", async () => {
    localLlmStatus.mockResolvedValue(
      statusFix({ served_window: 32768, served_window_proven: true, window_source: "slots" }),
    );
    await loaded();

    expect(screen.queryByText(/Your server is serving/)).toBeNull();
  });
});

describe("a split card, where the same model runs two ways", () => {
  // The defect: `Recommendation` carried ONE pull target, resolved from the Highest-quality rung, so
  // the "Fastest on GPU" row — the only config that runs at GPU speed on an 8 GB card — had no
  // download at all. The card printed a caption admitting it. Connecting an Ollama endpoint then
  // deleted the copyable commands too, removing the last route to it.
  const split = (over: Partial<LocalRecommendation> = {}): LocalRecommendation => ({
    repo: "bartowski/Qwen2.5-7B-Instruct-GGUF",
    display_name: "Qwen2.5 7B Instruct",
    architecture: "qwen2",
    role_hint: "chat",
    parameters_b: 7.62,
    active_parameters_b: 7.62,
    context_length: 32768,
    multimodal: false,
    reasoning: null,
    ollama_pull: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q8_0",
    sharded_quant: false,
    gpu_pull: {
      tag: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M",
      sharded: false,
      same_file: false,
    },
    licence: {
      id: "apache-2.0",
      name: "Apache License 2.0",
      url: "https://www.apache.org/licenses/LICENSE-2.0",
      open: true,
      summary: "A permissive open-source licence.",
    },
    fit: {
      verdict: "comfortable",
      quant: "Q8_0",
      context: 32768,
      kv: "f16",
      est_memory_gb: 10.0,
      est_tokens_per_sec: 5,
      speed_basis: null,
      notes: [],
    },
    gpu: {
      kind: "split",
      fit: {
        verdict: "comfortable",
        quant: "Q5_K_M",
        context: 32768,
        kv: "q8_0",
        est_memory_gb: 6.6,
        est_tokens_per_sec: 71,
        speed_basis: null,
        notes: [],
      },
    },
    ...over,
  });

  it("offers BOTH rungs a download, not just the one that runs in system RAM", async () => {
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [split()] });
    await loaded();

    const buttons = await screen.findAllByRole("button", { name: /^download$/i });
    expect(buttons).toHaveLength(2);
  });

  it("fetches the quant of the row it was clicked on", async () => {
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [split()] });
    pullLocalModel.mockResolvedValue(undefined);
    await loaded();

    // The second row is "Fastest on GPU" — the rung that had no button at all before.
    fireEvent.click((await screen.findAllByRole("button", { name: /^download$/i }))[1]);

    await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());
    expect(pullLocalModel.mock.calls[0][0]).toBe("hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M");
  });

  it("says so plainly when both rows are one file, instead of sending you hunting", async () => {
    // `gpu_fit` splits on context or KV precision alone, so a split whose rungs share a quant is
    // legal and pinned by a Rust test. The old caption asserted the second row was "a different
    // file" unconditionally, which was simply false here.
    localModelRecommendations.mockResolvedValue({
      ...recs(),
      curated: [
        split({
          gpu_pull: {
            tag: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q8_0",
            sharded: false,
            same_file: true,
          },
        }),
      ],
    });
    await loaded();

    expect(await screen.findByText(/Both rows are the same file/)).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /^download$/i })).toHaveLength(1);
    expect(screen.queryByText(/different file/)).toBeNull();
  });

  it("keeps the copyable commands reachable after an Ollama endpoint is connected", async () => {
    // They used to be deleted outright at exactly this moment — including the llama-server line,
    // which is for a different runner and has nothing to do with whether PM can drive Ollama.
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [split()] });
    await loaded();

    fireEvent.click(await screen.findByRole("button", { name: /install it another way/i }));

    // The runner guide elsewhere on the page names llama-server too, so scope by count, not identity.
    expect((await screen.findAllByText(/llama-server -hf/)).length).toBeGreaterThan(0);
    expect(
      screen.getByText(/ollama pull hf\.co\/bartowski\/Qwen2\.5-7B-Instruct-GGUF:Q5_K_M/),
    ).toBeTruthy();
  });

  it("gives each row a llama-server line that runs it the way the row was sized", async () => {
    // A bare `-hf repo:quant` loads the model's whole trained context on a current build, which is
    // not what either row measured — and the page called it "the exact command for each model".
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [split()] });
    const { container } = await loaded();
    fireEvent.click(await screen.findByRole("button", { name: /install it another way/i }));
    const models = container.querySelector("#sec-localai-models") as HTMLElement;
    expect(
      within(models).getByText(
        "llama-server -hf bartowski/Qwen2.5-7B-Instruct-GGUF:Q8_0 --ctx-size 32768 -np 1",
      ),
    ).toBeTruthy();
    expect(
      within(models).getByText(
        "llama-server -hf bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M --ctx-size 32768 -np 1 -fa on -ctk q8_0 -ctv q8_0",
      ),
    ).toBeTruthy();
    // An `ollama pull` can't carry either, so the card says what to set instead.
    expect(models.textContent).toContain(
      "An ollama pull only fetches the file: Ollama runs every model at the one context it is set to, so set that to the context on the row you choose, and its cache to q8_0 if the row shows “q8_0 KV”, for it to run the way PM sized it.",
    );
  });

  it("marks a rung Installed only when that exact quant is being served", async () => {
    listLocalLlmModels.mockResolvedValue(
      served({ id: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M", embedding: false }),
    );
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [split()] });
    await loaded();

    expect(await screen.findByText(/^Installed$/)).toBeTruthy();
    // The other rung is a genuinely different file, so it keeps its own Download.
    expect(screen.getAllByRole("button", { name: /^download$/i })).toHaveLength(1);
  });
});

describe("a download owned by the backend", () => {
  // The pull is a backend job precisely so the settings view can unmount (the tab router unmounts
  // on every switch) without the download losing its UI — the old component-owned state came back
  // as a RE-ARMED Download button over a server still saturating the connection.
  const pulled = (): LocalRecommendation => ({
    repo: "bartowski/Phi-3.5-mini-instruct-GGUF",
    display_name: "Phi 3.5 mini instruct",
    architecture: "phi3",
    role_hint: "background",
    parameters_b: 3.8,
    active_parameters_b: 3.8,
    context_length: 8192,
    multimodal: false,
    reasoning: null,
    // The shape the generator really emits, naming this fixture's own repo + fitted quant.
    ollama_pull: "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
    sharded_quant: false,
    gpu_pull: null,
    licence: {
      id: "mit",
      name: "MIT License",
      url: "https://opensource.org/license/mit",
      open: true,
      summary: "A permissive open-source licence.",
    },
    fit: {
      verdict: "comfortable",
      quant: "Q4_K_M",
      context: 8192,
      kv: "f16",
      est_memory_gb: 2.6,
      est_tokens_per_sec: 35,
      speed_basis: null,
      notes: [],
    },
    gpu: { kind: "single" },
  });

  it("is adopted on mount: the card shows Downloading with a Cancel, not a re-armed button", async () => {
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [pulled()] });
    activeLocalPull.mockResolvedValue({
      model: "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
      status: "downloading",
      completed_bytes: 1024,
      total_bytes: 4096,
      running: true,
      error: null,
      started_at_ms: 0,
    });
    await loaded();

    const downloading = await screen.findByRole("button", { name: /downloading/i });
    expect((downloading as HTMLButtonElement).disabled).toBe(true);
    expect(pullLocalModel).not.toHaveBeenCalled();
    // The list comes back folded on every mount, and a closed fold is `inert`: role queries still
    // find what is inside it, which is how this used to pass with the only Cancel out of reach. The
    // progress and the Cancel are above the fold while it is closed.
    const cancel = screen.getByRole("button", { name: /cancel/i });
    expect(cancel.closest("[inert]")).toBeNull();
    expect(
      screen
        .getByRole("progressbar", { name: "Downloading Phi 3.5 mini instruct" })
        .closest("[inert]"),
    ).toBeNull();
  });

  it("is shown once: above the folded list, then on its card once the list is open", async () => {
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [pulled()] });
    activeLocalPull.mockResolvedValue({
      model: "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
      status: "downloading",
      completed_bytes: 1024,
      total_bytes: 4096,
      running: true,
      error: null,
      started_at_ms: 0,
    });
    const { container } = await loaded();
    await screen.findByRole("button", { name: /cancel/i });
    const models = container.querySelector("#sec-localai-models") as HTMLElement;

    fireEvent.click(within(models).getByRole("button", { name: "Show it" }));

    expect(
      within(models).getByRole("button", { name: "Show the model" }).getAttribute("aria-expanded"),
    ).toBe("true");
    const cancels = screen.getAllByRole("button", { name: /cancel/i });
    expect(cancels).toHaveLength(1);
    expect(cancels[0].closest("#localai-rec-bartowski-phi-3-5-mini-instruct-gguf")).toBeTruthy();
    expect(cancels[0].closest("[inert]")).toBeNull();
  });

  it("still clears its own marker when the download finishes", async () => {
    // The per-tag guard below must not become a way for the marker to stick: write the happy path
    // first, because a guard that never clears looks exactly like a download that never ends.
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [pulled()] });
    pullLocalModel.mockResolvedValue(undefined);
    await loaded();

    fireEvent.click(screen.getByRole("button", { name: /^download$/i }));
    await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());

    expect(await screen.findByRole("button", { name: /^download$/i })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /downloading/i })).toBeNull();
  });

  it("a refused second download leaves the running one on screen, not a blank card", async () => {
    // Reachable through the licence dialog, whose Accept button is not gated on another pull being
    // live: open it, let a backend-owned download get adopted underneath, then confirm. The backend
    // refuses the second pull — and the refusal must not take the FIRST one's progress bar with it.
    // It used to: `pull()` ended in an unconditional reset, and the 1s snapshot poller is keyed on
    // that marker, so a live multi-gigabyte download went invisible until the view remounted.
    const gemma: LocalRecommendation = {
      ...pulled(),
      repo: "bartowski/gemma-2-2b-it-GGUF",
      display_name: "gemma 2 2b it",
      ollama_pull: "hf.co/bartowski/gemma-2-2b-it-GGUF:Q4_K_M",
      licence: {
        id: "gemma",
        name: "Gemma Terms of Use",
        url: "https://ai.google.dev/gemma/terms",
        open: false,
        summary: "Google's own terms, not an open-source licence.",
      },
    };
    localModelRecommendations.mockResolvedValue({
      ...recs(),
      curated: [pulled(), gemma],
      terms_accepted: [],
    });
    acceptLocalModelTerms.mockResolvedValue(["gemma"]);
    await loaded();

    fireEvent.click(screen.getAllByRole("button", { name: /^download$/i })[1]);
    // The dialog, not the card's own licence link — the licence NAME appears in both.
    await screen.findByRole("button", { name: /accept and download/i });

    // The Phi pull is adopted while the dialog sits open; the backend then refuses the second.
    activeLocalPull.mockResolvedValue({
      model: "hf.co/bartowski/Phi-3.5-mini-instruct-GGUF:Q4_K_M",
      status: "downloading",
      completed_bytes: 2048,
      total_bytes: 8192,
      running: true,
      error: null,
      started_at_ms: 0,
    });
    pullLocalModel.mockRejectedValue(new Error("a model download is already running"));

    fireEvent.click(screen.getByRole("button", { name: /accept and download/i }));

    expect(await screen.findByRole("button", { name: /downloading/i })).toBeTruthy();
    expect(screen.getByRole("button", { name: /cancel/i })).toBeTruthy();
    expect(await screen.findByText(/already running/)).toBeTruthy();
  });
});

// ---------------------------------------------------------------------------------------------
// "Test it" — the only thing in the tab that asks the model to actually produce a token. Every
// other check is metadata (the server answers, the id is in the list, the weights are on disk),
// and the setups that fail fail at the step none of those cover.
// ---------------------------------------------------------------------------------------------

const testPass = (over: Partial<LocalTestResult> = {}): LocalTestResult => ({
  model: "llama3.2:1b",
  ok: true,
  reply: "ready",
  elapsed_ms: 2400,
  loaded_for_test: false,
  was_holding: [],
  message: null,
  ...over,
});

describe("testing a role's model", () => {
  it("offers the button only where there is a local pair to test", async () => {
    // The fixture routes Chat local with a model, and Background to cloud with none.
    await loaded();
    expect(screen.getAllByRole("button", { name: /^test it$/i })).toHaveLength(1);
  });

  it("shows what the model actually said, not just that it worked", async () => {
    testLocalLlm.mockResolvedValue(testPass());
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));

    const outcome = await screen.findByText(/Answered in 2.4s/);
    // The reply IS the evidence — a tick with nothing behind it is the reassurance this exists to
    // stop giving. Asserted on the outcome line itself rather than the page, which says "ready"
    // twice more in copy that has nothing to do with the test.
    expect(outcome.textContent).toContain("ready");
    expect(testLocalLlm).toHaveBeenCalledWith("chat");
  });

  it("says when the test itself loaded the model, because PM then owns that memory", async () => {
    testLocalLlm.mockResolvedValue(testPass({ loaded_for_test: true }));
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    expect(await screen.findByText(/release setting applies to it/)).toBeTruthy();
  });

  it("reports a server that answered without saying anything usable as a failure", async () => {
    // A 200 the gateway would score as `Alive` for health. The question here is different — "does
    // this work" — and the honest answer is no.
    testLocalLlm.mockResolvedValue(
      testPass({
        ok: false,
        reply: null,
        message: "the server answered, but the model's reply was empty.",
      }),
    );
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    expect(await screen.findByText(/reply was empty/)).toBeTruthy();
    expect(screen.queryByText(/Answered in/)).toBeNull();
  });

  it("surfaces a refusal raised before anything was sent", async () => {
    testLocalLlm.mockRejectedValue(new Error("a test is already running"));
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    expect(await screen.findByText(/already running/)).toBeTruthy();
  });

  it("drops a result the settings above it have just made untrue", async () => {
    testLocalLlm.mockResolvedValue(testPass());
    setLocalLlmRoleModel.mockResolvedValue(undefined);
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    await screen.findByText(/Answered in 2.4s/);

    // A pass shown against a model you have since swapped is worse than no pass at all.
    const modelSelect = screen
      .getAllByRole("combobox")
      .find((el) => (el as HTMLSelectElement).value === "llama3.2:1b");
    fireEvent.change(modelSelect as HTMLSelectElement, { target: { value: "" } });
    await waitFor(() => expect(screen.queryByText(/Answered in 2.4s/)).toBeNull());
  });

  it("picks a running test back up after the tab has been away", async () => {
    // The tab router unmounts this view on every switch and a test can take minutes. Held only in
    // component state, the answer would be lost by looking at another tab — and the button would
    // come back enabled while the backend was still refusing a second one.
    activeLocalTest.mockResolvedValue({
      role: "chat",
      model: "llama3.2:1b",
      running: true,
      result: null,
    });
    await loaded();
    expect(await screen.findByRole("button", { name: /testing/i })).toBeTruthy();
  });

  it("adopts a result that landed while the tab was elsewhere", async () => {
    activeLocalTest.mockResolvedValue({
      role: "chat",
      model: "llama3.2:1b",
      running: false,
      result: testPass(),
    });
    await loaded();
    expect(await screen.findByText(/Answered in 2.4s/)).toBeTruthy();
  });

  it("never shows a result under a model it did not ask", async () => {
    // The pickers stay live during a test, and the backend job outlives this view — so a result
    // can land after the row has been pointed at something else.
    activeLocalTest.mockResolvedValue({
      role: "chat",
      model: "some-other-model",
      running: false,
      result: testPass({ model: "some-other-model" }),
    });
    await loaded();
    expect(screen.queryByText(/Answered in/)).toBeNull();
  });

  it("says what the server was already holding, because loading may have displaced it", async () => {
    testLocalLlm.mockResolvedValue(
      testPass({ loaded_for_test: true, was_holding: ["qwen2.5:14b"] }),
    );
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    expect(await screen.findByText(/may have unloaded that/)).toBeTruthy();
    expect(screen.getByText(/qwen2.5:14b/)).toBeTruthy();
  });

  it("admits when it could not check what was loaded, and what that costs", async () => {
    // PM only unloads what it can prove it loaded, so a load it could not observe is one it will
    // never hand back. Saying so beats an invisible leak.
    testLocalLlm.mockResolvedValue(testPass({ loaded_for_test: null }));
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: /^test it$/i }));
    expect(await screen.findByText(/won.t hand that memory back/)).toBeTruthy();
  });

  it("explains the wait before the click when the model is mid-answer", async () => {
    localLlmStatus.mockResolvedValue(statusFix({ chat_answering: true }));
    await loaded();
    // Not a refusal — the button still works. A test arriving during a reply waits for the lane,
    // which can be the length of the whole reply, and that is worth saying in advance.
    expect(await screen.findByText(/would wait its turn/)).toBeTruthy();
    expect((screen.getByRole("button", { name: /^test it$/i }) as HTMLButtonElement).disabled).toBe(
      false,
    );
  });
});

describe("the On battery section (#432)", () => {
  it("sits between Assign roles and the graphics card, in the order the rail lists them", async () => {
    // It moves only what Assign roles set to Local, fall back to cloud, and what moving can't save
    // the graphics-card section below can — so it reads between the two, and the settings rail
    // (registry.ts) scrolls to it in that same order.
    const { container } = await loaded();
    const ids = Array.from(container.querySelectorAll("[data-settings-section]")).map(
      (el) => el.id,
    );
    const at = (id: string) => ids.indexOf(id);
    expect(at("sec-localai-power")).toBeGreaterThan(-1);
    expect(at("sec-localai-power")).toBe(at("sec-localai-roles") + 1);
    expect(at("sec-localai-lifecycle")).toBe(at("sec-localai-power") + 1);
  });
});

describe("PM's pick follows the server", () => {
  // The backend counts a file on disk towards the pick only if the stored server could serve it,
  // and reports what a server holds only once there is one — so the pick and "Already on this
  // device" change when the server does, even when what it serves (nothing) doesn't.
  it("is read again on connecting to a server that serves nothing yet", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    listLocalLlmModels.mockResolvedValue([]);
    setLocalLlmEndpoint.mockResolvedValue("http://127.0.0.1:11434");
    render(<LocalAiSettings />);
    fireEvent.change(await screen.findByLabelText("Endpoint URL"), {
      target: { value: "http://127.0.0.1:11434" },
    });
    await waitFor(() => expect(localModelRecommendations).toHaveBeenCalledTimes(1));

    getLocalLlmConfig.mockResolvedValue(cfg({ chat_model: "", chat_routing: "cloud" }));
    localModelRecommendations.mockResolvedValue({ ...recs(), endpoint_inventory: 0 });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));

    await waitFor(() => expect(localModelRecommendations).toHaveBeenCalledTimes(2));
    expect(await screen.findByText(/nothing has been downloaded into it yet/)).toBeTruthy();
  });

  it("is read again on disconnecting from one", async () => {
    listLocalLlmModels.mockResolvedValue([]);
    clearLocalLlmEndpoint.mockResolvedValue(undefined);
    await loaded();
    // The listing has answered (the empty-server line only shows once it has).
    await screen.findByText(/isn't serving any models yet/i);
    expect(localModelRecommendations).toHaveBeenCalledTimes(1);

    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Disconnect" }));

    await waitFor(() => expect(localModelRecommendations).toHaveBeenCalledTimes(2));
  });
});

describe("looking for a server on this computer", () => {
  it("never probes the ports for a user who is already connected", async () => {
    // `configured` is false until the stored config has been read, so every visit to the tab sent a
    // probe to 11434, 1234 and 8080 and threw the answer away.
    await loaded();
    await act(async () => {
      await Promise.resolve();
    });
    expect(probeLocalLlmPorts).not.toHaveBeenCalled();
  });

  it("still looks once the config says nothing is connected", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    render(<LocalAiSettings />);
    await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1));
  });
});

describe("disconnecting", () => {
  // It forgets more than the address — the token and both jobs' models go with it — so it asks.
  it("asks first, and backing out calls nothing", async () => {
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    expect(await screen.findByText("Disconnect from your model server?")).toBeTruthy();
    expect(screen.getByText(/Your server and the models in it aren't touched\./)).toBeTruthy();
    expect(clearLocalLlmEndpoint).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(clearLocalLlmEndpoint).not.toHaveBeenCalled();
  });

  it("forgets the server once confirmed", async () => {
    clearLocalLlmEndpoint.mockResolvedValue(undefined);
    await loaded();
    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    fireEvent.click(await screen.findByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(clearLocalLlmEndpoint).toHaveBeenCalledTimes(1));
  });
});

describe("the settings PM's numbers assume", () => {
  // PM sizes most graphics-card configs on a compressed cache and a longer context than servers
  // start with; until this fold, nothing told anyone to set either.
  it("gives the connected server's own steps for both, folded", async () => {
    await loaded();
    const fold = screen.getByRole("button", { name: "Settings PM's numbers assume" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(screen.getByText(/These are the Ollama settings for both:/)).toBeTruthy();
    expect(screen.getAllByText("OLLAMA_KV_CACHE_TYPE").length).toBeGreaterThan(0);
    fireEvent.click(fold);
    expect(fold.getAttribute("aria-expanded")).toBe("true");
  });

  it("isn't offered for a server PM can't name", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: "http://127.0.0.1:9000" }));
    await loaded();
    expect(screen.queryByRole("button", { name: "Settings PM's numbers assume" })).toBeNull();
  });
});

describe("errors are said where they happened", () => {
  // One Callout at the top of a long tab read as a fault in whatever was nearest it. Each section
  // now says its own.
  it("puts a refused connect under the form that sent it", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    setLocalLlmEndpoint.mockRejectedValue(new Error("refusing a public cleartext address"));
    const { container } = render(<LocalAiSettings />);
    fireEvent.change(await screen.findByLabelText("Endpoint URL"), {
      target: { value: "http://203.0.113.9:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));
    const error = await screen.findByText(/refusing a public cleartext address/);
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-endpoint");
    expect(container.querySelectorAll('[role="alert"]')).toHaveLength(1);
  });

  it("puts a failed role write under Assign roles", async () => {
    setLocalLlmRouting.mockRejectedValue(new Error("vault locked"));
    await loaded();
    fireEvent.change(screen.getByRole("combobox", { name: "Where chat runs" }), {
      target: { value: "cloud" },
    });
    const error = await screen.findByText(/vault locked/);
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-roles");
  });

  it("puts a failed hardware read under the machine readout", async () => {
    localModelRecommendations.mockRejectedValue(new Error("couldn't read the GPU"));
    await loaded();
    const error = await screen.findByText(/couldn't read the GPU/);
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-machine");
  });
});

describe("pointers between sections", () => {
  it("are section names that take you there, never directions", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
    const onLocate = vi.fn();
    const { container } = render(<LocalAiSettings onLocate={onLocate} />);
    await screen.findByLabelText("Endpoint URL");
    const roles = container.querySelector("#sec-localai-roles") as HTMLElement;
    const link = Array.from(roles.querySelectorAll("button")).find(
      (b) => b.textContent === sectionLabel("sec-localai-endpoint"),
    );
    expect(link).toBeTruthy();
    fireEvent.click(link as HTMLButtonElement);
    await waitFor(() => expect(onLocate).toHaveBeenCalledWith("sec-localai-endpoint"));
  });
});

describe("speed on the model list", () => {
  it("says where each figure comes from, and never writes a tilde", async () => {
    const card: LocalRecommendation = {
      repo: "bartowski/Qwen2.5-7B-Instruct-GGUF",
      display_name: "Qwen2.5 7B Instruct",
      architecture: "qwen2",
      role_hint: null,
      parameters_b: 7.62,
      active_parameters_b: 7.62,
      context_length: 32768,
      multimodal: false,
      reasoning: null,
      ollama_pull: "hf.co/bartowski/Qwen2.5-7B-Instruct-GGUF:Q5_K_M",
      sharded_quant: false,
      gpu_pull: null,
      licence: {
        id: "apache-2.0",
        name: "Apache License 2.0",
        url: "https://www.apache.org/licenses/LICENSE-2.0",
        open: true,
        summary: "A permissive open-source licence.",
      },
      fit: {
        verdict: "tight",
        quant: "Q5_K_M",
        context: 32768,
        kv: "q8_0",
        est_memory_gb: 6.63,
        est_tokens_per_sec: 71.0,
        speed_basis: "gpu_published",
        notes: [],
      },
      gpu: { kind: "single" },
    };
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [card] });
    const { container } = await loaded();
    const models = container.querySelector("#sec-localai-models") as HTMLElement;
    expect(models.textContent).toContain("about 71 tok/s");
    expect(models.textContent).toContain(
      "Speeds are PM's estimates, not measurements on this computer: from published or typical memory speeds",
    );
    expect(models.textContent).not.toMatch(/up to \d/);
    expect(models.textContent).not.toMatch(/~\s?\d/);
    // The numbers guide names where the settings live, by its section — and only as somewhere they
    // are once a server is connected, which is the only time that fold exists.
    expect(models.textContent).toContain(
      `your server needs that setting too, and the context the card shows. Each model's commands say what to set, and once your server is connected, ${sectionLabel("sec-localai-endpoint")}'s “Settings PM's numbers assume” has the steps.`,
    );
  });

  it("explains a mixture of experts' figure the way PM works it out on a card", async () => {
    // fit.rs halves a MoE's card figure (`MOE_GPU_FACTOR`), so gemma 4 26B A4B (3.82B active) shows
    // about 32 at 384 GB/s where a dense 4B shows about 80. A guide saying a MoE "runs at the speed of
    // that small active part" would be contradicted by the card right beside it.
    const card: LocalRecommendation = {
      repo: "unsloth/gemma-4-26B-A4B-it-GGUF",
      display_name: "gemma 4 26B A4B it",
      architecture: "gemma4",
      role_hint: null,
      parameters_b: 25.23,
      active_parameters_b: 3.82,
      context_length: 262144,
      multimodal: false,
      reasoning: null,
      ollama_pull: "hf.co/unsloth/gemma-4-26B-A4B-it-GGUF:Q3_K_M",
      sharded_quant: false,
      gpu_pull: null,
      licence: {
        id: "apache-2.0",
        name: "Apache License 2.0",
        url: "https://www.apache.org/licenses/LICENSE-2.0",
        open: true,
        summary: "A permissive open-source licence.",
      },
      fit: {
        verdict: "tight",
        quant: "Q3_K_M",
        context: 32768,
        kv: "f16",
        est_memory_gb: 13.2,
        est_tokens_per_sec: 32.1,
        speed_basis: "gpu_published",
        notes: [],
      },
      gpu: { kind: "single" },
    };
    localModelRecommendations.mockResolvedValue({ ...recs(), curated: [card] });
    const { container } = await loaded();
    const models = container.querySelector("#sec-localai-models") as HTMLElement;
    expect(models.textContent).toContain("about 32 tok/s");
    expect(models.textContent).toContain("3.82B active");
    const term = Array.from(models.querySelectorAll("dt")).find((dt) =>
      dt.textContent?.startsWith("MoE (mixture of experts)"),
    );
    const entry = term?.nextElementSibling?.textContent ?? "";
    expect(entry).toContain(
      "On a graphics card it isn't as quick as an ordinary model the size of its active part: going by one published report, PM halves its estimate for a MoE there, since PM hasn't timed one itself",
    );
    expect(entry).toContain("quicker than an ordinary model of its full size");
    expect(entry).not.toMatch(/runs at the speed of/);
  });
});

describe("resetting the tab to its defaults", () => {
  // The footer every other settings tab has (#445). Whether the tab is at its defaults is the
  // backend's answer, so what is pinned here is that the tab ASKS — on mount and after each write it
  // makes — and that a reset leaves nothing on screen from before it.
  const resetButton = () =>
    screen.getByRole("button", { name: "Reset to defaults" }) as HTMLButtonElement;
  const start = () => document.getElementById("sec-localai-start") as HTMLElement;
  /** The backend says something is stored, and the footer has heard it. */
  async function offered() {
    await loaded();
    await waitFor(() => expect(resetButton().disabled).toBe(false));
  }
  async function confirmReset() {
    fireEvent.click(resetButton());
    fireEvent.click(await screen.findByRole("button", { name: "Reset" }));
  }

  it("asks the backend, and is disabled and says so while nothing is stored", async () => {
    localAiSettingsAreDefault.mockResolvedValue(true);
    await loaded();
    await waitFor(() => expect(localAiSettingsAreDefault).toHaveBeenCalled());
    expect(screen.getByText("Reset Local AI")).toBeTruthy();
    expect(screen.getByText("Everything on this tab is at its default.")).toBeTruthy();
    expect(resetButton().disabled).toBe(true);
  });

  it("is offered once the backend says something is stored", async () => {
    await offered();
    expect(screen.getByText("Restore this tab's settings to their defaults.")).toBeTruthy();
  });

  it("never reads a failed answer as 'at its defaults'", async () => {
    // A keychain PM couldn't read is not an empty one: hiding the reset then would hide the one
    // control that clears the token.
    localAiSettingsAreDefault.mockRejectedValue(new Error("the keychain is locked"));
    await offered();
  });

  it("asks before resetting, says what goes and what stays, and Cancel calls nothing", async () => {
    await offered();
    fireEvent.click(resetButton());
    expect(await screen.findByText("Reset Local AI to defaults?")).toBeTruthy();
    expect(
      screen.getByText(
        /^Disconnects your model server and forgets its access key, and puts roles, On battery, Model memory, the extra folder, the update check and the licences you agreed to back to their defaults\./,
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        /Your models stay where they are — on your server and on this computer — and so do your cloud key, your chats and the tray icon\.$/,
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(resetLocalAiSettings).not.toHaveBeenCalled();
  });

  it("is described in What's New the way the confirm describes it: the tray icon stays", async () => {
    // Model memory shows the tray toggle, but the reset leaves it (local_ai.rs `LOCAL_AI_SETTINGS`
    // holds no tray key). A release note promising "all of it back to its defaults" with only
    // models, cloud key and chats left alone was wrong for anyone with the tray on.
    const release = CHANGELOG.find((e) => e.version === "3.138.0-alpha");
    const notes = (release?.highlights ?? []).filter((h) => /Reset Local AI/.test(h));
    expect(notes).toHaveLength(1);
    expect(notes[0]).toMatch(
      /leaving your models, your cloud key, your chats and the tray icon where they are\./,
    );
    // The same list the confirm gives, which this tab's own tests above pin word for word.
    await offered();
    expect(screen.getByRole("switch", { name: /tray/i })).toBeTruthy();
    fireEvent.click(resetButton());
    expect(
      await screen.findByText(/and so do your cloud key, your chats and the tray icon\.$/),
    ).toBeTruthy();
  });

  it("resets, then reads the whole tab again: step 1, and nothing from before", async () => {
    const onLocate = vi.fn();
    const onBetterFitChange = vi.fn();
    render(<LocalAiSettings onLocate={onLocate} onBetterFitChange={onBetterFitChange} />);
    await screen.findByText(/Connected to/);
    await waitFor(() => expect(resetButton().disabled).toBe(false));
    const reads = {
      config: getLocalLlmConfig.mock.calls.length,
      recs: localModelRecommendations.mock.calls.length,
      release: getLocalReleasePolicy.mock.calls.length,
      test: activeLocalTest.mock.calls.length,
    };
    // What the backend holds once it has reset.
    getLocalLlmConfig.mockResolvedValue(
      cfg({ base_url: null, chat_model: null, background_model: null, chat_routing: "cloud" }),
    );
    listLocalLlmModels.mockResolvedValue([]);
    localAiSettingsAreDefault.mockResolvedValue(true);

    await confirmReset();

    await waitFor(() => expect(resetLocalAiSettings).toHaveBeenCalledTimes(1));
    // Every section starts again from what is stored — the stored config, the pick, the release
    // settings, the backend's test — rather than one of them going on showing the old server.
    await waitFor(() => expect(getLocalLlmConfig.mock.calls.length).toBeGreaterThan(reads.config));
    await waitFor(() =>
      expect(localModelRecommendations.mock.calls.length).toBeGreaterThan(reads.recs),
    );
    expect(getLocalReleasePolicy.mock.calls.length).toBeGreaterThan(reads.release);
    expect(activeLocalTest.mock.calls.length).toBeGreaterThan(reads.test);
    await waitFor(() =>
      expect(start().querySelector('[aria-current="step"]')?.textContent).toContain(
        "Get a model server",
      ),
    );
    expect(screen.queryByText(/Connected to/)).toBeNull();
    // The footer heard the new answer, and the reader is taken to where they now start.
    await waitFor(() => expect(resetButton().disabled).toBe(true));
    await waitFor(() => expect(onLocate).toHaveBeenCalledWith("sec-localai-start"));
    // The update check was one of the settings; the better-fit dot follows it.
    expect(onBetterFitChange).toHaveBeenCalled();
  });

  it("says a reset that failed, and leaves the tab, and the reader, where they were", async () => {
    // The backend refuses before it changes anything (a locked vault, a keychain that won't let go
    // of the token), so there is nothing new to show — and remounting would re-lay the tab out from
    // its loading state and scroll the error out of sight.
    resetLocalAiSettings.mockRejectedValue("couldn't remove the token from the keychain");
    const onLocate = vi.fn();
    render(<LocalAiSettings onLocate={onLocate} />);
    await screen.findByText(/Connected to/);
    await waitFor(() => expect(resetButton().disabled).toBe(false));
    const configReads = getLocalLlmConfig.mock.calls.length;
    const answers = localAiSettingsAreDefault.mock.calls.length;

    await confirmReset();

    expect(await screen.findByText("couldn't remove the token from the keychain")).toBeTruthy();
    expect(getLocalLlmConfig.mock.calls.length).toBe(configReads);
    expect(screen.getByText(/Connected to/)).toBeTruthy();
    expect(onLocate).not.toHaveBeenCalled();
    // The footer's own answer is asked again regardless: the store can fail after the token went.
    expect(localAiSettingsAreDefault.mock.calls.length).toBeGreaterThan(answers);
    expect(resetButton().disabled).toBe(false);
  });

  it("asks again after every write the tab makes", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg({ has_token: true }));
    await offered();
    let before = localAiSettingsAreDefault.mock.calls.length;

    // A `set_*` write announces itself from ipc.ts's `invoke`, which is mocked away here — so the
    // announcement is made by hand.
    act(() => {
      window.dispatchEvent(new Event(SETTING_SAVED_EVENT));
    });
    await waitFor(() =>
      expect(localAiSettingsAreDefault.mock.calls.length).toBeGreaterThan(before),
    );

    // Forget token is not a `set_*` command, so the tab says so itself.
    before = localAiSettingsAreDefault.mock.calls.length;
    fireEvent.click(screen.getByRole("button", { name: /forget token/i }));
    fireEvent.click(screen.getByRole("button", { name: /forget it/i }));
    await waitFor(() => expect(clearLocalLlmToken).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(localAiSettingsAreDefault.mock.calls.length).toBeGreaterThan(before),
    );
  });

  it("keeps the newest answer when an older one lands last", async () => {
    // A connect writes the address and then the token: two answers in flight, and the first to be
    // asked must not be the one left on screen.
    let stale!: (atDefaults: boolean) => void;
    localAiSettingsAreDefault.mockImplementationOnce(
      () => new Promise<boolean>((resolve) => (stale = resolve)),
    );
    await loaded();
    act(() => {
      window.dispatchEvent(new Event(SETTING_SAVED_EVENT));
    });
    await waitFor(() => expect(resetButton().disabled).toBe(false));

    await act(async () => {
      stale(true);
      await Promise.resolve();
    });
    expect(resetButton().disabled).toBe(false);
  });
});
