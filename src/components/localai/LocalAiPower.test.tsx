// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The On battery section (#432). It decides nothing — Rust does — so what these pin is that it says
// what Rust decided: why it can't act when it can't (and never "no key" about keys PM simply couldn't
// read), no value presented as the user's before PM has read it, a write that failed never left
// standing, and nowhere a promise that switching stops the battery draining.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW, REDUCES_POWER } from "../../lib/powerRoute";
import type { LocalLlmStatus, PowerRoleView, PowerView } from "../../lib/types";

const setLocalPowerPolicy = vi.fn();
const keepLocalOnBattery = vi.fn();

vi.mock("../../lib/ipc", () => ({
  setLocalPowerPolicy: (...a: unknown[]) => setLocalPowerPolicy(...a),
  keepLocalOnBattery: (...a: unknown[]) => keepLocalOnBattery(...a),
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({ depth: "standard" }),
}));

import { LocalAiPower } from "./LocalAiPower";

const movable = (over: Partial<PowerRoleView> = {}): PowerRoleView => ({
  route: "unchanged",
  blocked: null,
  local_model: "gemma3:4b",
  ...over,
});

/** A laptop on mains with both roles movable: the "ready" baseline every case bends one way. */
const power = (over: Partial<PowerView> = {}): PowerView => ({
  ...INERT_POWER_VIEW,
  source: "ac",
  percent: 90,
  has_battery: true,
  chat: movable(),
  background: movable(),
  ...over,
});

const status = (p: PowerView, over: Partial<LocalLlmStatus> = {}): LocalLlmStatus => ({
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
  ...over,
});

const onError = vi.fn();

function show(s: LocalLlmStatus | null, { configured = true, anyLocal = true } = {}) {
  return render(
    <LocalAiPower
      status={s}
      configured={configured}
      anyLocalRoleWithModel={anyLocal}
      onError={onError}
    />,
  );
}

const threshold = () =>
  screen.getByRole("combobox", { name: "Switch to the cloud at" }) as HTMLSelectElement;
const radios = () => screen.getAllByRole("radio") as HTMLInputElement[];
const keepLocal = () =>
  screen.getByRole("switch", { name: "Keep using local until I quit PM" }) as HTMLButtonElement;

/** Every control, for the "rendered and disabled" visibility rule. */
function expectAllDisabled() {
  expect(threshold().disabled).toBe(true);
  expect(radios()).toHaveLength(3);
  for (const r of radios()) expect(r.disabled).toBe(true);
  expect(keepLocal().disabled).toBe(true);
}

beforeEach(() => {
  vi.clearAllMocks();
  setLocalPowerPolicy.mockResolvedValue(undefined);
  keepLocalOnBattery.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("when the section can't act", () => {
  it("says so plainly for a cloud-only setup, with the controls shown and disabled", () => {
    show(null, { configured: false, anyLocal: false });
    expect(screen.getByText("Switch to cloud on battery — unavailable")).toBeTruthy();
    expect(
      screen.getByText(
        "This setting decides when PM uses a local model instead of the cloud. You're currently using OpenRouter for everything, so there's nothing to switch between. If you set up a local model in Local AI, this becomes available.",
      ),
    ).toBeTruthy();
    expectAllDisabled();
  });

  it("tells a keyless user PM works without it, and offers nothing to click", () => {
    const { container } = show(
      status(
        power({
          chat: movable({ blocked: "no_key" }),
          background: movable({ blocked: "no_key" }),
        }),
      ),
    );
    expect(screen.getByText("Switch to cloud on battery — unavailable")).toBeTruthy();
    expect(
      screen.getByText(
        "Running a local model on a dedicated GPU uses significant power. This setting can send requests to a cloud provider while you're on battery instead to save power. There are currently no cloud providers set up, but don't worry, PM works without it!",
      ),
    ).toBeTruthy();
    expect(screen.getByText("Linking an OpenRouter key would make this available.")).toBeTruthy();
    // A nudge, not a sales pitch: no button invites a key.
    expect(within(container).queryAllByRole("button", { name: /key/i })).toHaveLength(0);
    expectAllDisabled();
  });

  it("never calls keys it couldn't read 'no key'", () => {
    const { container } = show(
      status(
        power({
          chat: movable({ blocked: "key_unreadable" }),
          background: movable({ blocked: "no_key" }),
        }),
      ),
    );
    expect(screen.getByText("Switch to cloud on battery — unavailable right now")).toBeTruthy();
    expect(screen.getByText(/PM can't read your saved keys at the moment/)).toBeTruthy();
    expect(container.textContent).not.toMatch(/no cloud providers/i);
  });

  it("names each role's reason when the roles themselves rule it out", () => {
    show(
      status(
        power({
          chat: movable({ blocked: "local_only" }),
          background: movable({ blocked: "cloud_routing", local_model: null }),
        }),
      ),
    );
    expect(screen.getByText("Switch to cloud on battery — not used with your roles")).toBeTruthy();
    const body = screen.getByText(/To let PM switch while you're on battery/);
    expect(body.textContent).toContain("Chat is set to Local only, which never uses the cloud.");
    expect(body.textContent).toContain("Background work already uses the cloud.");
    expect(body.textContent).toContain(
      "set a role to Local, fall back to cloud under Assign roles above.",
    );
    expectAllDisabled();
  });

  it("treats a machine with no battery as always plugged in", () => {
    show(status(power({ has_battery: false, source: "ac", percent: null })));
    expect(
      screen.getByText(
        "No battery found, so PM treats this machine as always plugged in. Nothing here changes how it works.",
      ),
    ).toBeTruthy();
    expectAllDisabled();
  });

  it("presents nothing as the user's choice before PM has read it", () => {
    show(null);
    expectAllDisabled();
    // No 60% dressed up as a stored value, and no radio checked.
    expect(threshold().value).toBe("");
    expect(within(threshold()).queryByRole("option", { selected: true })?.textContent).not.toBe(
      "60%",
    );
    for (const r of radios()) expect(r.checked).toBe(false);
    // A switch can only look "off"; its title says that isn't an answer.
    const keep = screen.getByRole("switch", { name: "Keep using local until I quit PM" });
    expect(keep.getAttribute("title")).toBe("PM hasn't read this yet.");
    // And no readout or gate copy claims anything either.
    expect(screen.queryByText(/unavailable/)).toBeNull();
    expect(screen.queryByText(/Plugged in|On battery at/)).toBeNull();
  });
});

describe("the controls", () => {
  it("sends the level picked, and 0 for Never", () => {
    show(status(power()));
    fireEvent.change(threshold(), { target: { value: "40" } });
    expect(setLocalPowerPolicy).toHaveBeenCalledWith({ threshold: 40 });
    fireEvent.change(threshold(), { target: { value: "0" } });
    expect(setLocalPowerPolicy).toHaveBeenLastCalledWith({ threshold: 0 });
    expect(within(threshold()).getByRole("option", { name: "Never" })).toBeTruthy();
  });

  it("shows a stored level that isn't on the list as itself", () => {
    show(status(power({ threshold: 65, return_at: 80 })));
    expect(threshold().value).toBe("65");
    expect(within(threshold()).getByRole("option", { name: "65%" })).toBeTruthy();
    expect(screen.getByText(/When the battery falls to 65%/).textContent).toContain(
      "climbs back to 80%",
    );
  });

  it("shows all three What moves options with who each suits, and what each would move here", () => {
    show(status(power({ background: movable({ blocked: "local_only" }) })));
    const [both, chat, background] = radios();
    expect(both.checked).toBe(true);
    // All three "Suits you if" details at once, never one revealed by picking.
    expect(screen.getAllByText(/Suits you if/)).toHaveLength(3);
    expect(both.closest("label")?.textContent).toContain(
      "With your setup only chat would move: Background work is set to Local only, which never uses the cloud.",
    );
    expect(chat.closest("label")?.textContent).toContain("With your setup this moves chat.");
    expect(background.closest("label")?.textContent).toContain(
      "With your setup this moves nothing: Background work is set to Local only, which never uses the cloud.",
    );
    fireEvent.click(chat);
    expect(setLocalPowerPolicy).toHaveBeenCalledWith({ roles: "chat" });
  });

  it("works for a background-key-only setup, and says why chat can't move", () => {
    show(status(power({ any_cloud_key: true, chat: movable({ blocked: "no_key" }) })));
    expect(threshold().disabled).toBe(false);
    const chatOnly = screen.getByRole("radio", { name: /^Chat only/ });
    expect(chatOnly.closest("label")?.textContent).toContain(
      "Chat uses your main OpenRouter key, and only a background key is set up",
    );
  });

  it("lets the override be armed on mains", () => {
    show(status(power()));
    expect(keepLocal().disabled).toBe(false);
    fireEvent.click(keepLocal());
    expect(keepLocalOnBattery).toHaveBeenCalledWith(true);
    // Optimistic until the next snapshot says otherwise.
    expect(keepLocal().getAttribute("aria-checked")).toBe("true");
  });

  it("puts a rejected write back to the stored value and reports it", async () => {
    setLocalPowerPolicy.mockRejectedValueOnce(new Error("vault locked"));
    show(status(power()));
    fireEvent.change(threshold(), { target: { value: "40" } });
    await waitFor(() => expect(onError).toHaveBeenCalledWith("Error: vault locked"));
    expect(threshold().value).toBe("60");
  });

  it("takes a fresh snapshot as the truth over what it assumed", () => {
    const view = show(status(power()));
    fireEvent.change(threshold(), { target: { value: "40" } });
    expect(threshold().value).toBe("40");
    // The backend clamped or refused it quietly: the snapshot wins.
    view.rerender(
      <LocalAiPower
        status={status(power({ threshold: 30, return_at: 45 }))}
        configured
        anyLocalRoleWithModel
        onError={onError}
      />,
    );
    expect(threshold().value).toBe("30");
  });
});

describe("the readout", () => {
  it("names what moved, what the move buys, and when it comes back", () => {
    show(
      status(
        power({
          source: "battery",
          state: "battery_low",
          percent: 50,
          consent: "both",
          chat: movable({ route: "cloud" }),
          background: movable({ route: "cloud" }),
        }),
      ),
    );
    const readout = screen.getByText(/going to the cloud/);
    expect(readout.textContent).toContain(REDUCES_POWER);
    expect(readout.textContent).toContain("climbs back to 75%");
  });

  it("asks inline when the question is waiting, with the three answers", () => {
    show(
      status(
        power({
          source: "battery",
          state: "battery_low",
          consent_needed: true,
          chat: movable({ route: "needs_consent" }),
          background: movable({ route: "needs_consent" }),
        }),
      ),
    );
    expect(screen.getByText(/waiting for your go-ahead/)).toBeTruthy();
    for (const name of [
      "Keep using local",
      "Use the cloud on battery",
      "Never switch on battery",
    ]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
  });

  it("offers to withdraw consent once it is given, naming what it covers", () => {
    show(status(power({ consent: "background" })));
    expect(
      screen.getByText("You've allowed PM to use the cloud on battery for background work."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Withdraw" }));
    expect(setLocalPowerPolicy).toHaveBeenCalledWith({ consent: "none" });
  });

  it("never says switching stops the battery draining", () => {
    // The card stays awake while the server holds a model, so the cloud route REDUCES power use;
    // it does not stop anything. Checked across every state the section words.
    const states: Array<[LocalLlmStatus | null, { configured?: boolean; anyLocal?: boolean }]> = [
      [null, { configured: false, anyLocal: false }],
      [null, {}],
      [status(power()), {}],
      [status(power({ threshold: 0 })), {}],
      [status(power({ has_battery: false })), {}],
      [
        status(
          power({
            chat: movable({ blocked: "no_key" }),
            background: movable({ blocked: "no_key" }),
          }),
        ),
        {},
      ],
      [
        status(
          power({
            source: "battery",
            state: "battery_low",
            consent: "both",
            chat: movable({ route: "cloud" }),
          }),
          { chat_loaded: true },
        ),
        {},
      ],
      [
        status(
          power({
            source: "battery",
            state: "battery_low",
            consent_needed: true,
            chat: movable({ route: "needs_consent" }),
          }),
        ),
        {},
      ],
    ];
    for (const [s, opts] of states) {
      const { container } = show(s, opts);
      expect(container.textContent).not.toMatch(/stops? draining/i);
      cleanup();
    }
  });
});
