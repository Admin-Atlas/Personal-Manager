// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The On battery wording (#432). The backend decides; these pin that the words say what it decided —
// above all that nothing claims a move the router won't make, that "PM can't tell" never reads as
// "on battery", and that a role waiting on the consent question shows no "on battery" chip.

import { describe, expect, it } from "vitest";

import {
  INERT_POWER_VIEW,
  REDUCES_POWER,
  askPhrase,
  blockedReason,
  consentText,
  powerGate,
  powerReadout,
  powerSummary,
  powerTag,
  scopeNote,
  themPhrase,
  whoPhrase,
} from "./powerRoute";
import type { LocalLlmStatus, PowerRoleView, PowerView } from "./types";
import { sectionLabel } from "../components/localai/sections";

const movable = (over: Partial<PowerRoleView> = {}): PowerRoleView => ({
  route: "unchanged",
  blocked: null,
  local_model: "gemma3:4b",
  effective: "local_then_cloud",
  cloud_key: "present",
  ...over,
});

/** A laptop on mains with both roles movable — the "ready" baseline every case bends one way. */
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
  probed_now: true,
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

describe("REDUCES_POWER", () => {
  it("is exactly the locked phrase", () => {
    expect(REDUCES_POWER).toBe("reduces power use by not running inference on your GPU");
  });
});

describe("powerTag", () => {
  it("says nothing unless the policy has actually done something to the role", () => {
    expect(powerTag(null, "chat")).toBeNull();
    expect(
      powerTag(status(power({ chat: movable({ route: "cloud" }) }), { configured: false }), "chat"),
    ).toBeNull();
    expect(powerTag(status(power()), "chat")).toBeNull();
    // Waiting on the question: the strip owns that, and the local model really is still answering.
    expect(
      powerTag(status(power({ chat: movable({ route: "needs_consent" }) })), "chat"),
    ).toBeNull();
  });

  it("marks a moved role and a kept one, each for its own role only", () => {
    const s = status(
      power({ chat: movable({ route: "cloud" }), background: movable({ route: "kept_local" }) }),
    );
    expect(powerTag(s, "chat")).toBe("on_battery");
    expect(powerTag(s, "background")).toBe("kept_local");
  });
});

describe("powerGate", () => {
  it("is cloud_only before anything else, then loading", () => {
    expect(powerGate(false, true, status(power()))).toBe("cloud_only");
    expect(powerGate(true, false, status(power()))).toBe("cloud_only");
    // cloud_only outranks a missing status: there is nothing to wait for.
    expect(powerGate(false, false, null)).toBe("cloud_only");
    expect(powerGate(true, true, null)).toBe("loading");
  });

  it("explains a setup where neither role can move, unreadable keys first", () => {
    const both = (a: PowerRoleView["blocked"], b: PowerRoleView["blocked"]) =>
      powerGate(
        true,
        true,
        status(power({ chat: movable({ blocked: a }), background: movable({ blocked: b }) })),
      );
    expect(both("no_key", "key_unreadable")).toBe("key_unreadable");
    expect(both("no_key", "no_key")).toBe("no_key");
    expect(both("local_only", "no_key")).toBe("no_key");
    expect(both("local_only", "cloud_routing")).toBe("not_with_roles");
  });

  it("never calls a setup with only a background key keyless", () => {
    // Chat reads "no_key" because it uses only the main key, while background sits on the cloud
    // anyway — and the keyless copy ("no cloud providers set up") would then be false.
    const bgKeyOnly = power({
      any_cloud_key: true,
      chat: movable({ blocked: "no_key" }),
      background: movable({ blocked: "cloud_routing" }),
    });
    expect(powerGate(true, true, status(bgKeyOnly))).toBe("not_with_roles");
    expect(powerGate(true, true, status({ ...bgKeyOnly, any_cloud_key: false }))).toBe("no_key");
  });

  it("needs only one movable role to be worth showing", () => {
    expect(powerGate(true, true, status(power({ chat: movable({ blocked: "no_key" }) })))).toBe(
      "ready",
    );
  });

  it("treats a desktop as always plugged in, but never a machine that says it's on battery", () => {
    expect(powerGate(true, true, status(power({ has_battery: false })))).toBe("no_battery");
    expect(powerGate(true, true, status(power({ has_battery: false, source: "battery" })))).toBe(
      "ready",
    );
    // On a desktop nothing here can ever apply, so the battery is explained before a blocked setup:
    // naming the key that would "make this available" would promise something that won't happen.
    expect(
      powerGate(
        true,
        true,
        status(
          power({
            has_battery: false,
            chat: movable({ blocked: "no_key" }),
            background: movable({ blocked: "no_key" }),
          }),
        ),
      ),
    ).toBe("no_battery");
    // An unreadable sample is not evidence of a desktop.
    expect(powerGate(true, true, status(power({ has_battery: false, source: "unknown" })))).toBe(
      "ready",
    );
    expect(powerGate(true, true, status(power()))).toBe("ready");
  });
});

describe("whoPhrase / askPhrase", () => {
  it("names the roles with the right grammar", () => {
    expect(whoPhrase(true, false)).toBe("chat");
    expect(whoPhrase(false, true)).toBe("background work");
    expect(whoPhrase(true, true)).toBe("chat and background work");
    expect(askPhrase(true, false)).toBe("your chats");
    expect(askPhrase(false, true)).toBe(
      "background work (filing, and the titles, summaries and learning PM writes from your chats)",
    );
    expect(askPhrase(true, true)).toBe("your chats and background work");
    expect(themPhrase(true, false)).toBe("them");
    expect(themPhrase(false, true)).toBe("it");
    expect(themPhrase(true, true)).toBe("them");
  });

  it("puts only the waiting roles into the consent question", () => {
    const text = consentText(
      power({ source: "battery", percent: 41, background: movable({ route: "needs_consent" }) }),
    );
    expect(text).toContain("You're on battery at 41%.");
    expect(text).toContain(
      "PM can send background work (filing, and the titles, summaries and learning PM writes from your chats) to your OpenRouter cloud model instead of running it on your graphics card",
    );
    expect(text).toContain("leaves this machine");
    expect(text).toContain("billed to your OpenRouter key");
  });
});

describe("scopeNote", () => {
  it("says what each choice would really move", () => {
    expect(scopeNote("both", power())).toBe("With your setup this moves chat and background work.");
    expect(scopeNote("chat", power())).toBe("With your setup this moves chat.");

    const partial = power({ background: movable({ blocked: "local_only" }) });
    expect(scopeNote("both", partial)).toBe(
      "With your setup only chat would move: Background work is set to Local only, which never uses the cloud.",
    );
    expect(scopeNote("background", partial)).toBe(
      "With your setup this moves nothing: Background work is set to Local only, which never uses the cloud.",
    );

    const none = power({
      chat: movable({ blocked: "cloud_routing" }),
      background: movable({ blocked: "no_local_model" }),
    });
    expect(scopeNote("both", none)).toBe(
      "With your setup this moves nothing: Chat already uses the cloud; Background work has no local model.",
    );
  });

  it("names the key asymmetry rather than calling a background-only key 'no key'", () => {
    const p = power({ any_cloud_key: true, chat: movable({ blocked: "no_key" }) });
    expect(blockedReason("chat", "no_key", p)).toBe(
      "Chat uses your main OpenRouter key, and only a background key is set up",
    );
    const neither = power({
      chat: movable({ blocked: "no_key" }),
      background: movable({ blocked: "no_key" }),
    });
    expect(blockedReason("chat", "no_key", neither)).toBe("Chat has no OpenRouter key to use");
    expect(blockedReason("background", "no_key", neither)).toBe(
      "Background work has no OpenRouter key to use",
    );
    expect(blockedReason("chat", "key_unreadable", neither)).toBe(
      "PM can't read your saved keys right now",
    );
  });
});

describe("powerReadout — one case per row, first match wins", () => {
  const read = (over: Partial<PowerView>, loadedChat: boolean | null = null) =>
    powerReadout(power(over), loadedChat, null);

  it("1. can't tell", () => {
    expect(read({ source: "unknown", state: "mains" })).toBe(
      "PM can't tell right now whether you're plugged in, so it's acting as though you are.",
    );
    // Mid-latch it is NOT yet acting as though plugged in: an unreadable sample has to last the same
    // minute as any other change, and until then routes are still on the cloud. The readout says what
    // PM is doing, not what it will do.
    expect(
      read({
        source: "unknown",
        percent: null,
        state: "battery_low",
        chat: movable({ route: "cloud" }),
      }),
    ).toMatch(/^On battery, so chat is going to the cloud\./);
  });

  it("2. plugged back in, not settled yet", () => {
    expect(read({ source: "ac", state: "battery_low", chat: movable({ route: "cloud" }) })).toBe(
      "Plugged in. PM moves back to your local model in about a minute.",
    );
    // Nothing ever moved: there is nothing to move back.
    expect(read({ source: "ac", state: "battery_low" })).toBe(
      "Plugged in. PM is using your local model.",
    );
    expect(read({ source: "ac", state: "battery" })).toBe(
      "Plugged in. PM is using your local model.",
    );
  });

  it("3. just unplugged", () => {
    expect(read({ source: "battery", state: "mains", percent: 58 })).toBe(
      "On battery at 58%. PM waits a minute before acting on a change, so unplugging to move the laptop changes nothing.",
    );
  });

  it("4. mains, switching off", () => {
    expect(read({ threshold: 0 })).toBe(
      "Plugged in, battery at 90%. PM uses your local model as normal, and switching on battery is off.",
    );
  });

  it("5. mains, something would move", () => {
    expect(read({ roles: "chat", percent: null, consent: "chat" })).toBe(
      "Plugged in. On battery at 60% or below, PM moves chat to the cloud.",
    );
    expect(read({ background: movable({ blocked: "local_only" }), consent: "both" })).toBe(
      "Plugged in, battery at 90%. On battery at 60% or below, PM moves chat to the cloud.",
    );
    // Not agreed to yet — the question comes first.
    expect(read({ consent: "background" })).toBe(
      "Plugged in, battery at 90%. On battery at 60% or below, PM moves chat and background work to the cloud, after asking you once.",
    );
    // The override is on: nothing will move, so the readout mustn't promise a switch.
    expect(read({ keep_local: true })).toBe(
      "Plugged in, battery at 90%. You've asked PM to keep using your local model until you quit, so nothing moves on battery.",
    );
  });

  it("6. mains, nothing would move", () => {
    expect(read({ roles: "background", background: movable({ blocked: "local_only" }) })).toBe(
      "Plugged in, battery at 90%. With what you've chosen below, nothing would move on battery.",
    );
  });

  it("7. on battery, switching off", () => {
    expect(read({ source: "battery", state: "battery", percent: 20, threshold: 0 })).toBe(
      "On battery at 20%. Switching is off, so PM keeps using your local model.",
    );
  });

  it("8. on battery, above the level", () => {
    const above: Partial<PowerView> = { source: "battery", state: "battery", percent: 70 };
    expect(read({ ...above, consent: "both" })).toBe(
      "On battery at 70%. PM keeps using your local model until the battery falls to 60%, then moves chat and background work to the cloud.",
    );
    expect(read(above)).toBe(
      "On battery at 70%. PM keeps using your local model until the battery falls to 60%, then moves chat and background work to the cloud, after asking you once.",
    );
    expect(read({ ...above, keep_local: true })).toBe(
      "On battery at 70%. You asked PM to keep using your local model until you quit, so nothing moves.",
    );
    // Reachable with the section's readout on screen: the gate is ready because background could
    // move, but the chosen scope covers only chat, which is Local only.
    const nothing: Partial<PowerView> = {
      ...above,
      roles: "chat",
      chat: movable({ blocked: "local_only" }),
    };
    expect(powerGate(true, true, status(power(nothing)))).toBe("ready");
    expect(read(nothing)).toBe(
      "On battery at 70%. With what you've chosen below, nothing moves, so PM keeps using your local model.",
    );
    // Already at or below the level, inside the minute's settle (a raised level, or a normal drain).
    expect(read({ ...above, percent: 65, threshold: 70, consent: "both" })).toBe(
      "On battery at 65%. Once that has lasted a minute, PM moves chat and background work to the cloud.",
    );
    expect(read({ ...above, percent: 59 })).toBe(
      "On battery at 59%. Once that has lasted a minute, PM moves chat and background work to the cloud, after asking you once.",
    );
    // A battery that won't say its charge is never "low".
    expect(read({ ...above, percent: null })).toBe(
      "On battery. Your battery isn't reporting its charge, so PM keeps using your local model.",
    );
  });

  it("9. moved to the cloud — and the parked model still on the card", () => {
    const routed: Partial<PowerView> = {
      source: "battery",
      state: "battery_low",
      percent: 50,
      chat: movable({ route: "cloud" }),
      background: movable({ route: "cloud" }),
    };
    const text = read(routed);
    expect(text).toBe(
      `On battery at 50%, so chat and background work are going to the cloud. That ${REDUCES_POWER}. PM moves back to your local model about a minute after you plug in, or if the battery climbs back to 75%.`,
    );
    expect(read({ ...routed, background: movable() }, true)).toBe(
      `On battery at 50%, so chat is going to the cloud. That ${REDUCES_POWER}. PM moves back to your local model about a minute after you plug in, or if the battery climbs back to 75%. gemma3:4b is still loaded on your graphics card, and a loaded model keeps the card drawing power. To hand it back on battery, choose a time for "On battery, hand the memory back" below.`,
    );
  });

  it("10. waiting on the question", () => {
    expect(
      read({ source: "battery", state: "battery_low", chat: movable({ route: "needs_consent" }) }),
    ).toBe(
      "On battery at 90%. PM is waiting for your go-ahead before it uses the cloud, and is staying on your local model until then.",
    );
  });

  it("11. kept local", () => {
    expect(
      read({
        source: "battery",
        state: "battery_low",
        percent: null,
        chat: movable({ route: "kept_local" }),
      }),
    ).toBe(
      "On battery. You asked PM to keep using your local model until you quit, so nothing has moved.",
    );
  });

  it("12. low, but nothing chosen can move", () => {
    expect(read({ source: "battery", state: "battery_low", percent: 30 })).toBe(
      "On battery at 30%. Nothing you've chosen to move can use the cloud with your current setup, so PM is staying on your local model.",
    );
  });
});

describe("powerSummary — one case per branch, first match wins", () => {
  // The one-sentence version, for a readout outside On battery. Like the readout it words what Rust
  // decided and re-derives nothing — so each branch is pinned against the snapshot that selects it.

  it("says nothing where there is nothing to move, or nothing read yet", () => {
    expect(powerSummary("cloud_only", power())).toBeNull();
    expect(powerSummary("loading", null)).toBeNull();
    expect(powerSummary("ready", null)).toBeNull();
  });

  it("words each gate that keeps PM on the local model", () => {
    expect(powerSummary("no_battery", power({ has_battery: false }))).toBe(
      "No battery on this computer, so On battery never applies.",
    );
    // A section named in a plain string: it has to be the section's own name.
    expect(powerSummary("no_battery", power({ has_battery: false }))).toContain(
      sectionLabel("sec-localai-power"),
    );
    expect(powerSummary("no_key", power())).toBe(
      "On battery, PM stays on your local model — there's no cloud key to move to.",
    );
    expect(powerSummary("key_unreadable", power())).toBe(
      "On battery, PM stays on your local model while it can't read your saved keys.",
    );
    expect(powerSummary("not_with_roles", power())).toBe(
      "On battery, PM stays on your local model — your roles aren't set to Local, fall back to cloud.",
    );
  });

  it("puts a waiting question first", () => {
    expect(
      powerSummary(
        "ready",
        power({ consent_needed: true, keep_local: true, chat: movable({ route: "cloud" }) }),
      ),
    ).toBe("PM is waiting for your answer about using the cloud on battery.");
  });

  it("names what is on the cloud now, with the right verb", () => {
    expect(powerSummary("ready", power({ chat: movable({ route: "cloud" }) }))).toBe(
      "On battery, so chat is on your cloud model until you plug in.",
    );
    expect(
      powerSummary(
        "ready",
        power({ chat: movable({ route: "cloud" }), background: movable({ route: "cloud" }) }),
      ),
    ).toBe("On battery, so chat and background work are on your cloud model until you plug in.");
  });

  it("says the override, then switching off, then nothing movable", () => {
    expect(powerSummary("ready", power({ keep_local: true, threshold: 0 }))).toBe(
      "You've asked PM to keep using your local model until you quit.",
    );
    expect(powerSummary("ready", power({ threshold: 0 }))).toBe(
      "On battery, PM stays on your local model — switching is off.",
    );
    expect(
      powerSummary("ready", power({ roles: "chat", chat: movable({ blocked: "local_only" }) })),
    ).toBe("On battery, nothing you've chosen would move to the cloud.");
  });

  it("otherwise says when PM would move, and whether it asks first", () => {
    expect(powerSummary("ready", power())).toBe(
      "On battery at 60% or below, PM moves chat and background work to your cloud model, after asking you once.",
    );
    expect(powerSummary("ready", power({ consent: "both", roles: "background" }))).toBe(
      "On battery at 60% or below, PM moves background work to your cloud model.",
    );
  });

  it("never says switching stops the battery draining", () => {
    const gates = ["no_battery", "no_key", "key_unreadable", "not_with_roles", "ready"] as const;
    for (const gate of gates) {
      expect(powerSummary(gate, power()) ?? "").not.toMatch(/stops? draining/i);
    }
  });
});
