// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The webview's copy of the Local AI switch: read fresh on each mount, moved by every announcement,
// and never by a switch that failed.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalAiSwitch } from "./types";

const getLocalAiSwitch = vi.fn();
const setLocalAiSwitch = vi.fn();
vi.mock("./ipc", () => ({
  getLocalAiSwitch: () => getLocalAiSwitch(),
  setLocalAiSwitch: (on: boolean) => setLocalAiSwitch(on),
}));

import {
  localAiShown,
  refreshLocalAiSwitch,
  setLocalAiEnabled,
  useLocalAiSwitch,
} from "./localAiSwitch";

const sw = (over: Partial<LocalAiSwitch> = {}): LocalAiSwitch => ({
  on: true,
  chat_needs_model: false,
  background_needs_model: false,
  server_saved: true,
  ...over,
});

beforeEach(() => {
  getLocalAiSwitch.mockReset();
  setLocalAiSwitch.mockReset();
});
afterEach(cleanup);

describe("useLocalAiSwitch", () => {
  it("is unknown until the backend answers, then the stored answer", async () => {
    getLocalAiSwitch.mockResolvedValue(sw({ on: false }));
    const { result } = renderHook(() => useLocalAiSwitch());
    expect(result.current).toBeNull();
    await waitFor(() => expect(result.current).toEqual(sw({ on: false })));
  });

  it("stays unknown when the backend can't answer, which hides Local AI", async () => {
    getLocalAiSwitch.mockRejectedValue("the vault is locked");
    const { result } = renderHook(() => useLocalAiSwitch());
    await waitFor(() => expect(getLocalAiSwitch).toHaveBeenCalled());
    expect(result.current).toBeNull();
    expect(localAiShown(result.current)).toBe(false);
  });

  it("follows a switch made elsewhere, and a late first read can't undo it", async () => {
    let answer: (s: LocalAiSwitch) => void = () => {};
    getLocalAiSwitch.mockReturnValue(new Promise<LocalAiSwitch>((r) => (answer = r)));
    setLocalAiSwitch.mockResolvedValue(sw({ on: false, chat_needs_model: true }));
    const { result } = renderHook(() => useLocalAiSwitch());

    await act(async () => {
      await setLocalAiEnabled(false);
    });
    expect(result.current).toEqual(sw({ on: false, chat_needs_model: true }));
    // The read that started before the switch lands last, with the old answer.
    await act(async () => answer(sw({ on: true })));
    expect(result.current?.on).toBe(false);
  });

  it("announces nothing when the switch fails, so nothing moves", async () => {
    getLocalAiSwitch.mockResolvedValue(sw({ on: true }));
    setLocalAiSwitch.mockRejectedValue("database lock poisoned");
    const { result } = renderHook(() => useLocalAiSwitch());
    await waitFor(() => expect(result.current?.on).toBe(true));
    await act(async () => {
      await expect(setLocalAiEnabled(false)).rejects.toBe("database lock poisoned");
    });
    expect(result.current?.on).toBe(true);
  });

  it("re-reads and announces after a write that changes what the switch reports", async () => {
    getLocalAiSwitch.mockResolvedValueOnce(sw({ on: false, chat_needs_model: true }));
    const { result } = renderHook(() => useLocalAiSwitch());
    await waitFor(() => expect(result.current?.chat_needs_model).toBe(true));
    getLocalAiSwitch.mockResolvedValueOnce(sw({ on: false, chat_needs_model: false }));
    await act(async () => {
      await refreshLocalAiSwitch();
    });
    expect(result.current?.chat_needs_model).toBe(false);
  });
});

describe("localAiShown", () => {
  it("shows Local AI only when the switch says on", () => {
    expect(localAiShown(sw({ on: true }))).toBe(true);
    expect(localAiShown(sw({ on: false }))).toBe(false);
    expect(localAiShown(null)).toBe(false);
  });
});
