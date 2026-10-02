// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Assign roles says where each job's requests REALLY go — from the backend's own route
// (`effective`), never guessed from the routing alone. The trap these pin is the one every earlier
// design fell into: a role set to Local with no model was said to "use the cloud", when the gateway
// gives it nothing to answer with; and "on this computer" was said of a server on someone's LAN.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type {
  EffectiveRoute,
  LocalCoResidency,
  LocalLlmConfig,
  LocalLlmStatus,
  LocalServedModel,
} from "../../lib/types";

const setLocalLlmRoleModel = vi.fn();
const setLocalLlmRouting = vi.fn();

vi.mock("../../lib/ipc", () => ({
  setLocalLlmRoleModel: (...a: unknown[]) => setLocalLlmRoleModel(...a),
  setLocalLlmRouting: (...a: unknown[]) => setLocalLlmRouting(...a),
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({ depth: "standard" }),
}));

import { LocalAiRoles } from "./LocalAiRoles";
import { sectionLabel } from "./sections";

const cfg = (over: Partial<LocalLlmConfig> = {}): LocalLlmConfig => ({
  base_url: "http://127.0.0.1:11434",
  chat_model: "tiny-chat:1b",
  background_model: null,
  chat_routing: "local-then-cloud",
  background_routing: "cloud",
  has_token: false,
  ...over,
});

const status = (
  chat: EffectiveRoute,
  background: EffectiveRoute = "cloud",
  over: Partial<LocalLlmStatus> = {},
): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: false,
  chat_local_model: "tiny-chat:1b",
  background_local_model: null,
  served_window: 32768,
  served_window_proven: true,
  window_source: "slots",
  chat_answering: false,
  background_answering: false,
  chat_loaded: null,
  background_loaded: null,
  chat_released: false,
  background_released: false,
  power: {
    ...INERT_POWER_VIEW,
    chat: { ...INERT_POWER_VIEW.chat, effective: chat },
    background: { ...INERT_POWER_VIEW.background, effective: background },
  },
  ...over,
});

const SERVED: LocalServedModel[] = [
  { id: "tiny-chat:1b", embedding: false },
  { id: "other-chat:3b", embedding: false },
];

function show({
  config = cfg(),
  st = status("local_then_cloud") as LocalLlmStatus | null,
  served = SERVED,
  coResidency = null as LocalCoResidency | null,
  configured = true,
} = {}) {
  return render(
    <LocalAiRoles
      config={config}
      status={st}
      served={served}
      servedLoaded
      configured={configured}
      coResidency={coResidency}
      anyLocalRoleWithModel
      roleTests={{ testing: null, tests: {}, runTest: vi.fn(), clearTest: vi.fn() }}
      onConfigPatch={vi.fn()}
      onError={vi.fn()}
    />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  setLocalLlmRoleModel.mockResolvedValue(undefined);
  setLocalLlmRouting.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("each role's row", () => {
  it("asks where the job runs first, then which model, and names both controls", () => {
    show();
    const names = screen.getAllByRole("combobox").map((el) => el.getAttribute("aria-label") ?? "");
    expect(names).toEqual([
      "Where chat runs",
      "Chat model",
      "Where background work runs",
      "Background work model",
    ]);
    expect(screen.getByText("Background work")).toBeTruthy();
  });

  it("offers no local model, never 'use cloud'", () => {
    // With no model the job goes wherever its routing and keys send it — which can be nowhere.
    show();
    expect(screen.getAllByRole("option", { name: "— no local model —" })).toHaveLength(2);
    expect(screen.queryByRole("option", { name: /use cloud/ })).toBeNull();
  });

  it("says who each routing suits, for the one chosen", () => {
    show();
    expect(
      screen.getByText(
        "Good for most setups with a cloud key: your own model when it can, your cloud model when your server can't — it isn't reachable, a reply fails or times out, or a request is too long for the window it gives the model. It's the only setting On battery can move.",
      ),
    ).toBeTruthy();
    expect(
      screen.getByText(
        "Good if this computer is slow, or you'd rather not keep a model loaded. What you send goes to OpenRouter.",
      ),
    ).toBeTruthy();
    cleanup();
    show({ config: cfg({ chat_routing: "local" }) });
    expect(screen.getByText(/^Good if nothing should ever go to the cloud\./)).toBeTruthy();
  });
});

describe("the line that says where a job really goes", () => {
  const line = (
    chat: EffectiveRoute,
    config: LocalLlmConfig = cfg(),
    served: LocalServedModel[] = SERVED,
  ) => {
    const { container } = show({ config, st: status(chat), served });
    const text = container.querySelector("p.text-ink3")?.textContent ?? null;
    cleanup();
    return text;
  };

  it("says 'on this computer' only for an address on this computer", () => {
    expect(line("local_only", cfg({ chat_routing: "local" }))).toBe(
      "Runs on tiny-chat:1b on this computer, and never uses the cloud.",
    );
    expect(line("local_then_cloud", cfg({ base_url: "http://192.168.1.20:11434" }))).toBe(
      "Runs on tiny-chat:1b on your model server; your cloud model answers when your server can't — it isn't reachable, a reply fails or times out, or a request is too long for the window it gives the model.",
    );
  });

  it("words every other route from the backend's answer", () => {
    expect(line("cloud", cfg({ chat_routing: "cloud" }))).toBe("Uses your cloud model.");
    expect(line("cloud_for_power")).toBe(
      "On battery, so this is on your cloud model for now; tiny-chat:1b is waiting.",
    );
    expect(line("unknown")).toBe(
      "PM can't read your saved keys right now, so it can't say where this goes.",
    );
  });

  it("never says a job with nothing to answer with uses the cloud", () => {
    expect(line("nothing", cfg({ chat_routing: "cloud" }))).toBe(
      "There's no cloud key, so this job has nothing to answer with.",
    );
    expect(line("nothing", cfg({ chat_routing: "local", chat_model: null }))).toBe(
      "No local model chosen, so this job has nothing to answer with.",
    );
  });

  it("adds when the server isn't serving the model the job is set to", () => {
    expect(line("local_then_cloud", cfg(), [{ id: "other-chat:3b", embedding: false }])).toBe(
      "Runs on tiny-chat:1b on this computer; your cloud model answers when your server can't — it isn't reachable, a reply fails or times out, or a request is too long for the window it gives the model. Your server isn't serving tiny-chat:1b right now.",
    );
  });

  it("says nothing until the status has been read", () => {
    const { container } = show({ st: null });
    expect(container.querySelector("p.text-ink3")).toBeNull();
  });

  it("never says the cloud answers only when the server is down", () => {
    // The gateway also falls back on a running server: a reply that fails or times out, and a
    // request longer than the window the server gives the model (a long chat on Ollama's default
    // 4k). "Only if your server fails" told someone a long chat stayed on their computer.
    show({ config: cfg({ chat_routing: "local-then-cloud" }) });
    const text = document.body.textContent ?? "";
    expect(text).not.toMatch(/only if your server|when your server is down/);
    expect(text).toMatch(/a request is too long for the window it gives the model/);
  });
});

describe("a server serving nothing yet", () => {
  // With no model, a job goes wherever its routing and keys send it: a keyed Cloud job to the cloud,
  // a Local only or keyless one nowhere. The empty-server hint sat beside the row's own line saying
  // "nothing to answer with" while it claimed "both roles stay on cloud".
  it("names no destination of its own, and leaves that to each job's line", () => {
    show({
      config: cfg({ chat_routing: "local", chat_model: null }),
      st: status("nothing"),
      served: [],
    });
    const hint = screen.getByText(/isn't serving any models yet/);
    expect(hint.textContent).not.toMatch(/cloud/);
    expect(hint.textContent).toMatch(/the line under each job says where it goes meanwhile/);
    expect(
      screen.getByText("No local model chosen, so this job has nothing to answer with."),
    ).toBeTruthy();
  });
});

describe("two models that won't stay loaded together", () => {
  const co = (over: Partial<LocalCoResidency>): LocalCoResidency => ({
    ram: "exceeds",
    vram: null,
    combined_gb: 18,
    ram_budget_gb: 14,
    vram_budget_gb: null,
    ...over,
  });

  it("offers chat's model for both, and writes background work's model only", () => {
    show({
      config: cfg({ background_model: "other-chat:3b", background_routing: "local" }),
      coResidency: co({}),
    });
    expect(screen.getByText(/^Chat and background work use different models/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Use tiny-chat:1b for both" }));
    expect(setLocalLlmRoleModel).toHaveBeenCalledTimes(1);
    expect(setLocalLlmRoleModel).toHaveBeenCalledWith("background", "tiny-chat:1b");
    expect(
      screen.getByText("Same model for both jobs: it loads once and never has to swap."),
    ).toBeTruthy();
  });

  it("offers it when the pair is too close to call, and not when PM couldn't size them", () => {
    show({ coResidency: co({ ram: "too_close", combined_gb: 15 }) });
    expect(screen.getByRole("button", { name: "Use tiny-chat:1b for both" })).toBeTruthy();
    cleanup();
    show({ coResidency: co({ ram: "unknown", combined_gb: null }) });
    expect(screen.queryByRole("button", { name: /for both/ })).toBeNull();
  });
});

describe("before a server is connected", () => {
  it("names where to connect it, not a direction", () => {
    const { container } = show({ configured: false });
    expect(container.textContent).toContain(
      `Connect your model server first, under ${sectionLabel("sec-localai-endpoint")}. Then choose which model answers chat and which does background work.`,
    );
    expect(container.textContent).not.toMatch(/\b(above|below)\b/);
  });
});
