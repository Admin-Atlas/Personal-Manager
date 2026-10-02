// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Your machine" is what every fit and PM's pick were worked out from, so it shows the figures they
// were worked out with: the free memory read when the list was sized, and a bandwidth said to be the
// card's published one — not a "~" that reads as a measurement.

import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { LocalRecommendations } from "../../lib/types";

const theme = vi.hoisted(() => ({ depth: "standard" }));
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: theme.depth,
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

import { LocalAiMachine } from "./LocalAiMachine";

afterEach(() => {
  cleanup();
  theme.depth = "standard";
});

const recs = (hw: Partial<LocalRecommendations["hardware"]> = {}): LocalRecommendations => ({
  hardware: {
    platform: "linux",
    total_ram_gb: 32,
    available_ram_gb: 20,
    cpu_brand: "Test CPU",
    cpu_cores: 8,
    cpu_threads: 16,
    disk_free_gb: 200,
    gpu_name: "RTX 5060 Laptop",
    gpu_vendor: "nvidia",
    vram_gb: 8,
    vram_source: "nvidia-smi",
    gpu_bandwidth_gbps: 384,
    unified_memory: false,
    is_wsl: false,
    notes: [],
    ...hw,
  },
  reserve_gb: 2,
  gpu_reserve_gb: 1,
  catalog_version: 4,
  catalog_generated_at: "2026-09-30",
  endpoint_configured: false,
  cadence: "monthly",
  rescan_due: false,
  curated: [],
  installed: [],
  on_disk: [],
  disk_sources_present: [],
  disk_blocked: [],
  endpoint_inventory: null,
  co_residency: null,
  disk_found: 0,
  disk_truncated: false,
  scan_dir: null,
  terms_accepted: [],
  live_available_ram_gb: 12,
});

const show = (r: LocalRecommendations) =>
  render(<LocalAiMachine recs={r} loading={false} rescanning={false} onRescan={() => {}} />)
    .container;

describe("Your machine", () => {
  it("shows the free memory the fits were sized against, and the bandwidth as published", () => {
    const text = show(recs()).textContent ?? "";
    expect(text).toContain("12.0 GB free of 32.0 GB");
    expect(text).toContain("384 GB/s published");
    expect(text).not.toMatch(/~\d+ GB\/s/);
    expect(text).not.toContain("PM's model list: version");
  });

  it("says PM puts no speed on shared memory", () => {
    expect(show(recs({ unified_memory: true })).textContent).toContain(
      "PM doesn't estimate speed on chips that share memory with the processor yet",
    );
  });

  it("names the model list's version and date at Power depth", () => {
    theme.depth = "power";
    expect(show(recs()).textContent).toContain("PM's model list: version 4, built 30-09-2026.");
  });
});
