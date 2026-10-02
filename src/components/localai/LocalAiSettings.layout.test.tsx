// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The tab's shape, across the states a user actually passes through: the sections in the rail's
// order with the rail's names as their headings, every section's help pointing at an entry that
// exists, and never more than one primary button on the page — the step the user is on. These are
// the facts three lists (the rail, the headings, the help registry) used to have to keep agreeing
// on by hand; now they come from one, and this pins that the page renders that one.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { HELP } from "../../lib/help";
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
import { sectionsFor } from "../settings/registry";

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
/** The push signal's listener, so a test can deliver a status the way the backend does. */
const push = vi.hoisted(() => ({ listener: null as null | (() => void) }));

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
  onLocalLlmStatus: (listener: () => void) => {
    push.listener = listener;
    return Promise.resolve(() => {});
  },
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
import { LOCALAI_SECTIONS } from "./sections";

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

const gemmaRec = (): LocalRecommendation =>
  qwenRec({
    repo: "bartowski/gemma-2-2b-it-GGUF",
    display_name: "gemma 2 2b it",
    parameters_b: 2.61,
    active_parameters_b: 2.61,
    ollama_pull: "hf.co/bartowski/gemma-2-2b-it-GGUF:Q4_K_M",
    gpu_pull: null,
    gpu: { kind: "single" },
    fit: fit({ quant: "Q4_K_M", context: 8192, kv: "f16", est_memory_gb: 2.4 }),
    licence: {
      id: "gemma",
      name: "Gemma Terms of Use",
      url: "https://ai.google.dev/gemma/terms",
      open: false,
      summary: "Google's own terms, not an open-source licence.",
    },
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
  endpoint_configured: true,
  cadence: "on-catalog-update",
  rescan_due: false,
  curated: [qwenRec(), gemmaRec()],
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
  over: Partial<LocalLlmStatus> = {},
  r: Partial<PowerRoleView> = {},
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
  power: { ...INERT_POWER_VIEW, chat: role(r), background: role(r), ...power },
  ...over,
});

const served = (...ids: string[]): LocalServedModel[] =>
  ids.map((id) => ({ id, embedding: false }));

/** Both jobs on the pick, running on it — someone already set up. */
function returning() {
  getLocalLlmConfig.mockResolvedValue(
    cfg({
      chat_model: QWEN_TAG,
      background_model: QWEN_TAG,
      chat_routing: "local-then-cloud",
      background_routing: "local-then-cloud",
    }),
  );
  listLocalLlmModels.mockResolvedValue(served(QWEN_TAG));
  localLlmStatus.mockResolvedValue(status({}, { effective: "local_then_cloud" }));
}

const FIXTURES: Array<[string, () => void]> = [
  ["fresh", () => {}],
  ["detected", () => probeLocalLlmPorts.mockResolvedValue([OLLAMA])],
  ["connected, empty", () => getLocalLlmConfig.mockResolvedValue(cfg())],
  [
    "served, unassigned",
    () => {
      getLocalLlmConfig.mockResolvedValue(cfg());
      listLocalLlmModels.mockResolvedValue(served(QWEN_TAG));
    },
  ],
  ["returning", returning],
  [
    "unreachable",
    () => {
      getLocalLlmConfig.mockResolvedValue(cfg());
      localLlmStatus.mockResolvedValue(status({ reachable: false }));
    },
  ],
];

beforeEach(() => {
  vi.clearAllMocks();
  push.listener = null;
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
  releaseLocalGpu.mockResolvedValue(0);
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
  clearLocalLlmEndpoint.mockResolvedValue(undefined);
  probeLocalLlmPorts.mockResolvedValue([]);
});

/** Render the tab and wait until the start card has read the setup and the pick. */
async function mount() {
  const view = render(<LocalAiSettings />);
  const start = () => document.getElementById("sec-localai-start") as HTMLElement;
  await waitFor(() => expect(start().textContent).not.toMatch(/Reading your local AI setup/));
  await waitFor(() => expect(start().textContent).not.toMatch(/Sizing PM's models/));
  await waitFor(() => expect(start().textContent).not.toMatch(/Checking your model server/));
  await waitFor(() => expect(start().textContent).not.toMatch(/Checking what your server has/));
  return view;
}

/** The checks every state must pass. */
function expectShape(container: HTMLElement) {
  const sections = Array.from(container.querySelectorAll<HTMLElement>("[data-settings-section]"));
  // DOM order = registry order = scroll-spy order.
  expect(sections.map((s) => s.id)).toEqual(sectionsFor("localai").map((s) => s.id));
  for (const [k, s] of sections.entries()) {
    const row = LOCALAI_SECTIONS[k];
    // Each heading is the rail's label, word for word.
    expect(s.querySelector("h2")?.textContent).toBe(sectionsFor("localai")[k].label);
    // Each wrapper's help is its own row's, and the entry exists — HelpOverlay renders nothing for
    // an id it can't find.
    expect(s.getAttribute("data-help")).toBe(row.help);
    expect(HELP[row.help], row.help).toBeDefined();
    expect(HELP[row.help].title).toBe(row.label);
  }
}

describe("the tab's shape, in every state", () => {
  for (const [name, arrange] of FIXTURES) {
    it(`${name}: the rail's sections, names and help, and at most one primary`, async () => {
      arrange();
      const { container } = await mount();
      expectShape(container);
      expect(container.querySelectorAll('[data-variant="primary"]').length).toBeLessThanOrEqual(1);
      // The pane's first-child rule (no top rule, no top margin) lands on the start section.
      expect((container.firstElementChild as HTMLElement).id).toBe("sec-localai-start");
    });
  }

  it("consent needed: the one primary is the question's own", async () => {
    returning();
    localLlmStatus.mockResolvedValue(
      status(
        {},
        { effective: "local_then_cloud", route: "needs_consent" },
        {
          has_battery: true,
          source: "battery",
          state: "battery_low",
          percent: 20,
          consent_needed: true,
          any_cloud_key: true,
        },
      ),
    );
    const { container } = await mount();
    await waitFor(() =>
      expect(container.querySelectorAll('[data-variant="primary"]')).toHaveLength(1),
    );
    const primary = container.querySelector('[data-variant="primary"]') as HTMLElement;
    expect(primary.closest("[data-settings-section]")?.id).toBe("sec-localai-power");
    expectShape(container);
  });
});

describe("Model server", () => {
  it("puts a failed Connect under its own form", async () => {
    setLocalLlmEndpoint.mockRejectedValue(new Error("refusing a public cleartext address"));
    await mount();
    fireEvent.change(screen.getByLabelText("Endpoint URL"), {
      target: { value: "http://203.0.113.9:11434" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^connect$/i }));
    const error = await screen.findByText(/refusing a public cleartext address/);
    expect(error.closest("[data-settings-section]")?.id).toBe("sec-localai-endpoint");
  });

  it("asks before disconnecting, and Cancel calls nothing", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    fireEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    expect(
      await screen.findByRole("dialog", { name: "Disconnect from your model server?" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(clearLocalLlmEndpoint).not.toHaveBeenCalled();
  });
});

describe("Model memory", () => {
  it("reads what the server holds again when the status says it changed", async () => {
    returning();
    await mount();
    await waitFor(() => expect(push.listener).not.toBeNull());
    const before = localGpuResidency.mock.calls.length;
    // A test loads the model: the next status says so, and the readout must not go on claiming the
    // card is free.
    localLlmStatus.mockResolvedValue(
      status({ chat_loaded: true, background_loaded: true }, { effective: "local_then_cloud" }),
    );
    push.listener?.();
    await waitFor(() => expect(localGpuResidency.mock.calls.length).toBeGreaterThan(before));
  });
});

describe("All models", () => {
  const models = () => document.getElementById("sec-localai-models") as HTMLElement;

  it("is folded until asked for", async () => {
    await mount();
    expect(
      within(models())
        .getByRole("button", { name: "Show all 2 models" })
        .getAttribute("aria-expanded"),
    ).toBe("false");
    // Nothing on it is held in a scroller of its own.
    expect(models().querySelector(".overflow-y-auto")).toBeNull();
  });

  it("marks the pick's rung while the start card offers it, and keeps the other rung's Download", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    await mount();
    const card = document.getElementById(
      "localai-rec-bartowski-qwen2-5-7b-instruct-gguf",
    ) as HTMLElement;
    expect(within(card).getByText("PM's pick")).toBeTruthy();
    expect(within(card).getByRole("button", { name: "Show it" })).toBeTruthy();
    expect(within(card).getAllByRole("button", { name: /^download$/i })).toHaveLength(1);
  });

  it("keeps every rung's own action when the start card isn't offering the download", async () => {
    // LM Studio: PM can't download at all, so there is no Download to hand over.
    getLocalLlmConfig.mockResolvedValue(cfg({ base_url: "http://127.0.0.1:1234" }));
    await mount();
    expect(within(models()).queryByText("PM's pick")).toBeNull();
    expect(models().textContent).toContain(
      "PM can only download into an Ollama on its usual port (11434), and your server is LM Studio, so each model's “How to get it” has the steps instead.",
    );
  });

  it("tells another build of a model from the card's own file, and still offers the Download", async () => {
    getLocalLlmConfig.mockResolvedValue(cfg());
    listLocalLlmModels.mockResolvedValue(served("gemma2:2b"));
    localModelRecommendations.mockResolvedValue(
      recs({
        pick: undefined,
        installed: [{ id: "gemma2:2b", matched_repo: "bartowski/gemma-2-2b-it-GGUF", fit: fit() }],
      }),
    );
    await mount();
    const card = document.getElementById("localai-rec-bartowski-gemma-2-2b-it-gguf") as HTMLElement;
    expect(card.textContent).toContain("Your server has gemma2:2b, another build of this model.");
    expect(within(card).queryByText("Installed")).toBeNull();
    expect(within(card).getByRole("button", { name: /^download$/i })).toBeTruthy();
  });

  it("labels a restricted licence in words, not in the warning colour", async () => {
    await mount();
    const card = document.getElementById("localai-rec-bartowski-gemma-2-2b-it-gguf") as HTMLElement;
    const link = within(card).getByRole("link", { name: "Gemma Terms of Use" });
    expect(link.className).not.toContain("text-st-due");
    expect(link.parentElement?.textContent).toContain("Gemma Terms of Use · its own terms");
  });
});
