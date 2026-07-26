// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The milestone sort is stored per project under the key that used to hold ONE shared choice, so the
// parts worth pinning are the isolation between projects, that a pre-upgrade value is honoured rather
// than discarded, and that junk falls back rather than throwing.

import { describe, it, expect, beforeEach } from "vitest";
import { readMilestoneSort, writeMilestoneSort } from "./milestonePrefs";

const SORT_KEY = "pm.milestones.sort";

beforeEach(() => {
  localStorage.clear();
});

describe("per-project milestone sort", () => {
  it("defaults to deadline ascending", () => {
    expect(readMilestoneSort("Anything")).toEqual({ key: "deadline", dir: "asc" });
  });

  it("keeps each project's choice apart", () => {
    writeMilestoneSort("Roof", { key: "manual", dir: "asc" });
    writeMilestoneSort("Taxes", { key: "label", dir: "desc" });
    expect(readMilestoneSort("Roof")).toEqual({ key: "manual", dir: "asc" });
    expect(readMilestoneSort("Taxes")).toEqual({ key: "label", dir: "desc" });
    // A project that has never been sorted is unaffected by either.
    expect(readMilestoneSort("Garden")).toEqual({ key: "deadline", dir: "asc" });
  });

  it("honours a pre-upgrade shared value for every project until one is set", () => {
    // The old shape: a bare {key,dir} that applied everywhere. Discarding it would silently reset
    // the sort of someone who had deliberately chosen one.
    localStorage.setItem(SORT_KEY, JSON.stringify({ key: "label", dir: "desc" }));
    expect(readMilestoneSort("Roof")).toEqual({ key: "label", dir: "desc" });
    expect(readMilestoneSort("Taxes")).toEqual({ key: "label", dir: "desc" });
  });

  it("does not let one project's new choice leak into another", () => {
    localStorage.setItem(SORT_KEY, JSON.stringify({ key: "label", dir: "desc" }));
    writeMilestoneSort("Roof", { key: "manual", dir: "asc" });
    expect(readMilestoneSort("Roof")).toEqual({ key: "manual", dir: "asc" });
    // Once the store is a map, an unset project takes the default rather than the old shared value.
    expect(readMilestoneSort("Taxes")).toEqual({ key: "deadline", dir: "asc" });
  });

  it("falls back rather than throwing on junk", () => {
    localStorage.setItem(SORT_KEY, "not json");
    expect(readMilestoneSort("Roof")).toEqual({ key: "deadline", dir: "asc" });
    localStorage.setItem(SORT_KEY, JSON.stringify({ Roof: { key: "nope", dir: "sideways" } }));
    expect(readMilestoneSort("Roof")).toEqual({ key: "deadline", dir: "asc" });
  });
});
