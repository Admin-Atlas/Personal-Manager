// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The keyless-onboarding predicate (#295). The load-bearing property is the strict-superset one:
// a cloud key must always pass (existing keyed users are unaffected), and the two new disjuncts
// (local endpoint / onboarding done) only ever ADD readiness.

import { describe, expect, it } from "vitest";
import { aiReady } from "./aiGate";
import type { AiProviderStatus } from "./types";

const s = (over: Partial<AiProviderStatus>): AiProviderStatus => ({
  has_cloud_key: false,
  local_configured: false,
  onboarding_done: false,
  ...over,
});

describe("aiReady", () => {
  it("is false only when nothing is set up", () => {
    expect(aiReady(s({}))).toBe(false);
  });

  it("is true on a cloud key — existing keyed users, unchanged", () => {
    expect(aiReady(s({ has_cloud_key: true }))).toBe(true);
    // strict superset: the key alone still passes regardless of the new disjuncts
    expect(aiReady(s({ has_cloud_key: true, local_configured: true, onboarding_done: true }))).toBe(
      true,
    );
  });

  it("is true on a configured local endpoint (the keyless case the old gate missed)", () => {
    expect(aiReady(s({ local_configured: true }))).toBe(true);
  });

  it("is true once onboarding is explicitly done (the 'set up AI later' path)", () => {
    expect(aiReady(s({ onboarding_done: true }))).toBe(true);
  });
});
