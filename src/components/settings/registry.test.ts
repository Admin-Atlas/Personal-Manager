// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Settings rail with a switched-off feature (Mods spec §2: when off, its UI is gone): the tab
// leaves the rail, and a reader standing on it lands where the way back is.

import { describe, expect, it } from "vitest";

import { fallbackTab, SETTINGS_GROUPS, SETTINGS_TABS, tabShown, visibleGroups } from "./registry";

const ids = (groups: ReturnType<typeof visibleGroups>) =>
  groups.flatMap((g) => g.tabs.map((t) => t.id));

describe("the Settings rail and the Local AI switch", () => {
  it("shows every tab, in order, while Local AI is on", () => {
    expect(ids(visibleGroups(SETTINGS_GROUPS, { localAi: true }))).toEqual(
      SETTINGS_TABS.map((t) => t.id),
    );
  });

  it("leaves only the Local AI tab out while it is off, keeping the AI group and its order", () => {
    const off = visibleGroups(SETTINGS_GROUPS, { localAi: false });
    expect(ids(off)).toEqual(SETTINGS_TABS.map((t) => t.id).filter((id) => id !== "localai"));
    const ai = off.find((g) => g.header === "AI");
    expect(ai?.tabs.map((t) => t.id)).toEqual(["ai", "search"]);
    expect(off).toHaveLength(SETTINGS_GROUPS.length);
  });

  it("drops a group whose every tab is switched off, heading and all", () => {
    const groups = [
      { header: "Only local", tabs: SETTINGS_TABS.filter((t) => t.id === "localai") },
      { header: null, tabs: SETTINGS_TABS.filter((t) => t.id === "general") },
    ];
    expect(visibleGroups(groups, { localAi: false }).map((g) => g.header)).toEqual([null]);
  });

  it("moves a reader off the vanished tab to AI & Models, where the switch's way back on is", () => {
    expect(fallbackTab("localai", SETTINGS_GROUPS, { localAi: false })).toBe("ai");
    // A tab that is still shown stays put, and so does Local AI while it is on.
    expect(fallbackTab("localai", SETTINGS_GROUPS, { localAi: true })).toBe("localai");
    expect(fallbackTab("storage", SETTINGS_GROUPS, { localAi: false })).toBe("storage");
    expect(tabShown("ai", { localAi: false })).toBe(true);
  });

  it("falls back to General when nothing is left in the vanished tab's group", () => {
    const groups = [
      { header: "Only local", tabs: SETTINGS_TABS.filter((t) => t.id === "localai") },
    ];
    expect(fallbackTab("localai", groups, { localAi: false })).toBe("general");
  });
});
