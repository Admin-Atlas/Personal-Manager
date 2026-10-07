// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lifecycle section makes claims about somebody's hardware, so the states it must NOT confuse are
// what these pin: "PM couldn't ask" is not "nothing is loaded", a model PM didn't load is not PM's to
// free, and a server with no unload route must say so rather than offer options that do nothing.
//
// The on-battery release row moved to On battery, and its tests with it (LocalAiPower.test.tsx).
// What stays here is the release policy it sits beside.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  getLocalReleasePolicy.mockResolvedValue({
    policy: "server",
    idle_minutes: 5,
    battery_idle_minutes: 0,
  });
  setLocalReleasePolicy.mockResolvedValue(undefined);
  getTrayEnabled.mockResolvedValue(false);
  setTrayEnabled.mockResolvedValue(undefined);
});
afterEach(cleanup);

const loaded = async (over: Partial<LocalGpuResidency> = {}) => {
  localGpuResidency.mockResolvedValue(residency(over));
  const view = render(<LocalAiLifecycle configured power={null} />);
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

  it("says plainly when PM cannot release from this server, and how to do it yourself", async () => {
    // PM unloads only through Ollama's own API. Offering a picker that silently does nothing would
    // be worse than not having the feature — but the limit is PM's, not the server's: LM Studio
    // documents an eject (`lms unload`, and `POST /api/v1/models/unload` since 0.4.0). This said LM
    // Studio "has no unload command", which sent people to stop a server they didn't need to.
    await loaded({ no_unload_route: true });
    const line = await screen.findByText(/PM can only unload a model through Ollama/);
    expect(line.textContent).toMatch(/in LM Studio, eject the model \(or run lms unload\)/);
    expect(line.textContent).not.toMatch(/no unload command|no way to unload|only way/);
  });

  it("mentions an external display on the card, and promises to do nothing about it", async () => {
    // Surfaced, never acted on: plugging in a screen usually means more work is coming, not less.
    await loaded({ dgpu_displays: ["HDMI-A-1"] });
    const line = await screen.findByText(/external display/i);
    expect(line.textContent).toMatch(/HDMI-A-1/);
    expect(line.textContent).toMatch(/won.t change anything/i);
  });

  it("stays quiet about all of it until an endpoint is connected", async () => {
    render(<LocalAiLifecycle configured={false} power={null} />);
    expect(await screen.findByText(/Once your model server is connected/i)).toBeTruthy();
    expect(screen.queryByText(/Release now/i)).toBeNull();
  });

  it("says 'its memory' on a machine without a separate graphics card", async () => {
    // Without a card the server holds the model in memory the processor shares, and "the graphics
    // card is free" would name hardware that isn't there.
    localGpuResidency.mockResolvedValue(residency({ resident: [], vram_gb: null }));
    render(<LocalAiLifecycle configured power={null} hasDiscreteGpu={false} />);
    expect(
      await screen.findByText("Nothing is loaded right now, so its memory is free."),
    ).toBeTruthy();
    expect(screen.queryByText(/the graphics card is free/i)).toBeNull();

    cleanup();
    localGpuResidency.mockResolvedValue(residency({ resident: [model()], vram_gb: null }));
    render(<LocalAiLifecycle configured power={null} hasDiscreteGpu={false} />);
    const line = await screen.findByText(/Your server is holding/);
    expect(line.textContent).toContain("Your server is holding gemma3:4b in memory.");
    expect(line.textContent).not.toMatch(/card/);
  });

  it("no longer holds the on-battery row, which lives in On battery", async () => {
    await loaded();
    await settled();
    expect(screen.queryByRole("combobox", { name: "On battery, hand the memory back" })).toBeNull();
  });
});

const policyRow = () =>
  screen.getByRole("combobox", { name: "Give the memory back" }) as HTMLSelectElement;

/** Wait for the stored policy to land — the picker is disabled until it has. */
const settled = async () => {
  await waitFor(() => expect(policyRow().disabled).toBe(false));
};

describe("the release policy", () => {
  it("switches the options off when the server can't unload anything", async () => {
    // A picker that silently does nothing is worse than none: the line says why, and the controls
    // say so too.
    getLocalReleasePolicy.mockResolvedValue({
      policy: "idle",
      idle_minutes: 5,
      battery_idle_minutes: 0,
    });
    await loaded({ no_unload_route: true });
    expect(await screen.findByText(/are switched off/)).toBeTruthy();
    await waitFor(() => expect(policyRow().value).toBe("idle"));
    expect(policyRow().disabled).toBe(true);
    const quiet = screen.getByRole("combobox", { name: "Quiet period" }) as HTMLSelectElement;
    expect(quiet.disabled).toBe(true);
  });

  it("leaves them on for a server that can", async () => {
    getLocalReleasePolicy.mockResolvedValue({
      policy: "idle",
      idle_minutes: 5,
      battery_idle_minutes: 0,
    });
    await loaded();
    await settled();
    const quiet = screen.getByRole("combobox", { name: "Quiet period" }) as HTMLSelectElement;
    expect(quiet.disabled).toBe(false);
  });

  it("qualifies the chosen policy only when it is set", async () => {
    // "Leave it to my server — PM changes nothing" must never be contradicted by a silent battery
    // rule set in another section. (The stored value showing in that row is On battery's test.)
    await loaded();
    await settled();
    expect(screen.queryByText(/Except on battery/)).toBeNull();

    cleanup();
    getLocalReleasePolicy.mockResolvedValue({
      policy: "server",
      idle_minutes: 5,
      battery_idle_minutes: 10,
    });
    await loaded();
    expect(
      await screen.findByText(
        "Except on battery: there, PM also hands the memory back after 10 minutes without use, as set under On battery.",
      ),
    ).toBeTruthy();
  });

  it("restores the release policy too when its write fails", async () => {
    await loaded();
    await settled();
    setLocalReleasePolicy.mockRejectedValueOnce(new Error("nope"));
    const policy = policyRow();
    fireEvent.change(policy, { target: { value: "idle" } });
    expect(await screen.findByText(/Couldn't save that/)).toBeTruthy();
    await waitFor(() => expect(policy.value).toBe("server"));
  });

  it("doesn't present a release policy before PM has read the stored one", async () => {
    getLocalReleasePolicy.mockReturnValue(new Promise(() => {}));
    await loaded();
    const policy = policyRow();
    expect(policy.value).toBe("");
    expect(policy.disabled).toBe(true);
    expect(screen.queryByText(/PM changes nothing\. Your server decides/)).toBeNull();
  });
});
