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

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type {
  LocalLlmConfig,
  LocalRecommendation,
  LocalRecommendations,
  LocalServedModel,
} from "../../lib/types";

const checkLocalLlmEndpoint = vi.fn();
const clearLocalLlmEndpoint = vi.fn();
const clearLocalLlmToken = vi.fn();
const dismissLocalBetterFit = vi.fn();
const getLocalLlmConfig = vi.fn();
const listLocalLlmModels = vi.fn();
const localBetterFitNotice = vi.fn();
const localHardwareScan = vi.fn();
const localLlmStatus = vi.fn();
const localModelRecommendations = vi.fn();
const probeLocalLlmPorts = vi.fn();
const pullLocalModel = vi.fn();
const acceptLocalModelTerms = vi.fn();
const setLocalLlmEndpoint = vi.fn();
const setLocalLlmRoleModel = vi.fn();
const setLocalLlmRouting = vi.fn();
const setLocalLlmToken = vi.fn();
const setLocalModelRescanCadence = vi.fn();
const setLocalModelScanDir = vi.fn();

// A factory REPLACES the whole module, so every function the component imports must appear here or
// it is `undefined` at module-eval. LocalAiSettings imports eighteen.
vi.mock("../../lib/ipc", () => ({
  checkLocalLlmEndpoint: (...a: unknown[]) => checkLocalLlmEndpoint(...a),
  clearLocalLlmEndpoint: () => clearLocalLlmEndpoint(),
  clearLocalLlmToken: () => clearLocalLlmToken(),
  dismissLocalBetterFit: () => dismissLocalBetterFit(),
  getLocalLlmConfig: () => getLocalLlmConfig(),
  listLocalLlmModels: () => listLocalLlmModels(),
  localBetterFitNotice: () => localBetterFitNotice(),
  localHardwareScan: (...a: unknown[]) => localHardwareScan(...a),
  localLlmStatus: () => localLlmStatus(),
  localModelRecommendations: () => localModelRecommendations(),
  probeLocalLlmPorts: () => probeLocalLlmPorts(),
  pullLocalModel: (...a: unknown[]) => pullLocalModel(...a),
  acceptLocalModelTerms: (...a: unknown[]) => acceptLocalModelTerms(...a),
  setLocalLlmEndpoint: (...a: unknown[]) => setLocalLlmEndpoint(...a),
  setLocalLlmRoleModel: (...a: unknown[]) => setLocalLlmRoleModel(...a),
  setLocalLlmRouting: (...a: unknown[]) => setLocalLlmRouting(...a),
  setLocalLlmToken: (...a: unknown[]) => setLocalLlmToken(...a),
  setLocalModelRescanCadence: (...a: unknown[]) => setLocalModelRescanCadence(...a),
  setLocalModelScanDir: (...a: unknown[]) => setLocalModelScanDir(...a),
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

import { LocalAiSettings } from "./LocalAiSettings";

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
  catalog_version: 1,
  catalog_generated_at: "2026-07-22",
  endpoint_configured: true,
  cadence: "on-catalog-update",
  rescan_due: false,
  curated: [],
  installed: [],
  on_disk: [],
  disk_sources_present: [],
  disk_truncated: false,
  scan_dir: null,
  terms_accepted: [],
});

const served = (...models: LocalServedModel[]) => models;

beforeEach(() => {
  vi.clearAllMocks();
  getLocalLlmConfig.mockResolvedValue(cfg());
  listLocalLlmModels.mockResolvedValue(served({ id: "llama3.2:1b", embedding: false }));
  localModelRecommendations.mockResolvedValue(recs());
  localBetterFitNotice.mockResolvedValue(null);
  localLlmStatus.mockResolvedValue({
    configured: true,
    reachable: true,
    in_cooldown: false,
    cooldown_remaining_s: 0,
    probed_now: false,
  });
  clearLocalLlmToken.mockResolvedValue(undefined);
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

  it("warns that two local models share one machine, and only when they actually do", async () => {
    // Nothing else in PM says this. Both roles hit ONE endpoint, so two different models are both
    // resident and their memory adds up — while every Workbench verdict was worked out for a single
    // model against the whole budget. The better-fit check compares against the LARGER of the two,
    // never the sum, so it cannot warn about it either.
    const both = /holds both at once/i;

    // Default fixture: chat is local, background is on cloud — one model, nothing to warn about.
    await loaded();
    expect(screen.queryByText(both)).toBeNull();

    // Both local, but the SAME model on each: one resident model, the verdict already covers it.
    cleanup();
    getLocalLlmConfig.mockResolvedValue(
      cfg({ background_routing: "local", background_model: "llama3.2:1b" }),
    );
    await loaded();
    expect(screen.queryByText(both)).toBeNull();

    // Both local, two different models — the one case where the choices interact.
    cleanup();
    getLocalLlmConfig.mockResolvedValue(
      cfg({ background_routing: "local", background_model: "qwen2.5:7b" }),
    );
    await loaded();
    expect(screen.getByText(both)).toBeTruthy();
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
  // NOTE: this flow cannot be reached by clicking the real app today — every shipped catalogue entry
  // has `install.ollama: null`, so `canPull` is false and no Download button renders. These fixtures
  // populate it deliberately. Without them the gate would be dead code that nothing exercises.
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
    install: { ollama: "ollama pull gemma2:2b" },
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
      notes: [],
    },
    gpu: { kind: "single" },
    ...over,
  });

  const openModel = () =>
    model({
      repo: "bartowski/Phi-3.5-mini-instruct-GGUF",
      display_name: "Phi 3.5 mini instruct",
      install: { ollama: "ollama pull phi3.5:3.8b" },
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
      expect(pullLocalModel).toHaveBeenCalledWith("gemma2:2b", expect.anything()),
    );
    expect(acceptLocalModelTerms).toHaveBeenCalledWith("gemma");
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
      expect(pullLocalModel).toHaveBeenCalledWith("phi3.5:3.8b", expect.anything()),
    );
    expect(screen.queryByRole("button", { name: /accept and download/i })).toBeNull();
  });
});
