// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lifecycle section makes claims about somebody's hardware, so the states it must NOT confuse are
// what these pin: "PM couldn't ask" is not "nothing is loaded", a model PM didn't load is not PM's to
// free, and a server with no unload route must say so rather than offer options that do nothing.

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalGpuResidency } from "../../lib/types";

const localGpuResidency = vi.fn();
const releaseLocalGpu = vi.fn();
const getLocalReleasePolicy = vi.fn();
const setLocalReleasePolicy = vi.fn();
const getTrayEnabled = vi.fn();
const setTrayEnabled = vi.fn();

vi.mock("../../lib/ipc", () => ({
  localGpuResidency: () => localGpuResidency(),
  releaseLocalGpu: () => releaseLocalGpu(),
  getLocalReleasePolicy: () => getLocalReleasePolicy(),
  setLocalReleasePolicy: (...a: unknown[]) => setLocalReleasePolicy(...a),
  getTrayEnabled: () => getTrayEnabled(),
  setTrayEnabled: (...a: unknown[]) => setTrayEnabled(...a),
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({ depth: "standard" }),
}));

import { LocalAiLifecycle } from "./LocalAiLifecycle";

const residency = (over: Partial<LocalGpuResidency> = {}): LocalGpuResidency => ({
  resident: [],
  vram_gb: 8,
  dgpu_displays: [],
  policy: "server",
  idle_minutes: 5,
  no_unload_route: false,
  ...over,
});

const model = (over = {}) => ({
  model: "gemma3:4b",
  size_gb: 2.9,
  size_vram_gb: 2.9,
  pm_loaded: true,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  localGpuResidency.mockResolvedValue(residency());
  releaseLocalGpu.mockResolvedValue(0);
  getLocalReleasePolicy.mockResolvedValue({ policy: "server", idle_minutes: 5 });
  setLocalReleasePolicy.mockResolvedValue(undefined);
  getTrayEnabled.mockResolvedValue(false);
  setTrayEnabled.mockResolvedValue(undefined);
});
afterEach(cleanup);

const loaded = async (over: Partial<LocalGpuResidency> = {}) => {
  localGpuResidency.mockResolvedValue(residency(over));
  const view = render(<LocalAiLifecycle configured />);
  await waitFor(() => expect(localGpuResidency).toHaveBeenCalled());
  return view;
};

describe("LocalAiLifecycle", () => {
  it("never reads 'couldn't ask' as 'nothing is loaded'", async () => {
    // The two are opposite facts about someone's graphics card, and `null` vs `[]` is the only thing
    // separating them on the wire. Collapsing them would tell a user their card was free while a
    // model sat in it.
    await loaded({ resident: null });
    expect(await screen.findByText(/couldn't ask your server/i)).toBeTruthy();
    expect(screen.queryByText(/the graphics card is free/i)).toBeNull();

    cleanup();
    await loaded({ resident: [] });
    expect(await screen.findByText(/the graphics card is free/i)).toBeTruthy();
  });

  it("says a model PM didn't load is not PM's to free, and won't offer to", async () => {
    const { container } = await loaded({ resident: [model({ pm_loaded: false })] });
    expect(await screen.findByText(/that one is yours to manage/i)).toBeTruthy();
    const release = Array.from(container.querySelectorAll("button")).find((b) =>
      /release now/i.test(b.textContent ?? ""),
    );
    expect(release?.hasAttribute("disabled")).toBe(true);
  });

  it("offers to release what PM did load", async () => {
    const { container } = await loaded({ resident: [model()] });
    expect(await screen.findByText(/PM loaded it, so PM can hand it back/i)).toBeTruthy();
    const release = Array.from(container.querySelectorAll("button")).find((b) =>
      /release now/i.test(b.textContent ?? ""),
    );
    expect(release?.hasAttribute("disabled")).toBe(false);
  });

  it("hedges the card figure rather than presenting a floor as a measurement", async () => {
    // `size_vram` excludes the runtime's own context and compute buffers — measured 1.25 GB low on a
    // real load. Rendering it as "your card is holding this" would be a confident wrong number.
    await loaded({ resident: [model()] });
    expect(await screen.findByText(/at least/i)).toBeTruthy();
    expect(screen.getByText(/somewhat more/i)).toBeTruthy();
  });

  it("says plainly when the server cannot release at all", async () => {
    // llama-server and LM Studio have no unload gesture. Offering a picker that silently does
    // nothing would be worse than not having the feature.
    await loaded({ no_unload_route: true });
    expect(await screen.findByText(/no way to unload a model on request/i)).toBeTruthy();
  });

  it("mentions an external display on the card, and promises to do nothing about it", async () => {
    // Surfaced, never acted on: plugging in a screen usually means more work is coming, not less.
    await loaded({ dgpu_displays: ["HDMI-A-1"] });
    const line = await screen.findByText(/external display/i);
    expect(line.textContent).toMatch(/HDMI-A-1/);
    expect(line.textContent).toMatch(/won.t change anything/i);
  });

  it("stays quiet about all of it until an endpoint is connected", async () => {
    render(<LocalAiLifecycle configured={false} />);
    expect(await screen.findByText(/Connect an endpoint above/i)).toBeTruthy();
    expect(screen.queryByText(/Release now/i)).toBeNull();
  });
});
