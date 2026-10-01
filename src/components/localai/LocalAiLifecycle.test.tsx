// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The lifecycle section makes claims about somebody's hardware, so the states it must NOT confuse are
// what these pin: "PM couldn't ask" is not "nothing is loaded", a model PM didn't load is not PM's to
// free, and a server with no unload route must say so rather than offer options that do nothing.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type { LocalGpuResidency, PowerView } from "../../lib/types";

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
    render(<LocalAiLifecycle configured={false} power={null} />);
    expect(await screen.findByText(/Connect an endpoint above/i)).toBeTruthy();
    expect(screen.queryByText(/Release now/i)).toBeNull();
  });
});

describe("On battery, hand the memory back (#432)", () => {
  const batteryRow = () =>
    screen.getByRole("combobox", { name: "On battery, hand the memory back" }) as HTMLSelectElement;

  /** Wait for the stored value to land — the row is disabled until it has. */
  const settled = async () => {
    await waitFor(() => expect(batteryRow().disabled).toBe(false));
  };

  it("writes only its own field", async () => {
    await loaded();
    await settled();
    fireEvent.change(batteryRow(), { target: { value: "5" } });
    // Positional, with the policy left alone: a battery change must not restate (and so risk
    // overwriting) the release policy it sits under.
    expect(setLocalReleasePolicy).toHaveBeenCalledWith(null, undefined, 5);
    expect(
      await screen.findByText(/counting from no earlier than when you unplugged/),
    ).toBeTruthy();
  });

  it("shows nothing as stored until PM has read it", async () => {
    // A read that never answers: the row must not present "As set above" as the user's choice.
    getLocalReleasePolicy.mockReturnValue(new Promise(() => {}));
    await loaded();
    expect(batteryRow().disabled).toBe(true);
    expect(batteryRow().value).toBe("");
  });

  it("is off for a desktop, and says why", async () => {
    const desktop: PowerView = { ...INERT_POWER_VIEW, source: "ac", has_battery: false };
    localGpuResidency.mockResolvedValue(residency());
    render(<LocalAiLifecycle configured power={desktop} />);
    expect(
      await screen.findByText("PM didn't find a battery on this machine, so this never applies."),
    ).toBeTruthy();
    expect(batteryRow().disabled).toBe(true);
  });

  it("stays usable while the power readout isn't known, and on a laptop", async () => {
    // null is "not known to be a desktop", never "is a desktop".
    await loaded();
    await settled();
    cleanup();
    const laptop: PowerView = { ...INERT_POWER_VIEW, source: "ac", has_battery: true };
    render(<LocalAiLifecycle configured power={laptop} />);
    await settled();
  });

  it("is off when the server can't unload anything", async () => {
    await loaded({ no_unload_route: true });
    expect(await screen.findByText(/no way to unload a model on request/i)).toBeTruthy();
    expect(batteryRow().disabled).toBe(true);
  });

  it("qualifies the chosen policy only when it is set", async () => {
    // "Leave it to my server — PM changes nothing" must never be contradicted by a silent battery
    // rule underneath it.
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
        "Except on battery: there, PM also hands the memory back after 10 minutes without use, as set below.",
      ),
    ).toBeTruthy();
    expect(batteryRow().value).toBe("10");
  });

  it("shows a stored value that isn't on the list rather than snapping to a neighbour", async () => {
    getLocalReleasePolicy.mockResolvedValue({
      policy: "server",
      idle_minutes: 5,
      battery_idle_minutes: 7,
    });
    await loaded();
    await settled();
    expect(batteryRow().value).toBe("7");
  });

  it("puts a failed write back to what PM has stored, and says so", async () => {
    // This used to be swallowed: the picker kept showing a choice that was never saved, so the
    // setting quietly did not apply.
    await loaded();
    await settled();
    setLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    fireEvent.change(batteryRow(), { target: { value: "5" } });

    expect(
      await screen.findByText("Couldn't save that. This shows what PM has stored."),
    ).toBeTruthy();
    await waitFor(() => expect(batteryRow().value).toBe("0"));
    // Re-read, not guessed: once on mount, once after the failure.
    expect(getLocalReleasePolicy).toHaveBeenCalledTimes(2);

    // And it clears on the next change that does save.
    fireEvent.change(batteryRow(), { target: { value: "2" } });
    await waitFor(() =>
      expect(screen.queryByText("Couldn't save that. This shows what PM has stored.")).toBeNull(),
    );
  });

  it("restores the release policy too when its write fails", async () => {
    await loaded();
    await settled();
    setLocalReleasePolicy.mockRejectedValueOnce(new Error("nope"));
    const policy = screen.getByRole("combobox", {
      name: "Give the memory back",
    }) as HTMLSelectElement;
    fireEvent.change(policy, { target: { value: "idle" } });
    expect(await screen.findByText(/Couldn't save that/)).toBeTruthy();
    await waitFor(() => expect(policy.value).toBe("server"));
  });

  it("shows nothing as stored when neither the save nor reading it back worked", async () => {
    // Leaving the unsaved choice on screen beside "this shows what PM has stored" would be a lie.
    await loaded();
    await settled();
    setLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    getLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    fireEvent.change(batteryRow(), { target: { value: "5" } });
    expect(
      await screen.findByText("Couldn't save that, and PM couldn't read back what is stored."),
    ).toBeTruthy();
    expect(batteryRow().value).toBe("");
    expect(batteryRow().disabled).toBe(true);
    const policy = screen.getByRole("combobox", {
      name: "Give the memory back",
    }) as HTMLSelectElement;
    expect(policy.value).toBe("");
    expect(policy.disabled).toBe(true);
  });

  it("doesn't present a release policy before PM has read the stored one", async () => {
    getLocalReleasePolicy.mockReturnValue(new Promise(() => {}));
    await loaded();
    const policy = screen.getByRole("combobox", {
      name: "Give the memory back",
    }) as HTMLSelectElement;
    expect(policy.value).toBe("");
    expect(policy.disabled).toBe(true);
    expect(screen.queryByText(/PM changes nothing\. Your server decides/)).toBeNull();
  });

  it("counts only a positive AC reading as a desktop", async () => {
    // A wedged read is an Unknown sample; it isn't evidence that the battery has gone.
    const unread: PowerView = { ...INERT_POWER_VIEW, source: "unknown", has_battery: false };
    localGpuResidency.mockResolvedValue(residency());
    render(<LocalAiLifecycle configured power={unread} />);
    await settled();
    expect(
      screen.queryByText("PM didn't find a battery on this machine, so this never applies."),
    ).toBeNull();
  });
});
