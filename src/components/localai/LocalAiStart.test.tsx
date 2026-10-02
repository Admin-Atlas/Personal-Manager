// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The start card, "Your local model", rendered inside the whole tab — its buttons are the tab's
// writes, and the point of most of these is that each one makes exactly the write a section control
// makes. The words themselves are pinned in readiness.test.ts and pickWords.test.ts; here it is what
// a person sees and what a click does.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  DetectedEndpoint,
  LocalFitResult,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalPick,
  LocalRecommendation,
  LocalRecommendations,
  LocalServedModel,
  PowerRoleView,
  PowerView,
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

// A factory REPLACES the whole module, so every wrapper anything in the tab imports must be here.
vi.mock("../../lib/ipc", () => ({
  activeLocalPull: () => activeLocalPull(),
  cancelLocalPull: () => cancelLocalPull(),
  checkLocalLlmEndpoint: (...a: unknown[]) => checkLocalLlmEndpoint(...a),
  clearLocalLlmEndpoint: () => clearLocalLlmEndpoint(),
  clearLocalLlmToken: () => clearLocalLlmToken(),
  dismissLocalBetterFit: () => dismissLocalBetterFit(),
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
  setLocalPowerPolicy: (...a: unknown[]) => setLocalPowerPolicy(...a),
  keepLocalOnBattery: (...a: unknown[]) => keepLocalOnBattery(...a),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

// Depth is a variable here: one test reads the card at "min", where nothing that justifies the pick
// may disappear.
const theme = vi.hoisted(() => ({ depth: "standard" }));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: theme.depth,
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
import {
  ACTION_MIRRORS,
  COPY_COLLISIONS,
  SAME_SECTION_POINTERS,
  type StepAction,
} from "./readiness";
import { sectionLabel } from "./sections";

afterEach(cleanup);

const QWEN = "bartowski/Qwen2.5-7B-Instruct-GGUF";
const QWEN_TAG = `hf.co/${QWEN}:Q5_K_M`;
const OLLAMA: DetectedEndpoint = { url: "http://127.0.0.1:11434", label: "Ollama", models: [] };

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

const PICK: LocalPick = {
  kind: "catalogue",
  repo: QWEN,
  display_name: "Qwen2.5 7B Instruct",
  rung: "gpu",
  tag: QWEN_TAG,
  fit: fit(),
  download_gb: 5.07,
  basis: "gpu",
  also_have: null,
};

const recs = (over: Partial<LocalRecommendations> = {}): LocalRecommendations => ({
  hardware: {
    platform: "linux",
    total_ram_gb: 32,
    available_ram_gb: 20,
    cpu_brand: "Test CPU",
    cpu_cores: 8,
    cpu_threads: 16,
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
  catalog_version: 4,
  catalog_generated_at: "2026-09-30",
  endpoint_configured: false,
  cadence: "on-catalog-update",
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
  pick: PICK,
  live_available_ram_gb: 20,
  ...over,
});

const cfg = (over: Partial<LocalLlmConfig> = {}): LocalLlmConfig => ({
  base_url: "http://127.0.0.1:11434",
  chat_model: "",
  background_model: "",
  chat_routing: "cloud",
  background_routing: "cloud",
  has_token: false,
  ...over,
});

const role = (over: Partial<PowerRoleView> = {}): PowerRoleView => ({
  route: "unchanged",
  blocked: null,
  local_model: null,
  effective: "cloud",
  cloud_key: "present",
  ...over,
});

const status = (
  chat: Partial<PowerRoleView> = {},
  background: Partial<PowerRoleView> = chat,
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
  power: { ...INERT_POWER_VIEW, chat: role(chat), background: role(background), ...power },
});

const served = (...ids: string[]): LocalServedModel[] =>
  ids.map((id) => ({ id, embedding: false }));

beforeEach(() => {
  vi.clearAllMocks();
  theme.depth = "standard";
  getLocalLlmConfig.mockResolvedValue(cfg({ base_url: null }));
  listLocalLlmModels.mockResolvedValue([]);
  localModelRecommendations.mockResolvedValue(recs());
  localBetterFitNotice.mockResolvedValue(null);
  localLlmStatus.mockResolvedValue(status());
  localGpuResidency.mockResolvedValue({
    resident: [],
    vram_gb: 7.96,
    dgpu_displays: [],
    policy: "server",
    idle_minutes: 5,
    no_unload_route: false,
  });
  releaseLocalGpu.mockResolvedValue(1);
  getLocalReleasePolicy.mockResolvedValue({
    policy: "server",
    idle_minutes: 5,
    battery_idle_minutes: 0,
  });
  setLocalReleasePolicy.mockResolvedValue(undefined);
  setLocalPowerPolicy.mockResolvedValue(undefined);
  keepLocalOnBattery.mockResolvedValue(undefined);
  getTrayEnabled.mockResolvedValue(false);
  setTrayEnabled.mockResolvedValue(undefined);
  activeLocalPull.mockResolvedValue(null);
  activeLocalTest.mockResolvedValue(null);
  cancelLocalPull.mockResolvedValue(true);
  probeLocalLlmPorts.mockResolvedValue([]);
  setLocalLlmEndpoint.mockImplementation((u: string) => Promise.resolve(u));
  setLocalLlmRoleModel.mockResolvedValue(undefined);
  setLocalLlmRouting.mockResolvedValue(undefined);
  pullLocalModel.mockResolvedValue(undefined);
  acceptLocalModelTerms.mockResolvedValue([]);
});

/** The start section. */
const start = () => document.getElementById("sec-localai-start") as HTMLElement;

/** Any section, by its anchor. */
const section = (id: string) => document.getElementById(id) as HTMLElement;

/** Whether an element is on screen — not inside a closed fold, whose body is `inert`. */
const shown = (el: Element) => el.closest("[inert]") === null;

/** Render the tab, and wait until the start card has read the setup. */
async function mount(onLocate?: (id: string) => void) {
  const view = render(<LocalAiSettings onLocate={onLocate} />);
  await waitFor(() => expect(getLocalLlmConfig).toHaveBeenCalled());
  await waitFor(() => expect(start().textContent).not.toContain("Reading your local AI setup"));
  await waitFor(() => expect(start().textContent).not.toContain("Sizing PM's models"));
  return view;
}

describe("a fresh install's first paint", () => {
  it("offers nothing to download or copy, one way to get a server, and no primary", async () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000_000);
    const { container } = await mount();
    await waitFor(() => expect(within(start()).getByText(/Get a model server/)).toBeTruthy());

    const buttons = Array.from(container.querySelectorAll("button")).filter(shown);
    expect(buttons.filter((b) => /^download/i.test(b.textContent ?? ""))).toHaveLength(0);
    expect(buttons.filter((b) => b.textContent === "Copy")).toHaveLength(0);
    expect(
      Array.from(container.querySelectorAll('[data-variant="primary"]')).filter(shown),
    ).toHaveLength(0);
    const licences = Array.from(container.querySelectorAll("a")).filter(
      (a) => shown(a) && a.getAttribute("href") === qwenRec().licence.url,
    );
    expect(licences).toHaveLength(1);
    const get = Array.from(container.querySelectorAll("a")).filter(
      (a) => shown(a) && /^Get Ollama at/.test(a.textContent ?? ""),
    );
    expect(get).toHaveLength(1);
    expect(get[0].getAttribute("href")).toBe("https://ollama.com/download");

    // It looks for a server on mount, and again when the window comes back — but not twice in a
    // breath.
    await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1));
    fireEvent.focus(window);
    expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1);
    now.mockReturnValue(1_006_000);
    fireEvent.focus(window);
    await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(2));
    now.mockRestore();
  });

  it("never probes the local ports for someone already connected", async () => {
    // The tab mounts before its config read lands, and "not connected" assumed in that moment sent
    // every long-connected user's visit a probe of three local ports — whatever runs on 8080 included.
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    await waitFor(() => expect(start().textContent).toContain("Your server is answering at"));
    fireEvent.focus(window);
    expect(probeLocalLlmPorts).not.toHaveBeenCalled();
  });

  it("says a failed read of the setup, rather than reading it forever", async () => {
    // Nothing reads the config again until the tab does, so "Reading…" and "Checking…" would never
    // end. The error itself is Model server's to say.
    getLocalLlmConfig.mockRejectedValue(new Error("the keychain is locked"));
    render(<LocalAiSettings />);
    await waitFor(() =>
      expect(start().textContent).toContain(
        "PM couldn't read your local AI setup, so it can't say where things stand.",
      ),
    );
    expect(start().textContent).not.toContain("Reading your local AI setup");
    expect(within(start()).getByText("Needs attention", { selector: "h2 + span" })).toBeTruthy();
    expect(section("sec-localai-endpoint").textContent).toContain("the keychain is locked");
  });

  it("shows the runner guide for the one chosen, and nowhere else", async () => {
    await mount();
    await screen.findByRole("group", { name: "Server to install" });
    fireEvent.click(within(start()).getByRole("button", { name: "LM Studio" }));
    expect(
      within(start()).getByRole("link", { name: /^Get LM Studio at lmstudio\.ai\/download/ }),
    ).toBeTruthy();
    // Model server keeps no second guide while nothing is connected — it points here.
    const endpoint = document.getElementById("sec-localai-endpoint") as HTMLElement;
    expect(
      within(endpoint).queryByRole("button", { name: /Compare the three local servers/ }),
    ).toBeNull();
    expect(endpoint.textContent).toContain(
      "Don't have one yet? Step 1 under Your local model has the install steps for this computer.",
    );
  });
});

describe("step 1 with a server found", () => {
  it("connects to it with the one write Model server's form makes", async () => {
    probeLocalLlmPorts.mockResolvedValue([OLLAMA]);
    await mount();
    const connect = await within(start()).findByRole("button", { name: "Connect to Ollama" });
    expect(connect.getAttribute("data-variant")).toBe("primary");
    // The endpoint lists it too, from the same probe.
    expect(document.getElementById("sec-localai-endpoint")?.textContent).toContain(
      "Found on this computer:",
    );
    getLocalLlmConfig.mockResolvedValue(cfg());
    fireEvent.click(connect);
    await waitFor(() => expect(setLocalLlmEndpoint).toHaveBeenCalledWith("http://127.0.0.1:11434"));
    expect(setLocalLlmEndpoint).toHaveBeenCalledTimes(1);
    expect(setLocalLlmToken).not.toHaveBeenCalled();
  });

  it("points the pick's settings at steps that will be there, not ones that aren't yet", async () => {
    // Model server's "Settings PM's numbers assume" only renders once something is connected.
    probeLocalLlmPorts.mockResolvedValue([OLLAMA]);
    await mount();
    await within(start()).findByRole("button", { name: "Connect to Ollama" });
    expect(start().textContent).toContain(
      `the steps are under ${sectionLabel("sec-localai-endpoint")}, in “Settings PM's numbers assume”, once you've connected.`,
    );
    expect(
      within(section("sec-localai-endpoint")).queryByRole("button", {
        name: /Settings PM's numbers assume/,
      }),
    ).toBeNull();
  });

  it("says a refused connect in the start card, where it was asked for", async () => {
    probeLocalLlmPorts.mockResolvedValue([OLLAMA]);
    setLocalLlmEndpoint.mockRejectedValue(new Error("the server refused"));
    await mount();
    fireEvent.click(await within(start()).findByRole("button", { name: "Connect to Ollama" }));
    const error = await screen.findByText(/the server refused/);
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-start");
  });
});

describe("step 2 — the pick's download", () => {
  const llama = (): LocalRecommendation =>
    qwenRec({
      repo: "bartowski/Meta-Llama-3.1-8B-Instruct-GGUF",
      display_name: "Llama 3.1 8B Instruct",
      ollama_pull: "hf.co/bartowski/Meta-Llama-3.1-8B-Instruct-GGUF:Q5_K_M",
      gpu_pull: null,
      gpu: { kind: "single" },
      fit: fit(),
      licence: {
        id: "llama3.1",
        name: "Llama 3.1 Community License",
        url: "https://llama.com/llama3_1/license",
        open: false,
        summary: "Meta's own terms, not an open-source licence.",
      },
    });

  it("asks about a restricted licence before anything downloads", async () => {
    const rec = llama();
    getLocalLlmConfig.mockResolvedValue(cfg());
    localModelRecommendations.mockResolvedValue(
      recs({
        curated: [rec],
        pick: { ...PICK, repo: rec.repo, display_name: rec.display_name, tag: rec.ollama_pull! },
      }),
    );
    acceptLocalModelTerms.mockResolvedValue(["llama3.1"]);
    await mount();
    const download = await within(start()).findByRole("button", { name: "Download (5.1 GB)" });
    expect(download.getAttribute("data-variant")).toBe("primary");
    fireEvent.click(download);
    expect(await screen.findByRole("button", { name: /accept and download/i })).toBeTruthy();
    expect(pullLocalModel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: /accept and download/i }));
    await waitFor(() =>
      expect(pullLocalModel).toHaveBeenCalledWith(rec.ollama_pull, expect.anything()),
    );
    expect(acceptLocalModelTerms).toHaveBeenCalledWith("llama3.1");
  });

  it("gives the pick's rung to the start card: All models says 'PM's pick' and the other rung keeps Download", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    await within(start()).findByRole("button", { name: "Download (5.1 GB)" });
    const models = document.getElementById("sec-localai-models") as HTMLElement;
    expect(within(models).getByText("PM's pick")).toBeTruthy();
    expect(within(models).getByRole("button", { name: "Show it" })).toBeTruthy();
    // The Highest-quality rung is a different file, with its own Download.
    expect(within(models).getAllByRole("button", { name: /^download$/i })).toHaveLength(1);
  });

  it("offers no download when PM isn't picking one", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    localModelRecommendations.mockResolvedValue(
      recs({
        pick: { kind: "nothing", reason: "nothing_on_gpu", basis: "gpu", system_fallback: true },
      }),
    );
    await mount();
    await waitFor(() => expect(start().textContent).toContain("Choose a model"));
    expect(within(start()).queryByRole("button", { name: /download/i })).toBeNull();
    expect(start().textContent).toContain("Nothing in PM's list fits your 8.0 GB graphics card");
    expect(within(start()).getByRole("button", { name: "Go to All models" })).toBeTruthy();
    expect(document.getElementById("sec-localai-models")?.textContent).toContain(
      "Show the model anyway",
    );
  });

  it("says the user already has it, and offers nothing to download", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
    localModelRecommendations.mockResolvedValue(
      recs({
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
          fit: fit({ quant: "Q4_K_M" }),
          basis: "gpu",
        },
      }),
    );
    await mount();
    expect(
      await within(start()).findByText("PM's pick for this computer — you already have it"),
    ).toBeTruthy();
    expect(within(start()).queryByRole("button", { name: /download/i })).toBeNull();
    expect(start().textContent).toContain("as qwen2.5:latest");
  });
});

describe("a payload with no pick", () => {
  // The pick is optional so it can be removed from the backend alone: the card must still work.
  it("renders no pick at all, and step 2 sends the reader to All models", async () => {
    const onLocate = vi.fn();
    getLocalLlmConfig.mockResolvedValue(cfg());
    localModelRecommendations.mockResolvedValue(recs({ pick: undefined }));
    await mount(onLocate);
    await waitFor(() => expect(start().textContent).toContain("Choose a model"));
    expect(start().textContent).not.toContain("PM's pick");
    expect(start().textContent).toContain(
      "Choose one under All models; each says how it would run here.",
    );
    const models = document.getElementById("sec-localai-models") as HTMLElement;
    const fold = within(models).getByRole("button", { name: "Show the model" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(within(start()).getByRole("button", { name: "Go to All models" }));
    await waitFor(() => expect(onLocate).toHaveBeenCalledWith("sec-localai-models"));
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    // All models is as it always was: no pick chip, every rung its own Download.
    expect(within(models).queryByText("PM's pick")).toBeNull();
    expect(within(models).getAllByRole("button", { name: /^download$/i })).toHaveLength(2);
  });
});

describe("step 3 — one model for both jobs", () => {
  const ownedServed = recs({
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
    },
  });
  /** The calls the two role wrappers saw, in order. */
  function record() {
    const calls: string[] = [];
    setLocalLlmRoleModel.mockImplementation((r: string, m: string) => {
      calls.push(`model ${r} ${m}`);
      return Promise.resolve();
    });
    setLocalLlmRouting.mockImplementation((r: string, p: string) => {
      calls.push(`routing ${r} ${p}`);
      return Promise.resolve();
    });
    return calls;
  }

  it("with a key, falls each job back to the cloud", async () => {
    const calls = record();
    getLocalLlmConfig.mockResolvedValue(cfg());
    listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
    localModelRecommendations.mockResolvedValue(ownedServed);
    localLlmStatus.mockResolvedValue(status({ effective: "cloud", cloud_key: "present" }));
    await mount();
    const use = await within(start()).findByRole("button", {
      name: "Use Qwen2.5 7B Instruct for both",
    });
    expect(use.getAttribute("data-variant")).toBe("primary");
    fireEvent.click(use);
    await waitFor(() => expect(calls).toHaveLength(4));
    expect(calls).toEqual([
      "model chat qwen2.5:latest",
      "model background qwen2.5:latest",
      "routing chat local-then-cloud",
      "routing background local-then-cloud",
    ]);
  });

  it("without a key, sets them Local only", async () => {
    const calls = record();
    getLocalLlmConfig.mockResolvedValue(cfg());
    listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
    localModelRecommendations.mockResolvedValue(ownedServed);
    localLlmStatus.mockResolvedValue(status({ effective: "nothing", cloud_key: "absent" }));
    await mount();
    fireEvent.click(
      await within(start()).findByRole("button", { name: "Use Qwen2.5 7B Instruct for both" }),
    );
    await waitFor(() => expect(calls).toHaveLength(4));
    expect(calls.slice(2)).toEqual(["routing chat local", "routing background local"]);
  });

  it("leaves a job already routed locally as it is", async () => {
    const calls = record();
    getLocalLlmConfig.mockResolvedValue(cfg({ chat_routing: "local", chat_model: "llama3.2:1b" }));
    listLocalLlmModels.mockResolvedValue(served("llama3.2:1b"));
    localModelRecommendations.mockResolvedValue(recs({ pick: undefined }));
    // The status hasn't caught up with the stored routing yet, so nothing reads as at work.
    localLlmStatus.mockResolvedValue(status({ effective: "nothing", cloud_key: "absent" }));
    await mount();
    fireEvent.click(
      await within(start()).findByRole("button", { name: "Use llama3.2:1b for both" }),
    );
    await waitFor(() => expect(calls).toHaveLength(3));
    expect(calls).toEqual([
      "model chat llama3.2:1b",
      "model background llama3.2:1b",
      "routing background local",
    ]);
  });
});

describe("step 4 — the test", () => {
  const working = () => {
    getLocalLlmConfig.mockResolvedValue(
      cfg({
        chat_model: "qwen2.5:latest",
        background_model: "qwen2.5:latest",
        chat_routing: "local",
        background_routing: "local",
      }),
    );
    listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
    localLlmStatus.mockResolvedValue(status({ effective: "local_only", cloud_key: "absent" }));
  };

  it("is the same test as Assign roles' Test it, and neither can start a second", async () => {
    working();
    testLocalLlm.mockReturnValue(new Promise(() => {}));
    await mount();
    const send = await within(start()).findByRole("button", { name: "Send a test message" });
    // Optional after a restart: nothing nags, so it isn't the primary.
    expect(send.getAttribute("data-variant")).toBe("secondary");
    fireEvent.click(send);
    await waitFor(() => expect(testLocalLlm).toHaveBeenCalledWith("chat"));
    expect(await within(start()).findByText(/Waiting for the reply/)).toBeTruthy();
    expect(within(start()).queryByRole("button", { name: "Send a test message" })).toBeNull();
    const roles = document.getElementById("sec-localai-roles") as HTMLElement;
    for (const b of within(roles).getAllByRole("button", { name: /^(test it|testing…)$/i }))
      expect((b as HTMLButtonElement).disabled).toBe(true);
  });

  it("goes the other way too: Assign roles' Test it takes the step's button away", async () => {
    working();
    testLocalLlm.mockReturnValue(new Promise(() => {}));
    await mount();
    await within(start()).findByRole("button", { name: "Send a test message" });
    const roles = document.getElementById("sec-localai-roles") as HTMLElement;
    fireEvent.click(within(roles).getAllByRole("button", { name: /^test it$/i })[0]);
    await waitFor(() => expect(testLocalLlm).toHaveBeenCalledWith("chat"));
    await waitFor(() =>
      expect(within(start()).queryByRole("button", { name: "Send a test message" })).toBeNull(),
    );
  });

  it("frees the memory with the same call as Release now", async () => {
    working();
    localGpuResidency.mockResolvedValue({
      resident: [{ model: "qwen2.5:latest", size_gb: 7, size_vram_gb: 6.5, pm_loaded: true }],
      vram_gb: 7.96,
      dgpu_displays: [],
      policy: "server",
      idle_minutes: 5,
      no_unload_route: false,
    });
    await mount();
    const free = await within(start()).findByRole("button", { name: "Free it now" });
    expect(start().textContent).toContain(
      "Right now your server is holding qwen2.5:latest — at least 6.5 GB of your 8.0 GB card.",
    );
    fireEvent.click(free);
    await waitFor(() => expect(releaseLocalGpu).toHaveBeenCalledTimes(1));
    const lifecycle = document.getElementById("sec-localai-lifecycle") as HTMLElement;
    await waitFor(() =>
      expect(
        (within(lifecycle).getByRole("button", { name: "Release now" }) as HTMLButtonElement)
          .disabled,
      ).toBe(false),
    );
    fireEvent.click(within(lifecycle).getByRole("button", { name: "Release now" }));
    await waitFor(() => expect(releaseLocalGpu).toHaveBeenCalledTimes(2));
  });

  it("summarises On battery and takes you there", async () => {
    working();
    const onLocate = vi.fn();
    localLlmStatus.mockResolvedValue(
      status({ effective: "local_only", cloud_key: "absent" }, undefined, {
        has_battery: false,
        source: "ac",
      }),
    );
    await mount(onLocate);
    expect(
      await within(start()).findByText(
        /No battery on this computer, so On battery never applies\./,
      ),
    ).toBeTruthy();
    fireEvent.click(within(start()).getByRole("button", { name: "Battery options" }));
    await waitFor(() => expect(onLocate).toHaveBeenCalledWith("sec-localai-power"));
  });
});

describe("the better-fit line, moved into the start card", () => {
  it("says when it is the pick, and shows the pick", async () => {
    const onLocate = vi.fn();
    getLocalLlmConfig.mockResolvedValue(cfg());
    localBetterFitNotice.mockResolvedValue({
      repo: QWEN,
      display_name: "Qwen2.5 7B Instruct",
      replaces: "gemma3:4b",
      already_downloaded: false,
    });
    await mount(onLocate);
    const line = await within(start()).findByText(/would fit your machine better than/);
    expect(line.textContent).toContain("gemma3:4b. It's PM's pick for this computer.");
    fireEvent.click(within(start()).getByRole("button", { name: "Show me" }));
    await waitFor(() => expect(onLocate).toHaveBeenCalledWith("sec-localai-start"));
  });

  it("shows any other model's own card, opening the list first", async () => {
    const onLocate = vi.fn();
    getLocalLlmConfig.mockResolvedValue(cfg());
    localModelRecommendations.mockResolvedValue(recs({ pick: undefined }));
    localBetterFitNotice.mockResolvedValue({
      repo: QWEN,
      display_name: "Qwen2.5 7B Instruct",
      replaces: "gemma3:4b",
      already_downloaded: true,
    });
    await mount(onLocate);
    fireEvent.click(await within(start()).findByRole("button", { name: "Show me" }));
    await waitFor(() =>
      expect(onLocate).toHaveBeenCalledWith("localai-rec-bartowski-qwen2-5-7b-instruct-gguf"),
    );
    expect(document.getElementById("localai-rec-bartowski-qwen2-5-7b-instruct-gguf")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Show the model" }).getAttribute("aria-expanded"),
    ).toBe("true");
  });
});

describe("Depth never hides what justifies the pick", () => {
  it("keeps the memory figures, the quant, context and cache, and the speed's qualifier at min", async () => {
    theme.depth = "min";
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    await within(start()).findByText("PM's pick for this computer");
    const text = start().textContent ?? "";
    expect(text).toContain("6.6 GB of your 8.0 GB graphics card");
    expect(text).toContain("Q5_K_M");
    expect(text).toContain("32k context");
    expect(text).toContain("compressed cache (q8_0)");
    expect(text).toContain("up to 71 tok/s");
    expect(text).toContain(
      "Worked out from your graphics card's published memory speed (384 GB/s)",
    );
    expect(text).toContain("5.1 GB download");
    // The parameter count is detail, and only that.
    expect(text).not.toContain("7.62B parameters");
  });
});

describe("the pick's reason says at which context it fits", () => {
  it("in the pick's own line and in How PM picks", async () => {
    // A larger model that only fits the card with less than PM sizes for is passed over, so "the
    // largest that fits entirely on your graphics card" is only true at that context.
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    await within(start()).findByText("PM's pick for this computer");
    const text = start().textContent ?? "";
    expect(text).toContain(
      "The largest model in PM's list that fits entirely on your graphics card at the context PM sizes it for",
    );
    expect(text).toContain(
      "runs entirely on your graphics card with the room PM keeps free, at the context PM sizes it for — 32k tokens, less for a model made for less, and more for one your server already runs with more — because",
    );
  });
});

describe("every start-card action makes the write its section control makes", () => {
  // `ACTION_MIRRORS` names, for each kind of action, the ipc wrappers it calls and the section
  // control that already makes those calls. Each case presses the start card's button in one render
  // and the control in another; both must make every call the table names, the same calls, and no
  // other write. A kind added to the table without a case here fails to compile.
  const WRITES = {
    setLocalLlmEndpoint,
    setLocalLlmToken,
    pullLocalModel,
    setLocalLlmRoleModel,
    setLocalLlmRouting,
    testLocalLlm,
    releaseLocalGpu,
  };
  const WRAPPERS: Record<string, ReturnType<typeof vi.fn>> = { ...WRITES, probeLocalLlmPorts };

  interface MirrorCase {
    arrange: () => void;
    /** Wait until the tab has settled where the press happens. */
    ready?: () => Promise<void>;
    start: () => Promise<void>;
    control: () => Promise<void>;
    /** What of a call must match, when not all of it can. */
    same?: (call: unknown[]) => unknown[];
  }

  const ownedServed = () =>
    recs({
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
      },
    });
  const working = () => {
    getLocalLlmConfig.mockResolvedValue(
      cfg({
        chat_model: "qwen2.5:latest",
        background_model: "qwen2.5:latest",
        chat_routing: "local",
        background_routing: "local",
      }),
    );
    listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
    localLlmStatus.mockResolvedValue(status({ effective: "local_only", cloud_key: "absent" }));
  };
  const lookedOnce = async () => {
    await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1));
    await within(start()).findByRole("button", { name: "Look now" });
  };

  const CASES: Record<StepAction["kind"], MirrorCase> = {
    connect: {
      arrange: () => probeLocalLlmPorts.mockResolvedValue([OLLAMA]),
      start: async () => {
        fireEvent.click(await within(start()).findByRole("button", { name: "Connect to Ollama" }));
        await waitFor(() => expect(setLocalLlmEndpoint).toHaveBeenCalled());
      },
      control: async () => {
        const endpoint = section("sec-localai-endpoint");
        fireEvent.click(
          await within(endpoint).findByRole("button", { name: `Ollama — ${OLLAMA.url}` }),
        );
        fireEvent.click(within(endpoint).getByRole("button", { name: "Connect" }));
        await waitFor(() => expect(setLocalLlmEndpoint).toHaveBeenCalled());
      },
    },
    detect: {
      arrange: () => {},
      ready: lookedOnce,
      start: async () => {
        fireEvent.click(within(start()).getByRole("button", { name: "Look now" }));
        await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(2));
      },
      control: async () => {
        fireEvent.click(
          within(section("sec-localai-endpoint")).getByRole("button", {
            name: "Auto-detect a local server",
          }),
        );
        await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(2));
      },
    },
    download: {
      arrange: () => getLocalLlmConfig.mockResolvedValue(cfg()),
      ready: async () => {
        await within(start()).findByRole("button", { name: "Download (5.1 GB)" });
      },
      start: async () => {
        fireEvent.click(within(start()).getByRole("button", { name: "Download (5.1 GB)" }));
        await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());
      },
      control: async () => {
        fireEvent.click(
          within(section("sec-localai-models")).getAllByRole("button", { name: /^download$/i })[0],
        );
        await waitFor(() => expect(pullLocalModel).toHaveBeenCalled());
      },
      // One Download per tag: the start card's is the pick's rung and All models' the model's other
      // one, so they fetch two files of one model with the same call.
      same: (call) => [String(call[0]).startsWith(`hf.co/${QWEN}:`)],
    },
    assign: {
      arrange: () => {
        getLocalLlmConfig.mockResolvedValue(cfg());
        listLocalLlmModels.mockResolvedValue(served("qwen2.5:latest"));
        localModelRecommendations.mockResolvedValue(ownedServed());
        localLlmStatus.mockResolvedValue(status({ effective: "cloud", cloud_key: "present" }));
      },
      ready: async () => {
        await within(start()).findByRole("button", { name: "Use Qwen2.5 7B Instruct for both" });
      },
      start: async () => {
        fireEvent.click(
          within(start()).getByRole("button", { name: "Use Qwen2.5 7B Instruct for both" }),
        );
        await waitFor(() => expect(setLocalLlmRouting).toHaveBeenCalledTimes(2));
      },
      control: async () => {
        const roles = section("sec-localai-roles");
        const set = (name: string, value: string) =>
          fireEvent.change(within(roles).getByRole("combobox", { name }), { target: { value } });
        set("Chat model", "qwen2.5:latest");
        set("Background work model", "qwen2.5:latest");
        set("Where chat runs", "local-then-cloud");
        set("Where background work runs", "local-then-cloud");
        await waitFor(() => expect(setLocalLlmRouting).toHaveBeenCalledTimes(2));
      },
    },
    test: {
      arrange: () => {
        working();
        testLocalLlm.mockReturnValue(new Promise(() => {}));
      },
      ready: async () => {
        await within(start()).findByRole("button", { name: "Send a test message" });
      },
      start: async () => {
        fireEvent.click(within(start()).getByRole("button", { name: "Send a test message" }));
        await waitFor(() => expect(testLocalLlm).toHaveBeenCalled());
      },
      control: async () => {
        const roles = section("sec-localai-roles");
        fireEvent.click(within(roles).getAllByRole("button", { name: /^test it$/i })[0]);
        await waitFor(() => expect(testLocalLlm).toHaveBeenCalled());
      },
    },
    release: {
      arrange: () => {
        working();
        localGpuResidency.mockResolvedValue({
          resident: [{ model: "qwen2.5:latest", size_gb: 7, size_vram_gb: 6.5, pm_loaded: true }],
          vram_gb: 7.96,
          dgpu_displays: [],
          policy: "server",
          idle_minutes: 5,
          no_unload_route: false,
        });
      },
      ready: async () => {
        await within(start()).findByRole("button", { name: "Free it now" });
        const lifecycle = section("sec-localai-lifecycle");
        await waitFor(() =>
          expect(
            (within(lifecycle).getByRole("button", { name: "Release now" }) as HTMLButtonElement)
              .disabled,
          ).toBe(false),
        );
      },
      start: async () => {
        fireEvent.click(within(start()).getByRole("button", { name: "Free it now" }));
        await waitFor(() => expect(releaseLocalGpu).toHaveBeenCalled());
      },
      control: async () => {
        fireEvent.click(
          within(section("sec-localai-lifecycle")).getByRole("button", { name: "Release now" }),
        );
        await waitFor(() => expect(releaseLocalGpu).toHaveBeenCalled());
      },
    },
    locate: {
      arrange: () => {
        getLocalLlmConfig.mockResolvedValue(cfg());
        localModelRecommendations.mockResolvedValue(recs({ pick: undefined }));
      },
      ready: async () => {
        await within(start()).findByRole("button", { name: "Go to All models" });
      },
      start: async () => {
        fireEvent.click(within(start()).getByRole("button", { name: "Go to All models" }));
        await waitFor(() =>
          expect(
            screen.getByRole("button", { name: "Show the model" }).getAttribute("aria-expanded"),
          ).toBe("true"),
        );
      },
      control: async () => {},
    },
  };

  /** Render the tab as `c` arranges it, press with `press`, and return the calls each wrapper saw
   *  during the press — functions (a progress callback) as a placeholder, since each render has
   *  its own. */
  async function pressed(c: MirrorCase, press: () => Promise<void>) {
    vi.clearAllMocks();
    c.arrange();
    await mount();
    await c.ready?.();
    const before = Object.fromEntries(
      Object.entries(WRAPPERS).map(([n, f]) => [n, f.mock.calls.length]),
    );
    await press();
    const calls = Object.fromEntries(
      Object.entries(WRAPPERS).map(([n, f]) => [
        n,
        f.mock.calls
          .slice(before[n])
          .map((call) =>
            c.same ? c.same(call) : call.map((a) => (typeof a === "function" ? "fn" : a)),
          ),
      ]),
    );
    cleanup();
    return calls;
  }

  for (const [kind, c] of Object.entries(CASES) as [StepAction["kind"], MirrorCase][]) {
    it(`${kind}: ${ACTION_MIRRORS[kind].control ?? "writes nothing"}`, async () => {
      const mirror = ACTION_MIRRORS[kind];
      const fromStart = await pressed(c, c.start);
      for (const n of mirror.ipc) expect(fromStart[n]?.length, n).toBeGreaterThan(0);
      for (const n of Object.keys(WRITES))
        if (!mirror.ipc.includes(n)) expect(fromStart[n], `${n} from the start card`).toEqual([]);
      if (mirror.control === null) return;
      const fromControl = await pressed(c, c.control);
      expect(fromControl).toEqual(fromStart);
    });
  }
});

describe("the rendered card keeps the tab's copy rules", () => {
  // readiness.test.ts and pickWords.test.ts sweep the words those modules make. This sweeps the card
  // as rendered — what it adds itself (the better-fit line, what the server is holding, "How PM
  // picks") and the server guide it shows — against the same two lists.
  const states: Array<[string, () => void]> = [
    ["fresh", () => {}],
    ["detected", () => probeLocalLlmPorts.mockResolvedValue([OLLAMA])],
    [
      "connected, a better fit, PM's model loaded",
      () => {
        getLocalLlmConfig.mockResolvedValue(
          cfg({
            chat_model: "gemma3:4b",
            background_model: "gemma3:4b",
            chat_routing: "local-then-cloud",
            background_routing: "local-then-cloud",
          }),
        );
        listLocalLlmModels.mockResolvedValue(served("gemma3:4b"));
        localLlmStatus.mockResolvedValue(status({ effective: "local_then_cloud" }));
        localBetterFitNotice.mockResolvedValue({
          repo: QWEN,
          display_name: "Qwen2.5 7B Instruct",
          replaces: "gemma3:4b",
          already_downloaded: false,
        });
        localGpuResidency.mockResolvedValue({
          resident: [{ model: "gemma3:4b", size_gb: 4, size_vram_gb: 3.5, pm_loaded: true }],
          vram_gb: 7.96,
          dgpu_displays: [],
          policy: "server",
          idle_minutes: 5,
          no_unload_route: false,
        });
      },
    ],
    [
      "connected, someone else's model loaded",
      () => {
        getLocalLlmConfig.mockResolvedValue(
          cfg({
            chat_model: "gemma3:4b",
            background_model: "gemma3:4b",
            chat_routing: "local",
            background_routing: "local",
          }),
        );
        listLocalLlmModels.mockResolvedValue(served("gemma3:4b"));
        localLlmStatus.mockResolvedValue(status({ effective: "local_only", cloud_key: "absent" }));
        localGpuResidency.mockResolvedValue({
          resident: [{ model: "gemma3:4b", size_gb: 4, size_vram_gb: 3.5, pm_loaded: false }],
          vram_gb: 7.96,
          dgpu_displays: [],
          policy: "server",
          idle_minutes: 5,
          no_unload_route: false,
        });
      },
    ],
  ];
  for (const [name, arrange] of states) {
    it(name, async () => {
      arrange();
      await mount();
      await waitFor(() => expect(start().querySelectorAll("li").length).toBeGreaterThan(0));
      await within(start()).findAllByText(/PM's pick for this computer/);
      const text = start().textContent ?? "";
      for (const re of COPY_COLLISIONS) expect(text, `${re}`).not.toMatch(re);
      const stripped = SAME_SECTION_POINTERS.reduce(
        (t, re) => t.replace(new RegExp(re.source, `${re.flags}g`), ""),
        text,
      );
      expect(stripped).not.toMatch(/\b(above|below)\b/i);
    });
  }
});

describe("the adopted download's error", () => {
  it("lands where its progress was shown: the start card for the pick", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    activeLocalPull
      .mockResolvedValueOnce({
        model: QWEN_TAG,
        status: "downloading",
        completed_bytes: 1,
        total_bytes: 4,
        running: true,
        error: null,
        started_at_ms: 0,
      })
      .mockResolvedValue({
        model: QWEN_TAG,
        status: "error",
        completed_bytes: 1,
        total_bytes: 4,
        running: false,
        error: "disk full",
        started_at_ms: 0,
      });
    await mount();
    const error = await screen.findByText(/disk full/, undefined, { timeout: 3000 });
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-start");
  });

  it("and on its card for any other model", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    const other = `hf.co/${QWEN}:Q8_0`;
    activeLocalPull
      .mockResolvedValueOnce({
        model: other,
        status: "downloading",
        completed_bytes: 1,
        total_bytes: 4,
        running: true,
        error: null,
        started_at_ms: 0,
      })
      .mockResolvedValue({
        model: other,
        status: "error",
        completed_bytes: 1,
        total_bytes: 4,
        running: false,
        error: "disk full",
        started_at_ms: 0,
      });
    await mount();
    const error = await screen.findByText(/disk full/, undefined, { timeout: 3000 });
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-models");
  });
});

describe("no start-card button shares a section control's name", () => {
  // Two buttons with one name are two controls a screen reader can't tell apart — and a test that
  // finds a section's control by name would find two.
  const states: Array<[string, () => void]> = [
    ["fresh", () => {}],
    ["detected", () => probeLocalLlmPorts.mockResolvedValue([OLLAMA])],
    ["connected, empty", () => getLocalLlmConfig.mockResolvedValue(cfg())],
    [
      "two models that swap",
      () => {
        getLocalLlmConfig.mockResolvedValue(
          cfg({
            chat_model: "a:14b",
            background_model: "b:7b",
            chat_routing: "local",
            background_routing: "local",
          }),
        );
        listLocalLlmModels.mockResolvedValue(served("a:14b", "b:7b"));
        localLlmStatus.mockResolvedValue(status({ effective: "local_only", cloud_key: "absent" }));
        localModelRecommendations.mockResolvedValue(
          recs({
            co_residency: {
              ram: "fits",
              vram: "exceeds",
              combined_gb: 14,
              ram_budget_gb: 30,
              vram_budget_gb: 7,
            },
          }),
        );
      },
    ],
  ];
  for (const [name, arrange] of states) {
    it(name, async () => {
      arrange();
      const { container } = await mount();
      await waitFor(() => expect(start().querySelectorAll("li").length).toBeGreaterThan(0));
      const inStart = new Set(
        Array.from(start().querySelectorAll("button")).map((b) => b.textContent?.trim() ?? ""),
      );
      const elsewhere = Array.from(container.querySelectorAll("button"))
        .filter((b) => !start().contains(b))
        .map((b) => b.textContent?.trim() ?? "");
      for (const n of elsewhere) expect(inStart.has(n), `"${n}" is in both`).toBe(false);
    });
  }
});
