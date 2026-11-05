// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure classifier behind the sidebar footer's live half. The load-bearing tests are the SILENT
// ones: three quite different situations must all render exactly as the footer rendered before this
// existed, and the third of them ("PM cannot tell") must never come out as "not loaded".

import { describe, expect, it } from "vitest";
import {
  ACTIVITY_DETAIL,
  ACTIVITY_LABEL,
  awaitsLocalModel,
  localModelActivity,
  type LocalModelActivity,
} from "./localModelState";
import { INERT_POWER_VIEW } from "./powerRoute";
import type { LocalLlmStatus, PowerRoleView } from "./types";

const status = (over: Partial<LocalLlmStatus>): LocalLlmStatus => ({
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
  power: INERT_POWER_VIEW,
  ...over,
});

describe("localModelActivity", () => {
  it("says nothing at all when there is nothing it can honestly say", () => {
    // No snapshot yet, and no endpoint — the zero-pixel contract the whole footer already keeps.
    expect(localModelActivity(null, "chat")).toBeNull();
    expect(localModelActivity(status({ configured: false }), "chat")).toBeNull();
    // The role is using cloud, so the row is naming a cloud model and none of this is about it.
    expect(localModelActivity(status({ chat_local_model: null }), "chat")).toBeNull();
    // And the one worth naming: an endpoint with no `/api/ps` — llama-server, LM Studio, a
    // `/v1`-only proxy — or nothing seen recently enough to repeat. "Cannot tell" is silent, and
    // must NEVER come out as "not loaded": those servers hold their model for their whole life.
    expect(localModelActivity(status({ chat_loaded: null }), "chat")).toBeNull();
  });

  it("reports a call in flight ahead of anything the server said", () => {
    // In-flight is PM's own count, with no server to ask and nothing to be stale about — so it wins
    // even over a residency reading that says the model is not there (a cold load is exactly that).
    expect(localModelActivity(status({ chat_answering: true, chat_loaded: false }), "chat")).toBe(
      "answering",
    );
  });

  it("keeps the two roles apart", () => {
    const s = status({ chat_answering: true, background_answering: false });
    expect(localModelActivity(s, "chat")).toBe("answering");
    expect(localModelActivity(s, "background")).toBeNull();
  });

  it("separates a server unloading a model from PM handing it back", () => {
    expect(localModelActivity(status({ chat_loaded: true }), "chat")).toBe("loaded");
    // Not loaded, and PM's own release policy is why — a setting the user chose and can change.
    expect(localModelActivity(status({ chat_loaded: false, chat_released: true }), "chat")).toBe(
      "released",
    );
    // Not loaded, and PM did not do it. Their server's business.
    expect(localModelActivity(status({ chat_loaded: false }), "chat")).toBe("unloaded");
    // A release marker is only meaningful while the model is absent: something resident is
    // resident, whoever loaded it.
    expect(localModelActivity(status({ chat_loaded: true, chat_released: true }), "chat")).toBe(
      "loaded",
    );
  });

  it("has a word and a sentence for every state it can return", () => {
    const states: LocalModelActivity[] = ["answering", "loaded", "released", "unloaded"];
    for (const state of states) {
      expect(ACTIVITY_LABEL[state]).toBeTruthy();
      expect(ACTIVITY_DETAIL[state]).toBeTruthy();
    }
    // The one a passing glance is most likely to misread, so it is spelled out rather than implied.
    expect(ACTIVITY_LABEL.unloaded).toBe("not loaded");
  });

  it("puts no 'loaded' chip over a row the On battery policy has moved to the cloud", () => {
    // The backend reports no local model for a moved role, so the row names the cloud model — and a
    // "loaded" beside a cloud model's name would read as the cloud model being on the card. The
    // loaded fields still describe the parked model; this classifier must not borrow them.
    const moved = status({
      chat_local_model: null,
      chat_loaded: true,
      power: {
        ...INERT_POWER_VIEW,
        state: "battery_low",
        chat: {
          route: "cloud",
          blocked: null,
          local_model: "gemma3:4b",
          effective: "cloud_for_power",
          cloud_key: "present",
        },
      },
    });
    expect(localModelActivity(moved, "chat")).toBeNull();
  });

  it("words a release without claiming which setting caused it", () => {
    // The on-battery release and the release policy mark a model released through the same path.
    expect(ACTIVITY_DETAIL.released).toMatch(/Local AI settings/);
  });
});

describe("awaitsLocalModel", () => {
  const role = (over: Partial<PowerRoleView>): PowerRoleView => ({
    route: "unchanged",
    blocked: "no_local_model",
    local_model: null,
    effective: "nothing",
    cloud_key: "present",
    ...over,
  });
  const withRoles = (chat: PowerRoleView, background: PowerRoleView) =>
    status({ power: { ...INERT_POWER_VIEW, chat, background } });

  it("is a local role with nothing to answer it, even with a cloud key there", () => {
    // Local AI off (the status then reads as not configured, with no local model named), with chat
    // routed local: the footer must not name the cloud model as if it answered.
    const off = {
      ...withRoles(role({}), role({ blocked: "cloud_routing", effective: "cloud" })),
      configured: false,
      chat_local_model: null,
      background_local_model: null,
    };
    expect(awaitsLocalModel(off, "chat")).toBe(true);
    expect(awaitsLocalModel(off, "background")).toBe(false);
  });

  it("is never a cloud role, a role something answers, or a snapshot that can't say", () => {
    const noKey = withRoles(
      role({ blocked: "cloud_routing", cloud_key: "absent" }),
      role({ blocked: null, effective: "local_then_cloud", local_model: "gemma3:4b" }),
    );
    expect(awaitsLocalModel(noKey, "chat")).toBe(false);
    expect(awaitsLocalModel(noKey, "background")).toBe(false);
    expect(awaitsLocalModel(withRoles(role({ effective: "unknown" }), role({})), "chat")).toBe(
      false,
    );
    expect(awaitsLocalModel(null, "chat")).toBe(false);
  });
});
