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
// Ollama DOES accept `hf.co/<repo>:<quant>` — that is what the Download button pulls since #793 —
// but only the catalogue's per-quant tag is byte-verified against the HF tree, so this DISPLAY
// helper must never re-derive one (workbenchGuide.ts documents the rule; the verified tag arrives
// as data). LM Studio's `lms get` documentation still never says an HF repo id is accepted, so LM
// Studio gets no command at all. A copy box with a command that silently does nothing — or fetches
// an unverified file — is the outcome being guarded against.

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

  it("tells every runner on every platform whether it keeps running", () => {
    // The question item 2 was about. The answer used to leak into whichever field the author was
    // writing — Ollama's `bestFor` on a SHARED base, llama-server's `caveat`, LM Studio's copy not
    // at all — so it went out platform-blind in one place and unsaid in another.
    for (const platform of PLATFORMS) {
      for (const g of runnerGuides(platform)) {
        expect(
          g.lifecycle.trim(),
          `${g.name} on ${platform} says nothing about staying running`,
        ).not.toBe("");
      }
    }
  });

  it("never promises autostart from a field that is shared across platforms", () => {
    // `bestFor` sits on the shared `base`, so anything platform-specific written into it is emitted
    // identically on all three. It used to say Ollama "starts with your machine and stays running",
    // which is true of the ollama.com app and false of `brew install ollama` (the formula installs
    // the server and starts nothing). The claim belongs in `lifecycle`, which is per-platform.
    for (const platform of PLATFORMS) {
      for (const g of runnerGuides(platform)) {
        expect(g.bestFor, `${g.name} on ${platform}`).not.toMatch(/starts with your machine/i);
        expect(g.bestFor, `${g.name} on ${platform}`).not.toMatch(/stays running/i);
      }
    }
  });

  it("gives the Linux context change a command that actually changes something", () => {
    // It named a drop-in path and nothing else — not the file's contents, not `daemon-reload`. A
    // user who followed it literally created nothing and restarted nothing. Ollama's own FAQ
    // documents `systemctl edit`, and that is now what PM says.
    const linux = runnerGuides("linux").find((g) => g.name === "Ollama");
    const ctx = linux?.steps.find((step) => step.includes("OLLAMA_CONTEXT_LENGTH"));
    expect(ctx).toMatch(/systemctl edit/);
    expect(ctx).toMatch(/daemon-reload/);
  });

  it("stops offering `ollama serve` on Linux, where the service already holds the port", () => {
    // The install script enables the systemd unit, so 11434 is taken and this fails with a port
    // clash. Verified on a packaged install: `systemctl is-enabled ollama` → enabled.
    const linux = runnerGuides("linux").find((g) => g.name === "Ollama");
    for (const step of linux?.steps ?? []) expect(step).not.toMatch(/ollama serve/);
  });

  it("tells Windows users to quit Ollama before setting the variable", () => {
    // Setting it under a running Ollama does nothing, and the step used to say only "restart it" —
    // which reads as "restart afterwards" and leaves the change inert. The FAQ is explicit.
    const win = runnerGuides("windows").find((g) => g.name === "Ollama");
    const ctx = win?.steps.find((step) => step.includes("OLLAMA_CONTEXT_LENGTH"));
    expect(ctx).toMatch(/quit ollama/i);
  });

  it("never states a flat context default, on any platform", () => {
    // Ollama chooses it from VRAM — under 24 GB → 4k, 24-48 → 32k, above → 256k. Saying "Ollama
    // serves a 4,096-token context by default" was a claim about the user's machine PM had not made.
    for (const platform of PLATFORMS) {
      const ollama = runnerGuides(platform).find((g) => g.name === "Ollama");
      const ctx = ollama?.steps.find((step) => step.includes("OLLAMA_CONTEXT_LENGTH"));
      expect(ctx, `Ollama on ${platform}`).not.toMatch(/by default/i);
      expect(ctx, `Ollama on ${platform}`).toMatch(/24 GB/);
    }
  });

  it("keeps `caveat` meaning a hardware exclusion, not a lifecycle note", () => {
    // llama-server's caveat was "it runs only while its terminal window is open" — a lifecycle fact
    // wearing the label every other runner uses for "this hardware can't run it". That ambiguity is
    // why LM Studio's lifecycle went unsaid: its caveat slot was already spent on hardware.
    for (const platform of PLATFORMS) {
      const llama = runnerGuides(platform).find((g) => g.name === "llama-server");
      expect(llama?.caveat, `llama-server on ${platform}`).toBeNull();
      expect(llama?.lifecycle).toMatch(/terminal window is open/);
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
