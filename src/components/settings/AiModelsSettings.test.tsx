// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The cost summary on AI & Models. Its first read is what refreshes OpenRouter's prices, which the
// backend does only once an OpenRouter key is stored (`ensure_pricing_fresh`). So a key saved here
// for the first time must read the summary again, or the person who just set OpenRouter up sees no
// prices until they next open the tab.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { costSummary, setOpenRouterKey, setOpenRouterBackgroundKey } = vi.hoisted(() => ({
  costSummary: vi.fn(),
  setOpenRouterKey: vi.fn(),
  setOpenRouterBackgroundKey: vi.fn(),
}));

const SETTINGS = {
  chat_models: [],
  background_models: [],
  chat_auto_switch: false,
  background_auto_switch: false,
};

// Spread over the real module, so a wrapper this tab doesn't call here stays as it is.
vi.mock("../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  costSummary: () => costSummary(),
  hasOpenRouterKey: () => Promise.resolve(false),
  hasOpenRouterBackgroundKey: () => Promise.resolve(false),
  getSettings: () => Promise.resolve(SETTINGS),
  settingsDefaults: () => Promise.resolve(SETTINGS),
  getLocalAiSwitch: () => Promise.reject(new Error("not in this test")),
  setOpenRouterKey: (key: string) => setOpenRouterKey(key),
  setOpenRouterBackgroundKey: (key: string) => setOpenRouterBackgroundKey(key),
}));
vi.mock("../ModelListEditor", () => ({ ModelListEditor: () => null }));
// The primitives read the theme; the ThemeProvider itself pulls in IPC.
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { AiModelsSettings } from "./AiModelsSettings";

beforeEach(() => {
  costSummary.mockReturnValue(new Promise(() => {}));
  setOpenRouterKey.mockResolvedValue(undefined);
  setOpenRouterBackgroundKey.mockResolvedValue(undefined);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

/** Type `value` into the main key field (`which` 0) or the background one (1), then click away. */
async function saveIn(which: 0 | 1, value: string) {
  const field = (await screen.findAllByPlaceholderText("sk-or-..."))[which];
  fireEvent.change(field, { target: { value } });
  fireEvent.blur(field);
}

describe("the cost summary on AI & Models", () => {
  it("reads it once on opening", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
  });

  it("reads it again once a first OpenRouter key is saved", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(0, "sk-or-v1-first");
    await waitFor(() => expect(setOpenRouterKey).toHaveBeenCalledWith("sk-or-v1-first"));
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(2));
  });

  it("reads it again once a background-only key is saved", async () => {
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(1, "sk-or-v1-background");
    await waitFor(() =>
      expect(setOpenRouterBackgroundKey).toHaveBeenCalledWith("sk-or-v1-background"),
    );
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(2));
  });

  it("does not read it again when saving the key failed", async () => {
    setOpenRouterKey.mockRejectedValue(new Error("keychain locked"));
    render(<AiModelsSettings />);
    await waitFor(() => expect(costSummary).toHaveBeenCalledTimes(1));
    await saveIn(0, "sk-or-v1-first");
    await waitFor(() => expect(screen.getByText(/keychain locked/)).toBeTruthy());
    expect(costSummary).toHaveBeenCalledTimes(1);
  });
});
