// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one-time On battery question (#432). It is the only thing standing between a low battery and
// what you send leaving the machine, so these pin that it appears only when the backend says it is
// owed, says plainly that data leaves and is billed, offers the safe answer first, and has no way to
// make it go away other than answering — a dismissed question would simply come back.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../lib/powerRoute";
import type { LocalLlmStatus, PowerRoleView, PowerView } from "../lib/types";

const setLocalPowerPolicy = vi.fn();
const keepLocalOnBattery = vi.fn();

vi.mock("../lib/ipc", () => ({
  setLocalPowerPolicy: (...a: unknown[]) => setLocalPowerPolicy(...a),
  keepLocalOnBattery: (...a: unknown[]) => keepLocalOnBattery(...a),
}));

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({ depth: "standard" }),
}));

import { PowerConsentAsk, PowerConsentStrip } from "./PowerConsentStrip";

const asking: PowerRoleView = {
  route: "needs_consent",
  blocked: null,
  local_model: "gemma3:4b",
  effective: "local_then_cloud",
  cloud_key: "present",
};
const still: PowerRoleView = {
  route: "unchanged",
  blocked: null,
  local_model: "gemma3:4b",
  effective: "local_then_cloud",
  cloud_key: "present",
};

const power = (over: Partial<PowerView> = {}): PowerView => ({
  ...INERT_POWER_VIEW,
  source: "battery",
  percent: 55,
  has_battery: true,
  state: "battery_low",
  consent_needed: true,
  chat: asking,
  background: asking,
  ...over,
});

const status = (p: PowerView): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: false,
  chat_local_model: "gemma3:4b",
  background_local_model: "gemma3:4b",
  served_window: null,
  served_window_proven: false,
  window_source: null,
  chat_answering: false,
  background_answering: false,
  chat_loaded: null,
  background_loaded: null,
  chat_released: false,
  background_released: false,
  power: p,
});

const ANSWERS = ["Keep using local", "Use the cloud on battery", "Never switch on battery"];

beforeEach(() => {
  vi.clearAllMocks();
  setLocalPowerPolicy.mockResolvedValue(undefined);
  keepLocalOnBattery.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("PowerConsentStrip", () => {
  it("renders nothing unless the backend says the question is owed", () => {
    expect(render(<PowerConsentStrip status={null} />).container.firstChild).toBeNull();
    expect(
      render(<PowerConsentStrip status={status(power({ consent_needed: false }))} />).container
        .firstChild,
    ).toBeNull();
  });

  it("is an info strip that announces itself, with no way to dismiss it", () => {
    render(<PowerConsentStrip status={status(power())} />);
    const strip = screen.getByRole("status");
    // Every way out is an answer: exactly the three, no ✕ or Dismiss.
    expect(Array.from(strip.querySelectorAll("button")).map((b) => b.textContent)).toEqual(ANSWERS);
    expect(screen.queryByRole("button", { name: /dismiss|close|✕/i })).toBeNull();
  });

  it("says that what you send leaves the machine and is billed", () => {
    render(<PowerConsentStrip status={status(power())} />);
    const strip = screen.getByRole("status");
    expect(strip.textContent).toContain("leaves this machine");
    expect(strip.textContent).toContain("billed");
    expect(strip.textContent).toContain("You're on battery at 55%.");
  });

  it("names only the roles that are waiting on it", () => {
    const words = (p: PowerView) => {
      const { container } = render(<PowerConsentStrip status={status(p)} />);
      const text = container.textContent ?? "";
      cleanup();
      return text;
    };
    expect(words(power({ background: still }))).toContain(
      "PM can send your chats to your OpenRouter cloud model instead of running them on your graphics card",
    );
    expect(words(power({ chat: still }))).toContain(
      "PM can send background work (filing, and the titles, summaries and learning PM writes from your chats) to your OpenRouter cloud model instead of running it on your graphics card",
    );
    expect(words(power())).toContain(
      "PM can send your chats and background work to your OpenRouter cloud model instead of running them on your graphics card",
    );
  });

  it("offers the safe answer first, and each answer does what it says", async () => {
    for (const [i, name] of ANSWERS.entries()) {
      render(<PowerConsentStrip status={status(power())} />);
      expect(screen.getAllByRole("button")[i].textContent).toBe(name);
      fireEvent.click(screen.getByRole("button", { name }));
      await waitFor(() =>
        expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(true),
      );
      cleanup();
    }
    expect(keepLocalOnBattery).toHaveBeenCalledWith(true);
    expect(setLocalPowerPolicy).toHaveBeenNthCalledWith(1, { consent: "both" });
    expect(setLocalPowerPolicy).toHaveBeenNthCalledWith(2, { threshold: 0 });
  });

  it("agrees to exactly the roles the question named", () => {
    render(<PowerConsentStrip status={status(power({ chat: still }))} />);
    expect(screen.getByText(/send background work/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Use the cloud on battery" }));
    expect(setLocalPowerPolicy).toHaveBeenCalledWith({ consent: "background" });
  });

  it("disables all three while an answer is being saved", async () => {
    let resolve = () => {};
    setLocalPowerPolicy.mockReturnValue(new Promise<void>((r) => (resolve = r)));
    const view = render(<PowerConsentStrip status={status(power())} />);
    const allDisabled = (on: boolean) => {
      for (const name of ANSWERS) {
        expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(on);
      }
    };
    fireEvent.click(screen.getByRole("button", { name: "Use the cloud on battery" }));
    allDisabled(true);
    // A snapshot that lands while the write is still in flight was read before it: still busy.
    view.rerender(<PowerConsentStrip status={status(power())} />);
    allDisabled(true);
    // Still disabled once it resolves: the question is answered, and the next snapshot takes it
    // away. Re-arming on the resolve would offer a second answer to a question already gone.
    await act(async () => resolve());
    allDisabled(true);
    // A snapshot after the answer that STILL asks hands the buttons back rather than wedging them.
    view.rerender(<PowerConsentStrip status={status(power())} />);
    allDisabled(false);
  });

  it("hands the buttons back and says so when the answer couldn't be saved", async () => {
    keepLocalOnBattery.mockRejectedValueOnce(new Error("nope"));
    render(<PowerConsentStrip status={status(power())} />);
    fireEvent.click(screen.getByRole("button", { name: "Keep using local" }));
    expect(await screen.findByText("Couldn't save that. Try again.")).toBeTruthy();
    for (const name of ANSWERS) {
      expect((screen.getByRole("button", { name }) as HTMLButtonElement).disabled).toBe(false);
    }
  });
});

describe("PowerConsentAsk inside On battery", () => {
  // The Settings overlay covers the strip, so the section asks the same question inline. There the
  // controls are just under it, and "in Settings → Local AI → On battery" would send someone to the
  // section they are already reading.
  const text = (inSettings: boolean) => {
    const { container } = render(<PowerConsentAsk power={power()} inSettings={inSettings} />);
    const words = container.textContent ?? "";
    cleanup();
    return words;
  };

  it("swaps only the last sentence", () => {
    const strip = text(false);
    const settings = text(true);
    const OLD = "You can change any of this in Settings → Local AI → On battery.";
    const NEW = "You can change any of this below.";
    expect(strip).toContain(OLD);
    expect(strip).not.toContain(NEW);
    expect(settings).toContain(NEW);
    expect(settings).not.toContain(OLD);
    expect(settings.replace(NEW, OLD)).toBe(strip);
  });

  it("is the strip's wording when nothing says otherwise", () => {
    render(<PowerConsentAsk power={power()} />);
    expect(
      screen.getByText(/You can change any of this in Settings → Local AI → On battery\./),
    ).toBeTruthy();
  });
});
