// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Chats-tab sidebar fold prefs. The decision worth pinning is the TRI-STATE: absent has to mean
// "never chosen" so the caller keeps its density-derived seed, rather than collapsing to a boolean
// default that would freeze Depth out on a fresh install.

import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  chatSectionsAreDefault,
  readChatSectionOpen,
  readShowThinking,
  resetChatSections,
  resetShowThinking,
  showThinkingIsDefault,
  writeChatSectionOpen,
  writeShowThinking,
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

// The Thinking toggle. Off is #852's fast path (thinking switched off on Ollama), so the read must
// land there on every doubt: absent, a value this code never writes, or storage that throws.
describe("the chat Thinking toggle pref", () => {
  const KEY = "pm.chat.showThinking";

  it("is off until turned on", () => {
    expect(readShowThinking()).toBe(false);
    expect(showThinkingIsDefault()).toBe(true);
  });

  it('reads on only for the "1" it writes', () => {
    localStorage.setItem(KEY, "1");
    expect(readShowThinking()).toBe(true);
    for (const other of ["true", "0", "x"]) {
      localStorage.setItem(KEY, other);
      expect(readShowThinking(), other).toBe(false);
    }
  });

  it("reads off when storage throws", () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    try {
      expect(readShowThinking()).toBe(false);
    } finally {
      spy.mockRestore();
    }
  });

  it('writes on as "1", off as no key at all, and announces each', () => {
    let heard = 0;
    const bump = () => void heard++;
    window.addEventListener("pm:settings-changed", bump);
    writeShowThinking(true);
    expect(localStorage.getItem(KEY)).toBe("1");
    expect(heard).toBe(1);
    expect(showThinkingIsDefault()).toBe(false);
    writeShowThinking(false);
    // Removed, not "0": absent IS the default, so a reset and an off are the same state.
    expect(localStorage.getItem(KEY)).toBeNull();
    expect(heard).toBe(2);
    expect(showThinkingIsDefault()).toBe(true);
    window.removeEventListener("pm:settings-changed", bump);
  });

  it("resets by removing the key, and announces", () => {
    writeShowThinking(true);
    let heard = 0;
    const bump = () => void heard++;
    window.addEventListener("pm:settings-changed", bump);
    resetShowThinking();
    window.removeEventListener("pm:settings-changed", bump);
    expect(localStorage.getItem(KEY)).toBeNull();
    expect(readShowThinking()).toBe(false);
    expect(heard).toBe(1);
  });
});
