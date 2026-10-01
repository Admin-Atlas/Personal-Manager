// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The two readings of an endpoint address that copy leans on: which server it is, and whether it is
// on this computer. The second is a privacy claim — "on this computer" said of a LAN server would be
// false about where someone's chats go — so it is pinned on the near misses, not just the easy case.

import { describe, expect, it } from "vitest";

import { isLoopback, runnerOf, whereOf } from "./readiness";

describe("runnerOf", () => {
  it("names the three servers by the ports PM probes", () => {
    expect(runnerOf("http://127.0.0.1:11434")).toBe("Ollama");
    expect(runnerOf("http://localhost:1234/v1")).toBe("LM Studio");
    expect(runnerOf("http://192.168.1.20:8080")).toBe("llama-server");
  });

  it("parses the port rather than matching text", () => {
    // ":114341" is not 11434, and "11434" in a path is not a port.
    expect(runnerOf("http://localhost:114341")).toBeNull();
    expect(runnerOf(":114341")).toBeNull();
    expect(runnerOf("http://localhost:9000/11434")).toBeNull();
  });

  it("names nothing it can't place", () => {
    expect(runnerOf("http://localhost:9000")).toBeNull();
    expect(runnerOf("not a url")).toBeNull();
    expect(runnerOf("")).toBeNull();
    expect(runnerOf(null)).toBeNull();
    expect(runnerOf(undefined)).toBeNull();
  });
});

describe("isLoopback", () => {
  it("is true only for this computer", () => {
    expect(isLoopback("http://localhost:11434")).toBe(true);
    expect(isLoopback("http://LOCALHOST:11434")).toBe(true);
    expect(isLoopback("http://127.0.0.1:11434")).toBe(true);
    expect(isLoopback("http://127.1:11434")).toBe(true);
    expect(isLoopback("http://[::1]:8080")).toBe(true);
  });

  it("never mistakes a name or another machine for this one", () => {
    expect(isLoopback("http://127.example.com:11434")).toBe(false);
    expect(isLoopback("http://localhost.example.com:11434")).toBe(false);
    expect(isLoopback("http://192.168.1.20:11434")).toBe(false);
    expect(isLoopback("https://my-server.tailnet.ts.net")).toBe(false);
    expect(isLoopback("not a url")).toBe(false);
    expect(isLoopback(null)).toBe(false);
  });

  it("words where a model runs from it", () => {
    expect(whereOf("http://127.0.0.1:11434")).toBe("on this computer");
    expect(whereOf("http://192.168.1.20:11434")).toBe("on your model server");
    expect(whereOf(null)).toBe("on your model server");
  });
});
