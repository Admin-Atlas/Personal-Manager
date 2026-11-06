// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { evaluateAttemptMarker, MACOS_MINIMUM_FIELD, macosUpdateBlock } from "./updateGate";

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

describe("macosUpdateBlock", () => {
  const feed = (minimum: unknown) => ({ version: "3.200.0-alpha", [MACOS_MINIMUM_FIELD]: minimum });

  it("reads the field the release writes", () => {
    // scripts/build-updater-manifest.mjs writes this name, and its test pins it there too.
    expect(MACOS_MINIMUM_FIELD).toBe("minimum_macos_version");
  });

  it("holds back an update that needs a newer macOS, naming both versions", () => {
    expect(macosUpdateBlock(feed("15.0"), "14.5")).toEqual({ required: "15", running: "14.5" });
    expect(macosUpdateBlock(feed("14.0"), "13.6.1")).toEqual({ required: "14", running: "13.6.1" });
    expect(macosUpdateBlock(feed("14.2"), "14.1.9")).toEqual({
      required: "14.2",
      running: "14.1.9",
    });
    expect(macosUpdateBlock(feed("14.2.1"), "14.2")).toEqual({
      required: "14.2.1",
      running: "14.2",
    });
  });

  it("compares numerically, not as text", () => {
    expect(macosUpdateBlock(feed("14.0"), "9.9")).not.toBeNull();
    expect(macosUpdateBlock(feed("26.0"), "15.7")).not.toBeNull();
    expect(macosUpdateBlock(feed("14.10"), "14.9")).not.toBeNull();
    expect(macosUpdateBlock(feed("14.9"), "14.10")).toBeNull();
  });

  it("lets through an update this Mac can open, a missing part counting as 0", () => {
    expect(macosUpdateBlock(feed("14.0"), "14.0")).toBeNull();
    expect(macosUpdateBlock(feed("14"), "14.0")).toBeNull();
    expect(macosUpdateBlock(feed("14.0.0"), "14")).toBeNull();
    expect(macosUpdateBlock(feed("14.0"), "15.1")).toBeNull();
    expect(macosUpdateBlock(feed("14.0"), "26.0.1")).toBeNull();
  });

  it("reads a running version with a suffix by its numbers", () => {
    // A Rapid Security Response build: still macOS 13.4.1, still too old for 14.
    expect(macosUpdateBlock(feed("14.0"), "13.4.1 (a)")).toEqual({
      required: "14",
      running: "13.4.1 (a)",
    });
    expect(macosUpdateBlock(feed("14.0"), " 14.5\n")).toBeNull();
  });

  it("treats the compatibility-mode 10.16 as the macOS it says", () => {
    // Apple's compatibility mode reports every macOS from 11 on as 10.16. Holding back is the safe
    // reading: a wrong hold leaves PM as it is, a wrong install leaves a PM that doesn't open.
    expect(macosUpdateBlock(feed("14.0"), "10.16")).not.toBeNull();
  });

  it("goes ahead off macOS, and when this Mac's version can't be read", () => {
    expect(macosUpdateBlock(feed("15.0"), null)).toBeNull();
    expect(macosUpdateBlock(feed("15.0"), "")).toBeNull();
    expect(macosUpdateBlock(feed("15.0"), "unknown")).toBeNull();
  });

  it("goes ahead when the feed states no minimum, or one the release verifier wouldn't pass", () => {
    expect(macosUpdateBlock({ version: "3.174.2-alpha" }, "13.6")).toBeNull();
    for (const bad of [undefined, null, 15, "", "15.0-beta", "v15", " 15.0", "15.0.0.1", {}]) {
      expect(macosUpdateBlock(feed(bad), "13.6")).toBeNull();
    }
    for (const notAFeed of [undefined, null, "15.0", 15, []]) {
      expect(macosUpdateBlock(notAFeed, "13.6")).toBeNull();
    }
  });
});
