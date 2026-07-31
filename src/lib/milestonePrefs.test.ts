// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The milestone sort is stored per project, so the parts worth pinning are the isolation between
// projects, that a pre-upgrade value is honoured rather than discarded, and that junk falls back
// rather than throwing.
//
// The store moved: the map's KEYS are project names, so it is user content and now lives in the
// encrypted `settings` table under `milestone_ui` rather than in the webview's plaintext
// localStorage. These cases therefore drive the module through the hydrated cache (with `getPref` /
// `setPref` mocked) instead of seeding localStorage. The one case that still seeds localStorage is
// the pre-upgrade shared value — that shape can only arrive through ADOPTION now, which is where it
// moved to, and it must keep working or someone's deliberately chosen sort resets on upgrade.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { getPref, setPref } from "./ipc";
import { __resetStoredPrefs, hydrateStoredPrefs } from "./storedPrefs";
import {
  readMilestoneSort,
  readShowCompletedMilestones,
  writeMilestoneSort,
  writeShowCompletedMilestones,
} from "./milestonePrefs";

vi.mock("./ipc", () => ({
  getPref: vi.fn(async () => null),
  setPref: vi.fn(async () => undefined),
}));

const get = vi.mocked(getPref);
const set = vi.mocked(setPref);

/** Open the session with `milestone_ui` already holding `blob` (omit for an empty store). */
async function hydrateWith(blob?: unknown) {
  get.mockImplementation(async (key: string) =>
    key === "milestone_ui" && blob !== undefined ? JSON.stringify(blob) : null,
  );
  await hydrateStoredPrefs();
}

beforeEach(() => {
  __resetStoredPrefs();
  localStorage.clear();
  get.mockReset();
  set.mockReset();
  get.mockImplementation(async () => null);
  set.mockImplementation(async () => undefined);
});

describe("per-project milestone sort", () => {
  it("defaults to deadline ascending", async () => {
    await hydrateWith();
    expect(readMilestoneSort("Anything")).toEqual({ key: "deadline", dir: "asc" });
  });

  it("keeps each project's choice apart", async () => {
    await hydrateWith();
    writeMilestoneSort("Roof", { key: "manual", dir: "asc" });
    writeMilestoneSort("Taxes", { key: "label", dir: "desc" });
    expect(readMilestoneSort("Roof")).toEqual({ key: "manual", dir: "asc" });
    expect(readMilestoneSort("Taxes")).toEqual({ key: "label", dir: "desc" });
    // A project that has never been sorted is unaffected by either.
    expect(readMilestoneSort("Garden")).toEqual({ key: "deadline", dir: "asc" });
    // And the project names went to the encrypted store, not to localStorage.
    expect(localStorage.getItem("pm.milestones.sort")).toBeNull();
    expect(set).toHaveBeenLastCalledWith(
      "milestone_ui",
      JSON.stringify({
        sort: { Roof: { key: "manual", dir: "asc" }, Taxes: { key: "label", dir: "desc" } },
      }),
    );
  });

  it("honours a pre-upgrade shared value carried across by hydration", async () => {
    // The oldest shape: a bare {key,dir} that applied everywhere, written before the sort was per
    // project. It lived in localStorage, so adoption is the ONLY path it can arrive by now — and
    // discarding it there would silently reset the sort of someone who had deliberately chosen one.
    localStorage.setItem("pm.milestones.sort", JSON.stringify({ key: "label", dir: "desc" }));
    await hydrateWith();

    expect(readMilestoneSort("Roof")).toEqual({ key: "label", dir: "desc" });
    expect(readMilestoneSort("Taxes")).toEqual({ key: "label", dir: "desc" });
    // Carried across verbatim — the reading module, not the adopter, is what understands the shape.
    expect(set).toHaveBeenCalledWith(
      "milestone_ui",
      JSON.stringify({ sort: { key: "label", dir: "desc" } }),
    );
    expect(localStorage.getItem("pm.milestones.sort")).toBeNull();
  });

  it("does not let one project's new choice leak into another", async () => {
    await hydrateWith({ sort: { key: "label", dir: "desc" } });
    writeMilestoneSort("Roof", { key: "manual", dir: "asc" });
    expect(readMilestoneSort("Roof")).toEqual({ key: "manual", dir: "asc" });
    // Once the store is a map, an unset project takes the default rather than the old shared value.
    expect(readMilestoneSort("Taxes")).toEqual({ key: "deadline", dir: "asc" });
  });

  it("falls back rather than throwing on junk", async () => {
    await hydrateWith({ sort: "not an object" });
    expect(readMilestoneSort("Roof")).toEqual({ key: "deadline", dir: "asc" });

    __resetStoredPrefs();
    await hydrateWith({ sort: { Roof: { key: "nope", dir: "sideways" } } });
    expect(readMilestoneSort("Roof")).toEqual({ key: "deadline", dir: "asc" });
  });
});

describe("show completed", () => {
  it("defaults to true and round-trips through the store", async () => {
    await hydrateWith();
    expect(readShowCompletedMilestones()).toBe(true);
    writeShowCompletedMilestones(false);
    expect(readShowCompletedMilestones()).toBe(false);
    expect(set).toHaveBeenLastCalledWith("milestone_ui", JSON.stringify({ showCompleted: false }));
  });

  it("adopts a pre-upgrade localStorage value", async () => {
    localStorage.setItem("pm.milestones.showCompleted", "false");
    await hydrateWith();
    expect(readShowCompletedMilestones()).toBe(false);
    expect(localStorage.getItem("pm.milestones.showCompleted")).toBeNull();
  });

  it("cannot be overwritten by a mount-effect default before hydration lands", async () => {
    // ProjectView writes this unconditionally on mount, with the default. Reaching the store before
    // the stored value arrived would persist `true` over a deliberate `false`.
    writeShowCompletedMilestones(true);
    expect(set).not.toHaveBeenCalled();
    await hydrateWith({ showCompleted: false });
    expect(readShowCompletedMilestones()).toBe(false);
  });
});
