// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The bridge's reading rules (modBridge.ts): what a frame's messages may be, and every refusal.

import { describe, expect, it } from "vitest";

import {
  MAX_ARGS_CHARS,
  MAX_FRAME_HEIGHT,
  MIN_FRAME_HEIGHT,
  callError,
  clampFrameHeight,
  encodeArgs,
  errorText,
  isConnectRequest,
  readPortMessage,
} from "./modBridge";

describe("the connect request", () => {
  it("is exactly the versioned shape", () => {
    expect(isConnectRequest({ pm: "connect", version: 1 })).toBe(true);
    expect(isConnectRequest({ pm: "connect", version: 1, modId: "atlas.map" })).toBe(true);
  });

  it.each([
    null,
    undefined,
    "connect",
    ["connect"],
    { pm: "connect" },
    { pm: "connect", version: "1" },
    { pm: "connect", version: 2 },
    { pm: "port", version: 1 },
    { type: "connect", version: 1 },
  ])("refuses %j", (data) => {
    expect(isConnectRequest(data)).toBe(false);
  });
});

describe("a height request", () => {
  it("is clamped and rounded", () => {
    expect(clampFrameHeight(300)).toBe(300);
    expect(clampFrameHeight(300.6)).toBe(301);
    expect(clampFrameHeight(0)).toBe(MIN_FRAME_HEIGHT);
    expect(clampFrameHeight(-1e9)).toBe(MIN_FRAME_HEIGHT);
    expect(clampFrameHeight(1e9)).toBe(MAX_FRAME_HEIGHT);
    expect(readPortMessage({ resize: 1e9 })).toEqual({ kind: "resize", height: MAX_FRAME_HEIGHT });
  });

  it.each(["300", "300px", null, undefined, Number.NaN, Infinity, -Infinity, {}, [300], true])(
    "drops %j",
    (value) => {
      expect(clampFrameHeight(value)).toBeNull();
      expect(readPortMessage({ resize: value })).toEqual({ kind: "drop" });
    },
  );
});

describe("a call", () => {
  it("is read with its id, method and arguments, and nothing else", () => {
    expect(
      readPortMessage({
        id: 0,
        method: "storage.get",
        args: { key: "k" },
        session: "s",
        modId: "m",
      }),
    ).toEqual({ kind: "call", id: 0, method: "storage.get", args: { key: "k" } });
    expect(readPortMessage({ id: 5, method: "a" })).toEqual({
      kind: "call",
      id: 5,
      method: "a",
      args: null,
    });
  });

  it.each([
    null,
    "a string",
    42,
    [1, "storage.get"],
    { method: "storage.get" },
    { id: -1, method: "storage.get" },
    { id: 1.5, method: "storage.get" },
    { id: "1", method: "storage.get" },
    { id: Number.MAX_SAFE_INTEGER + 1, method: "storage.get" },
    { id: Number.NaN, method: "storage.get" },
  ])("drops %j, which has no id to answer", (data) => {
    expect(readPortMessage(data)).toEqual({ kind: "drop" });
  });

  it.each([
    undefined,
    "",
    "1abc",
    ".storage",
    "storage get",
    "storage/get",
    "x".repeat(129),
    "störage.get",
    "a\nb",
    7,
  ])("refuses the method %j", (method) => {
    expect(readPortMessage({ id: 3, method })).toMatchObject({ kind: "refuse", id: 3 });
  });

  it("refuses arguments that aren't plain JSON", () => {
    const cycle: Record<string, unknown> = {};
    cycle.self = cycle;
    expect(readPortMessage({ id: 1, method: "a", args: cycle })).toMatchObject({ kind: "refuse" });
    expect(readPortMessage({ id: 1, method: "a", args: { n: 10n } })).toMatchObject({
      kind: "refuse",
    });
  });

  it("refuses arguments past the size cap, and takes them at the cap", () => {
    const at = "x".repeat(MAX_ARGS_CHARS - 2); // the quotes make it exactly the cap
    expect(encodeArgs(at)).toEqual({ ok: true, value: at });
    expect(encodeArgs(at + "x")).toEqual({ ok: false, error: "A call's arguments are too large." });
  });

  it("sends exactly the JSON it measured", () => {
    const when = new Date(0);
    expect(encodeArgs({ when, gone: undefined, list: [1, undefined] })).toEqual({
      ok: true,
      value: { when: "1970-01-01T00:00:00.000Z", list: [1, null] },
    });
  });
});

describe("refusals sent back", () => {
  it("carry the backend's text, clipped", () => {
    expect(errorText("unknown method")).toBe("unknown method");
    expect(errorText(new Error("boom"))).toBe("boom");
    expect(errorText({ code: 1 })).toBe("PM refused that call.");
    expect(errorText("x".repeat(600))).toHaveLength(501);
    expect(callError(4, "y".repeat(600)).error).toHaveLength(501);
  });
});
