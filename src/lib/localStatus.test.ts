// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure classifier behind the chat provider surfaces (sidebar line + composer chip). Pins the
// zero-pixel contract (no endpoint → null) and the tri-state mapping, so a surface can't drift from
// the others.

import { describe, expect, it } from "vitest";
import { localEndpointState } from "./localStatus";
import type { LocalLlmStatus } from "./types";

const status = (over: Partial<LocalLlmStatus>): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: true,
  chat_local_model: null,
  background_local_model: null,
  ...over,
});

describe("localEndpointState", () => {
  it("returns null when there is no status (nothing configured yet)", () => {
    expect(localEndpointState(null)).toBeNull();
  });

  it("returns null when an endpoint is not configured (zero-pixel for cloud-only users)", () => {
    expect(localEndpointState(status({ configured: false }))).toBeNull();
  });

  it("maps a reachable endpoint to connected", () => {
    expect(localEndpointState(status({ reachable: true }))).toBe("connected");
  });

  it("maps a cooldown to resting regardless of the last reachability", () => {
    expect(localEndpointState(status({ in_cooldown: true, reachable: false }))).toBe("resting");
    expect(localEndpointState(status({ in_cooldown: true, reachable: true }))).toBe("resting");
  });

  it("maps an unreachable-but-not-resting endpoint to unreachable", () => {
    expect(localEndpointState(status({ reachable: false, in_cooldown: false }))).toBe(
      "unreachable",
    );
  });
});
