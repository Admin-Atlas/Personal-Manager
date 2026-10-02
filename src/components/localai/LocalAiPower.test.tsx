// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The On battery section (#432). It decides nothing — Rust does — so what these pin is that it says
// what Rust decided: why it can't act when it can't (and never "no key" about keys PM simply couldn't
// read), no value presented as the user's before PM has read it, a write that failed never left
// standing, and nowhere a promise that switching stops the battery draining.
//
// "On battery, hand the memory back" lives here now, beside every other battery decision, and its
// tests moved with it from LocalAiLifecycle.test.tsx with their assertions intact.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW, REDUCES_POWER } from "../../lib/powerRoute";
import type { LocalGpuResidency, LocalLlmStatus, PowerRoleView, PowerView } from "../../lib/types";

const setLocalPowerPolicy = vi.fn();
const keepLocalOnBattery = vi.fn();
const localGpuResidency = vi.fn();
const releaseLocalGpu = vi.fn();
const getLocalReleasePolicy = vi.fn();
const setLocalReleasePolicy = vi.fn();
const getTrayEnabled = vi.fn();
const setTrayEnabled = vi.fn();

// A factory REPLACES the whole module, so every function the section imports must be here. The
// release four are the battery row's (through `useReleaseSettings`); the tray pair is Model
// memory's, which one test renders beside this section on the same release settings.
vi.mock("../../lib/ipc", () => ({
  setLocalPowerPolicy: (...a: unknown[]) => setLocalPowerPolicy(...a),
  keepLocalOnBattery: (...a: unknown[]) => keepLocalOnBattery(...a),
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
import { LocalAiPower } from "./LocalAiPower";
import { sectionLabel } from "./sections";
import { useReleaseSettings } from "./useReleaseSettings";

const residency = (over: Partial<LocalGpuResidency> = {}): LocalGpuResidency => ({
  resident: [],
  vram_gb: 8,
  dgpu_displays: [],
  policy: "server",
  idle_minutes: 5,
  no_unload_route: false,
  ...over,
});

const movable = (over: Partial<PowerRoleView> = {}): PowerRoleView => ({
  route: "unchanged",
  blocked: null,
  local_model: "gemma3:4b",
  effective: "local_then_cloud",
  cloud_key: "present",
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

describe("when the section can't act", () => {
  it("says so plainly for a cloud-only setup, with the controls shown and disabled", () => {
    const { container } = show(null, { configured: false, anyLocal: false });
    expect(screen.getByText("Switch to the cloud on battery — not available yet")).toBeTruthy();
    // The pointer to the start card is a section name (a link inside the tab, plain text here), so
    // the sentence spans an element: read it as the reader does, whole.
    expect(container.textContent).toContain(
      `This moves work from a local model to your cloud model while a laptop's battery is low. Nothing runs on a local model yet, so there's nothing to move — ${sectionLabel("sec-localai-start")} walks you through setting one up.`,
    );
    // Not "you're using OpenRouter for everything": a keyless user uses nothing at all.
    expect(screen.queryByText(/OpenRouter for everything/)).toBeNull();
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
      "set a role to Local, fall back to cloud under Assign roles.",
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

describe("the two groups", () => {
  it("says what each one saves and what it costs, unfolded", () => {
    const { container } = show(status(power()));
    const headings = Array.from(container.querySelectorAll("h3")).map((h) => h.textContent);
    expect(headings).toEqual(["Moving work to the cloud", "Handing back the graphics card"]);
    expect(container.textContent).toContain(
      `Saves: sending requests to your cloud model ${REDUCES_POWER}. Costs: what you send leaves this computer and is billed to your OpenRouter key, and PM asks you before the first time.`,
    );
    expect(container.textContent).toContain(
      "Costs: the next message waits a few seconds while the model loads again. Works with Ollama; nothing leaves this computer.",
    );
    // The tray lives in Model memory, and the pointer names it rather than a direction.
    expect(container.textContent).toContain(
      `With the tray icon on (under ${sectionLabel("sec-localai-lifecycle")}), closing the window doesn't quit PM, so it stays on.`,
    );
    // Ready: nothing is folded away.
    expect(screen.queryByRole("button", { name: "What you could set here" })).toBeNull();
  });

  it("folds both groups for a cloud-only setup, keeping the controls mounted and disabled", async () => {
    const { container } = show(null, { configured: false, anyLocal: false });
    const fold = screen.getByRole("button", { name: "What you could set here" });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    // Mounted, so what the section would do is still there; disabled, because it can't yet.
    expectAllDisabled();
    await waitFor(() => expect(getLocalReleasePolicy).toHaveBeenCalled());
    expect(batteryRow().disabled).toBe(true);
    expect(batteryRow().closest("[inert]")).not.toBeNull();
    expect(container.querySelectorAll("h3")).toHaveLength(2);
  });

  it("folds them on a machine with no battery too", () => {
    show(status(power({ has_battery: false, source: "ac", percent: null })));
    expect(
      screen.getByRole("button", { name: "What you could set here" }).getAttribute("aria-expanded"),
    ).toBe("false");
  });

  it("keeps the consent line and Withdraw outside the fold", () => {
    // Taking back permission for data to leave the machine is never something to go looking for.
    show(status(power({ consent: "both" })), { anyLocal: false });
    expect(screen.getByRole("button", { name: "What you could set here" })).toBeTruthy();
    const withdraw = screen.getByRole("button", { name: "Withdraw" });
    expect(withdraw.closest("[inert]")).toBeNull();
    fireEvent.click(withdraw);
    expect(setLocalPowerPolicy).toHaveBeenCalledWith({ consent: "none" });
  });

  it("asks the consent question as the section, not as the app-wide strip", () => {
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
    expect(screen.getByText(/You can change any of this below\./)).toBeTruthy();
    expect(screen.queryByText(/in Settings → Local AI → On battery/)).toBeNull();
  });
});

// Moved here from LocalAiLifecycle.test.tsx with the row, assertions intact.
const batteryRow = () =>
  screen.getByRole("combobox", { name: "On battery, hand the memory back" }) as HTMLSelectElement;

describe("On battery, hand the memory back (#432)", () => {
  /** Render the section with the release settings it reads, and wait for them to be asked for. */
  const loaded = async (
    over: Partial<LocalGpuResidency> = {},
    s: LocalLlmStatus | null = status(power()),
  ) => {
    localGpuResidency.mockResolvedValue(residency(over));
    const view = show(s);
    await waitFor(() => expect(localGpuResidency).toHaveBeenCalled());
    return view;
  };

  /** Wait for the stored value to land — the row is disabled until it has. */
  const settled = async () => {
    await waitFor(() => expect(batteryRow().disabled).toBe(false));
  };

  it("writes only its own field", async () => {
    await loaded();
    await settled();
    fireEvent.change(batteryRow(), { target: { value: "5" } });
    // Positional, with the policy left alone: a battery change must not restate (and so risk
    // overwriting) the release policy it shares its storage with.
    expect(setLocalReleasePolicy).toHaveBeenCalledWith(null, undefined, 5);
    expect(
      await screen.findByText(/counting from no earlier than when you unplugged/),
    ).toBeTruthy();
  });

  it("shows nothing as stored until PM has read it", async () => {
    // A read that never answers: the row must not present "As set under …" as the user's choice.
    getLocalReleasePolicy.mockReturnValue(new Promise(() => {}));
    await loaded();
    expect(batteryRow().disabled).toBe(true);
    expect(batteryRow().value).toBe("");
  });

  it("names the policy it defers to by its section, not by a direction", async () => {
    await loaded();
    await settled();
    const zero = within(batteryRow()).getByRole("option", {
      name: `As set under ${sectionLabel("sec-localai-lifecycle")}`,
    });
    expect(zero.textContent).not.toMatch(/\b(above|below)\b/);
    expect(
      screen.getByText(/On battery, PM does whatever "Give the memory back" under/),
    ).toBeTruthy();
  });

  it("is off for a desktop, and says why", async () => {
    await loaded({}, status(power({ source: "ac", has_battery: false })));
    expect(
      await screen.findByText("PM didn't find a battery on this machine, so this never applies."),
    ).toBeTruthy();
    expect(batteryRow().disabled).toBe(true);
  });

  it("stays usable while the power readout isn't known, and on a laptop", async () => {
    // null is "not known to be a desktop", never "is a desktop".
    await loaded({}, null);
    await settled();
    cleanup();
    await loaded({}, status(power({ source: "ac", has_battery: true })));
    await settled();
  });

  it("is off when the server can't unload anything", async () => {
    await loaded({ no_unload_route: true });
    expect(
      await screen.findByText(
        /Your server can't unload a model on request, so this can't do anything with it/,
      ),
    ).toBeTruthy();
    expect(batteryRow().disabled).toBe(true);
  });

  it("is off until a server is connected", async () => {
    show(null, { configured: false, anyLocal: false });
    // Stored and read — the placeholder has gone — and still off: there is no server to hand memory
    // back from.
    await waitFor(() => expect(batteryRow().value).toBe("0"));
    expect(batteryRow().disabled).toBe(true);
  });

  it("shows the stored value", async () => {
    // The battery half of Model memory's "qualifies the chosen policy only when it is set".
    getLocalReleasePolicy.mockResolvedValue({
      policy: "server",
      idle_minutes: 5,
      battery_idle_minutes: 10,
    });
    await loaded();
    await waitFor(() => expect(batteryRow().value).toBe("10"));
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

  it("shows nothing as stored when neither the save nor reading it back worked", async () => {
    // Leaving the unsaved choice on screen beside "this shows what PM has stored" would be a lie —
    // in either section. The two pickers live in two sections now, on the tab's one set of release
    // settings, so both are rendered here on one, the way the tab does.
    const ready = status(power());
    function Both() {
      const release = useReleaseSettings({ status: ready });
      return (
        <>
          <LocalAiPower
            status={ready}
            configured
            anyLocalRoleWithModel
            onError={onError}
            release={release}
          />
          <LocalAiLifecycle configured power={ready.power} release={release} />
        </>
      );
    }
    render(<Both />);
    await waitFor(() => expect(localGpuResidency).toHaveBeenCalled());
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

  it("counts only a positive AC reading as a desktop", async () => {
    // A wedged read is an Unknown sample; it isn't evidence that the battery has gone.
    await loaded({}, status(power({ source: "unknown", has_battery: false })));
    await settled();
    expect(
      screen.queryByText("PM didn't find a battery on this machine, so this never applies."),
    ).toBeNull();
  });
});
