// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Already downloaded" list (#449) only ever contains models NO endpoint is serving — local_ai.rs
// filters with `!already_served`. That makes "you can't pick these yet" a GATING hint rather than
// prose, so the settings doctrine keeps it unfolded and visible. These pin that it is actually
// rendered, that it says the right half depending on whether an endpoint exists, and that it stays
// out of the way when there is nothing on disk to explain.
//
// The last suite covers what that framing MISSED: the list being empty is not the same as having
// nothing downloaded, and a folder PM cannot read is not a folder that is absent.

import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { LocalOnDiskModel, LocalRecommendations } from "../../lib/types";
import { DownloadedModels } from "./LocalAiSettings";

// `useTheme` is stubbed so the section's <Button>s don't need the full ThemeProvider, matching
// ConnectorItemRow.test.tsx.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

const MODEL: LocalOnDiskModel = {
  name: "gemma-3-4b-it-Q4_K_M",
  source: "lm_studio",
  path: "/models/gemma-3-4b-it-Q4_K_M.gguf",
  size_gb: 2.5,
  sidecar_gb: 0,
  quant: "Q4_K_M",
  shards: 1,
  matched_repo: "google/gemma-3-4b-it",
  fit: {
    verdict: "comfortable",
    quant: "Q4_K_M",
    context: 8192,
    kv: "f16",
    est_memory_gb: 3.4,
    est_tokens_per_sec: 40,
    notes: [],
  },
};

function recs(over: Partial<LocalRecommendations> = {}): LocalRecommendations {
  return {
    hardware: {
      platform: "windows",
      total_ram_gb: 32,
      available_ram_gb: 20,
      cpu_brand: null,
      cpu_cores: null,
      cpu_threads: null,
      disk_free_gb: null,
      gpu_name: null,
      gpu_vendor: null,
      vram_gb: null,
      vram_source: null,
      gpu_bandwidth_gbps: null,
      unified_memory: false,
      is_wsl: false,
      notes: [],
    },
    reserve_gb: 2,
    gpu_reserve_gb: 1,
    catalog_version: 1,
    catalog_generated_at: "2026-07-26",
    endpoint_configured: true,
    cadence: "monthly",
    rescan_due: false,
    curated: [],
    installed: [],
    on_disk: [MODEL],
    disk_sources_present: ["lm_studio"],
    disk_blocked: [],
    endpoint_inventory: null,
    co_residency: null,
    disk_found: 1,
    disk_truncated: false,
    scan_dir: null,
    terms_accepted: [],
    ...over,
  };
}

const noop = () => {};

describe("DownloadedModels — the unserved gating hint", () => {
  it("says a downloaded model isn't assignable until its server serves it", () => {
    const { container } = render(
      <DownloadedModels recs={recs()} configured onPickFolder={noop} onClearFolder={noop} />,
    );
    expect(container.textContent).toContain("None of these can be assigned yet");
    expect(container.textContent).toContain("Assign roles");
    // The model itself still renders — the hint explains the list, it doesn't replace it.
    expect(container.textContent).toContain("gemma-3-4b-it-Q4_K_M");
  });

  it("points at connecting an endpoint first when there isn't one", () => {
    const { container } = render(
      <DownloadedModels
        recs={recs()}
        configured={false}
        onPickFolder={noop}
        onClearFolder={noop}
      />,
    );
    expect(container.textContent).toContain("Connect an endpoint");
    expect(container.textContent).not.toContain("None of these can be assigned yet");
  });

  it("stays silent when nothing was found on disk", () => {
    const { container } = render(
      <DownloadedModels
        recs={recs({ on_disk: [] })}
        configured
        onPickFolder={noop}
        onClearFolder={noop}
      />,
    );
    expect(container.textContent).not.toContain("can be assigned");
    expect(container.textContent).not.toContain("Connect an endpoint");
  });
});

describe("a runner that is installed but empty", () => {
  // #790's sibling. `on_disk` has already had everything the endpoint serves removed from it, so an
  // empty list cannot tell "no runner on this machine" from "a runner with nothing in it" — and the
  // second is exactly what a user sees the moment they remove their last model, or the first time
  // they install Ollama. It used to be told, wrongly, that everything it had was already served.
  const render3 = (over: Partial<LocalRecommendations>) =>
    render(
      <DownloadedModels
        recs={recs({ on_disk: [], ...over })}
        configured
        onPickFolder={noop}
        onClearFolder={noop}
      />,
    );

  it("says the folder is empty rather than claiming everything is already served", () => {
    const { container } = render3({ disk_sources_present: ["lm_studio"], disk_found: 0 });
    expect(container.textContent).toContain("but nothing downloaded into it yet");
    expect(container.textContent).not.toContain("already being served");
  });

  it("still says 'already being served' when the crawl did find something", () => {
    const { container } = render3({ disk_sources_present: ["lm_studio"], disk_found: 3 });
    expect(container.textContent).toContain("already being served");
    expect(container.textContent).not.toContain("but nothing downloaded into it yet");
  });

  it("still says no folder at all when no runner is present", () => {
    const { container } = render3({ disk_sources_present: [], disk_found: 0 });
    expect(container.textContent).toContain("No model folder found");
  });
});

describe("a store PM is not allowed to read", () => {
  // The defect Bobby hit. A packaged Linux Ollama runs as its own user with home /usr/share/ollama
  // at mode 0700, so the crawl gets EACCES — and `Path::is_dir()`, which every root probe used to
  // be, reports that identically to "not there". The panel then told a machine that was serving two
  // models out of that very store that it had no model folder for Ollama at all.
  const render4 = (over: Partial<LocalRecommendations>) =>
    render(
      <DownloadedModels
        recs={recs({ on_disk: [], disk_sources_present: [], disk_found: 0, ...over })}
        configured
        onPickFolder={noop}
        onClearFolder={noop}
      />,
    );

  it("names the store and its path instead of reporting an absence", () => {
    const { container } = render4({
      disk_blocked: [{ source: "ollama", path: "/usr/share/ollama/.ollama/models" }],
    });
    expect(container.textContent).toContain("/usr/share/ollama/.ollama/models");
    expect(container.textContent).not.toContain("No model folder found");
  });

  it("never tells anyone to loosen a service account's permissions", () => {
    // Suggesting a chmod on a system service's home to make a settings panel count files would be a
    // bad trade, and connecting the server gets the same answer for free.
    const { container } = render4({
      disk_blocked: [{ source: "ollama", path: "/usr/share/ollama/.ollama/models" }],
    });
    expect(container.textContent).not.toMatch(/chmod|chown|sudo|permissions to/i);
  });

  it("says what the endpoint holds rather than counting what nothing serves", () => {
    // With an Ollama endpoint, `/v1/models` lists what has been PULLED, so everything downloaded is
    // also served and the unserved list is structurally empty forever. Reading that as "you have
    // nothing downloaded" is the same lie by a different route.
    const { container } = render4({ endpoint_inventory: 2 });
    expect(container.textContent).toContain("Your server has 2 models");
    expect(container.textContent).not.toContain("No model folder found");
  });

  it("tells a first-time installer their server is simply empty", () => {
    const { container } = render4({ endpoint_inventory: 0 });
    expect(container.textContent).toContain("nothing has been downloaded into it yet");
    expect(container.textContent).not.toContain("No model folder found");
  });
});
