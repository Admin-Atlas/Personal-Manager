// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Settings and the Local AI switch (D7): the Local AI tab exists only while it is on, a reader on
// it when it goes off lands on AI & Models, and onboarding's "On this device" choice is what turns
// it on for a brand-new user (and finishing with the cloud instead turns it back off).

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalAiSwitch } from "../lib/types";

const getLocalAiSwitch = vi.fn();
const setLocalAiSwitch = vi.fn();
const setLocalLlmRouting = vi.fn();
// Hoisted with the mock, which runs `ok` while building the module.
const { calls, ok } = vi.hoisted(() => {
  const calls: string[] = [];
  const ok =
    (name: string) =>
    (...args: unknown[]) => {
      calls.push(`${name}(${args.map((a) => JSON.stringify(a)).join(",")})`);
      return Promise.resolve();
    };
  return { calls, ok };
});

// Spread over the real module: some of the tabs SettingsView imports bind IPC wrappers at import
// time, and none of those is called by what these tests render.
vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getLocalAiSwitch: () => getLocalAiSwitch(),
  setLocalAiSwitch: (on: boolean) => {
    calls.push(`setLocalAiSwitch(${on})`);
    return setLocalAiSwitch(on);
  },
  setLocalLlmRouting: (role: string, pref: string) => {
    calls.push(`setLocalLlmRouting("${role}","${pref}")`);
    return setLocalLlmRouting(role, pref);
  },
  hasOpenRouterKey: () => Promise.resolve(false),
  getSettings: () =>
    Promise.resolve({
      chat_models: [],
      background_models: [],
      chat_auto_switch: false,
      background_auto_switch: false,
      time_zone: "UTC",
      indexing_speed: "fast",
    }),
  languageOptions: () => Promise.reject(new Error("not in this test")),
  vaultStatus: () => Promise.resolve({ mode: "device" }),
  setTimeZone: ok("setTimeZone"),
  setChatModels: ok("setChatModels"),
  setBackgroundModels: ok("setBackgroundModels"),
  setChatAutoSwitch: ok("setChatAutoSwitch"),
  setBackgroundAutoSwitch: ok("setBackgroundAutoSwitch"),
  setOpenRouterKey: ok("setOpenRouterKey"),
  setOnboardingDone: ok("setOnboardingDone"),
  setIndexingSpeed: ok("setIndexingSpeed"),
  setVaultEmbedder: ok("setVaultEmbedder"),
}));

// The tabs' own contents have their own tests; here they only need to say which one is showing.
vi.mock("./localai/LocalAiSettings", () => ({ LocalAiSettings: () => <p>Local AI tab</p> }));
vi.mock("./settings/AiModelsSettings", () => ({ AiModelsSettings: () => <p>AI & Models tab</p> }));
vi.mock("./settings/GeneralSettings", () => ({ GeneralSettings: () => <p>General tab</p> }));
vi.mock("./onboarding/OnboardingLocalConnect", () => ({
  OnboardingLocalConnect: () => <p>Connect your local server</p>,
}));
vi.mock("./ModelListEditor", () => ({ ModelListEditor: () => null }));
vi.mock("./SignInStrip", () => ({ SignInStrip: () => null }));
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { SettingsView } from "./SettingsView";
import { announceLocalAiSwitch } from "../lib/localAiSwitch";

const sw = (on: boolean): LocalAiSwitch => ({
  on,
  chat_needs_model: false,
  background_needs_model: false,
  server_saved: false,
});

beforeEach(() => {
  // jsdom has no layout, and the Settings dialog's scroll-spy watches the pane's size.
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  // Nor scrolling: a tab change scrolls the pane back to its top.
  if (!Element.prototype.scrollTo) Element.prototype.scrollTo = () => {};
  calls.length = 0;
  getLocalAiSwitch.mockReset();
  setLocalAiSwitch.mockReset();
  setLocalLlmRouting.mockReset();
  setLocalLlmRouting.mockResolvedValue(undefined);
});
afterEach(cleanup);

const railItem = (name: string) =>
  screen.queryAllByRole("button").find((b) => b.textContent?.trim() === name) ?? null;

describe("the Settings rail", () => {
  it("has no Local AI tab while the switch is off, and has one while it is on", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(false));
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    await waitFor(() => expect(railItem("AI & Models")).not.toBeNull());
    expect(railItem("Local AI")).toBeNull();

    act(() => announceLocalAiSwitch(sw(true)));
    expect(railItem("Local AI")).not.toBeNull();
  });

  it("moves a reader on the Local AI tab to AI & Models when the switch goes off", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(true));
    render(<SettingsView onClose={() => {}} onboarding={false} />);
    await waitFor(() => expect(railItem("Local AI")).not.toBeNull());
    fireEvent.click(railItem("Local AI")!);
    await waitFor(() => expect(screen.getByText("Local AI tab")).toBeTruthy());

    act(() => announceLocalAiSwitch(sw(false)));
    expect(screen.queryByText("Local AI tab")).toBeNull();
    expect(screen.getByText("AI & Models tab")).toBeTruthy();
    expect(railItem("Local AI")).toBeNull();
  });
});

describe("onboarding's provider choice", () => {
  it("switches Local AI on before the local pane mounts", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(false));
    setLocalAiSwitch.mockResolvedValue(sw(true));
    render(<SettingsView onClose={() => {}} onboarding />);
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "On this device" }));
    await waitFor(() => expect(screen.getByText("Connect your local server")).toBeTruthy());
    expect(calls).toContain("setLocalAiSwitch(true)");
  });

  it("finishing with the cloud after trying local hands both roles to the cloud, then switches off", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(false));
    setLocalAiSwitch.mockImplementation((on: boolean) => Promise.resolve(sw(on)));
    const onClose = vi.fn();
    render(<SettingsView onClose={onClose} onboarding />);
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "On this device" }));
    await waitFor(() => expect(screen.getByText("Connect your local server")).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Cloud (OpenRouter)" }));
    fireEvent.change(screen.getByPlaceholderText("sk-or-..."), { target: { value: "sk-or-x" } });
    fireEvent.click(screen.getByRole("button", { name: "Get started" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());

    const routeChat = calls.indexOf('setLocalLlmRouting("chat","cloud")');
    const routeBg = calls.indexOf('setLocalLlmRouting("background","cloud")');
    const off = calls.indexOf("setLocalAiSwitch(false)");
    expect(routeChat).toBeGreaterThan(-1);
    expect(routeBg).toBeGreaterThan(-1);
    // Routing first, while the switch is still on; off then empties the local models.
    expect(off).toBeGreaterThan(Math.max(routeChat, routeBg));
  });

  it("leaves an existing local user's switch alone", async () => {
    getLocalAiSwitch.mockResolvedValue(sw(true));
    const onClose = vi.fn();
    render(<SettingsView onClose={onClose} onboarding />);
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    fireEvent.click(screen.getByRole("button", { name: "On this device" }));
    await waitFor(() => expect(screen.getByText("Connect your local server")).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Cloud (OpenRouter)" }));
    fireEvent.click(screen.getByRole("button", { name: "Set up AI later" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(calls.filter((c) => c.startsWith("setLocalAiSwitch"))).toEqual([]);
    expect(calls.filter((c) => c.startsWith("setLocalLlmRouting"))).toEqual([]);
  });
});
