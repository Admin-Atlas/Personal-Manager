// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The failed-install path. On Windows the updater plugin hides every window and drops the tray
// BEFORE it finds out the installer won't launch, then (since 2.11) throws into a PM that is still
// running.
// If the hook reported that failure without first bringing the main window back, the banner would
// render into a hidden window and PM would sit there with nothing on screen and no tray icon. So:
// the window is shown first, the failure is only reported after, and a failure to show never hides
// the install failure.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const install = vi.fn<() => Promise<void>>();
const showMainWindow = vi.fn<() => Promise<void>>();
const relaunch = vi.fn(async () => undefined);

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn(async () => ({
    version: "3.140.0-alpha",
    download: vi.fn(async () => undefined),
    install: () => install(),
  })),
}));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: () => relaunch() }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(async () => "3.139.4-alpha") }));
vi.mock("./ipc", () => ({
  smartAppControlState: vi.fn(async () => "off"),
  packageManagedLinux: vi.fn(async () => false),
  showMainWindow: () => showMainWindow(),
}));

import { useUpdater } from "./useUpdater";

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  install.mockRejectedValue(new Error("the installer would not launch"));
  showMainWindow.mockResolvedValue(undefined);
});
afterEach(cleanup);

/** Mount the hook and wait until the update is downloaded and waiting for a restart. */
async function staged() {
  const hook = renderHook(() => useUpdater());
  await waitFor(() => expect(hook.result.current.status).toBe("ready"));
  return hook;
}

describe("useUpdater restart", () => {
  it("shows the main window before it reports a failed install", async () => {
    let shown!: () => void;
    showMainWindow.mockImplementation(() => new Promise<void>((resolve) => (shown = resolve)));
    const { result } = await staged();

    act(() => result.current.restart());
    await waitFor(() => expect(showMainWindow).toHaveBeenCalledTimes(1));
    // The window is still on its way back: nothing may be reported into it yet.
    await act(async () => {});
    expect(result.current.installFailed).toBe(false);
    expect(result.current.status).toBe("installing");

    await act(async () => shown());
    await waitFor(() => expect(result.current.installFailed).toBe(true));
    expect(result.current.status).toBe("ready");
    expect(relaunch).not.toHaveBeenCalled();
  });

  it("still reports the failed install when the window can't be shown", async () => {
    showMainWindow.mockRejectedValue(new Error("no window"));
    const { result } = await staged();

    act(() => result.current.restart());
    await waitFor(() => expect(result.current.installFailed).toBe(true));
    expect(showMainWindow).toHaveBeenCalledTimes(1);
    expect(result.current.status).toBe("ready");
  });

  it("leaves the window alone when the install goes through", async () => {
    install.mockResolvedValue(undefined);
    const { result } = await staged();

    act(() => result.current.restart());
    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1));
    expect(showMainWindow).not.toHaveBeenCalled();
    expect(result.current.installFailed).toBe(false);
  });
});
