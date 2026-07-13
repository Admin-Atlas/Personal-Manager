// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { evaluateAttemptMarker } from "./updateGate";

describe("evaluateAttemptMarker", () => {
  it("no marker → nothing to decide", () => {
    expect(
      evaluateAttemptMarker({ attempted: null, running: "3.6.2-alpha", offered: "3.6.3-alpha" }),
    ).toEqual({
      blocked: false,
      clearMarker: false,
    });
  });

  it("now running the attempted version → applied, clear the marker", () => {
    expect(
      evaluateAttemptMarker({ attempted: "3.6.3-alpha", running: "3.6.3-alpha", offered: null }),
    ).toEqual({
      blocked: false,
      clearMarker: true,
    });
  });

  it("feed re-offers the version we tried and we're still on the old one → blocked", () => {
    expect(
      evaluateAttemptMarker({
        attempted: "3.6.3-alpha",
        running: "3.6.2-alpha",
        offered: "3.6.3-alpha",
      }),
    ).toEqual({ blocked: true, clearMarker: false });
  });

  it("feed offers a newer version than the one we tried → stale marker, clear it", () => {
    expect(
      evaluateAttemptMarker({
        attempted: "3.6.3-alpha",
        running: "3.6.2-alpha",
        offered: "3.6.4-alpha",
      }),
    ).toEqual({ blocked: false, clearMarker: true });
  });

  it("marker present but feed offers nothing (offline) → inconclusive, keep the marker", () => {
    expect(
      evaluateAttemptMarker({ attempted: "3.6.3-alpha", running: "3.6.2-alpha", offered: null }),
    ).toEqual({
      blocked: false,
      clearMarker: false,
    });
  });
});
