// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The runner guides are copy, but two things about them are load-bearing enough to pin.
//
// The ports must match the ones the BACKEND probes (`local_ai.rs` PROBE_PORTS: 11434 Ollama, 1234
// LM Studio, 8080 llama-server). A guide that tells you to connect to a port PM never looks at is
// worse than no guide, and nothing else would catch a drift between the two lists.
//
// And `installCommand` must keep refusing to invent a command. Only llama-server documents taking a
// Hugging Face repo id (`-hf <user>/<model>[:quant]`), which is exactly what the catalogue stores.
// Ollama names models its own way and LM Studio's `lms get` documentation never says an HF repo id
// is accepted — so PM prints neither. The temptation to "helpfully" add them is the thing being
// guarded against: a copy box with a command that silently does nothing is the worst outcome here.

import { describe, expect, it } from "vitest";

import type { SetupPlatform } from "./setupGuide";
import { installCommand, runnerGuides } from "./workbenchGuide";

const PLATFORMS: SetupPlatform[] = ["windows", "mac", "linux"];

describe("runnerGuides", () => {
  it("covers all three runners PM auto-detects, on their real ports", () => {
    const guides = runnerGuides("linux");
    expect(guides.map((g) => g.name)).toEqual(["Ollama", "LM Studio", "llama-server"]);
    // These are the ports `local_ai.rs` actually probes. Drift here and the copy sends people to a
    // port PM never checks.
    expect(guides.map((g) => g.port)).toEqual([11434, 1234, 8080]);
  });

  it("gives every runner real steps and a reason to pick it, on every platform", () => {
    for (const platform of PLATFORMS) {
      for (const g of runnerGuides(platform)) {
        expect(g.steps.length, `${g.name} on ${platform} has no steps`).toBeGreaterThan(0);
        for (const step of g.steps) expect(step.trim()).not.toBe("");
        // `bestFor` and `models` are what turn a list of three installers into a choice.
        expect(g.bestFor.trim(), `${g.name} on ${platform}`).not.toBe("");
        expect(g.models.trim(), `${g.name} on ${platform}`).not.toBe("");
        expect(g.url).toMatch(/^https:\/\//);
      }
    }
  });

  it("states LM Studio's hardware exclusion on macOS, where it actually rules people out", () => {
    // The one hard exclusion among the three: LM Studio does not support Intel Macs. A user on one
    // must not be sent down a path that cannot work.
    const mac = runnerGuides("mac").find((g) => g.name === "LM Studio");
    expect(mac?.caveat).toMatch(/Apple Silicon/i);
  });
});

describe("installCommand", () => {
  const repo = "bartowski/Qwen2.5-7B-Instruct-GGUF";

  it("gives llama-server the documented -hf form, with the quant PM picked", () => {
    expect(installCommand("llama-server", repo, "Q4_K_M")).toBe(`llama-server -hf ${repo}:Q4_K_M`);
    // The quant is optional in llama.cpp's own syntax, so an unscored model still gets a command.
    expect(installCommand("llama-server", repo, null)).toBe(`llama-server -hf ${repo}`);
  });

  it("invents nothing for the two runners that don't take a Hugging Face repo id", () => {
    expect(installCommand("ollama", repo, "Q4_K_M")).toBeNull();
    expect(installCommand("lmstudio", repo, "Q4_K_M")).toBeNull();
  });
});
