// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Chats-tab sidebar fold prefs. The decision worth pinning is the TRI-STATE: absent has to mean
// "never chosen" so the caller keeps its density-derived seed, rather than collapsing to a boolean
// default that would freeze Depth out on a fresh install.

import { describe, it, expect, beforeEach } from "vitest";
import {
  chatSectionsAreDefault,
  readChatSectionOpen,
  resetChatSections,
  writeChatSectionOpen,
} from "./chatPrefs";

beforeEach(() => {
  localStorage.clear();
});

describe("chat section fold prefs", () => {
  it("reports no choice until one is made, so the caller's seed wins", () => {
    expect(readChatSectionOpen("projects")).toBeNull();
    expect(readChatSectionOpen("global")).toBeNull();
    expect(chatSectionsAreDefault()).toBe(true);
  });

  it("remembers a section folded shut", () => {
    // The reported bug: this survived neither a restart nor a tab switch away from Chats.
    writeChatSectionOpen("projects", false);
    expect(readChatSectionOpen("projects")).toBe(false);
    expect(chatSectionsAreDefault()).toBe(false);
  });

  it("keeps the two sections independent", () => {
    writeChatSectionOpen("projects", false);
    expect(readChatSectionOpen("global")).toBeNull();
    writeChatSectionOpen("global", true);
    expect(readChatSectionOpen("projects")).toBe(false);
    expect(readChatSectionOpen("global")).toBe(true);
  });

  it("treats a corrupt value as never-chosen rather than throwing", () => {
    localStorage.setItem("pm.chats.sections", "not json at all");
    expect(readChatSectionOpen("projects")).toBeNull();
    localStorage.setItem("pm.chats.sections", '["projects"]'); // an array, not the record shape
    expect(readChatSectionOpen("projects")).toBeNull();
    localStorage.setItem("pm.chats.sections", '{"projects":"yes"}'); // right shape, wrong type
    expect(readChatSectionOpen("projects")).toBeNull();
  });

  it("resets back to density-derived folding", () => {
    writeChatSectionOpen("projects", false);
    writeChatSectionOpen("global", false);
    resetChatSections();
    expect(chatSectionsAreDefault()).toBe(true);
    expect(readChatSectionOpen("projects")).toBeNull();
    expect(readChatSectionOpen("global")).toBeNull();
  });

  it("announces on the app-wide settings signal so a still-mounted Sidebar follows", () => {
    let heard = 0;
    const bump = () => void heard++;
    window.addEventListener("pm:settings-changed", bump);
    writeChatSectionOpen("projects", false);
    resetChatSections();
    window.removeEventListener("pm:settings-changed", bump);
    expect(heard).toBe(2);
  });
});
