// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The webview's copy of the voice switch: read fresh on each mount, moved by every announcement,
// and re-read after a switch that failed, because switching off can fail after it has turned voice
// off (the removal is what failed).

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { VoiceSwitch } from "./types";

const getVoiceSwitch = vi.fn();
const setVoiceSwitch = vi.fn();
vi.mock("./ipc", () => ({
  getVoiceSwitch: () => getVoiceSwitch(),
  setVoiceSwitch: (on: boolean, removeEngine: boolean) => setVoiceSwitch(on, removeEngine),
}));

import { setVoiceEnabled, useVoiceSwitch, voiceShown } from "./voiceSwitch";

beforeEach(() => {
  getVoiceSwitch.mockReset();
  setVoiceSwitch.mockReset();
});
afterEach(cleanup);

describe("useVoiceSwitch", () => {
  it("is unknown until the backend answers, then the stored answer", async () => {
    getVoiceSwitch.mockResolvedValue({ on: true });
    const { result } = renderHook(() => useVoiceSwitch());
    expect(result.current).toBeNull();
    expect(voiceShown(result.current)).toBe(false);
    await waitFor(() => expect(result.current).toEqual({ on: true }));
    expect(voiceShown(result.current)).toBe(true);
  });

  it("stays unknown when the backend can't answer, which hides the mic", async () => {
    getVoiceSwitch.mockRejectedValue("the vault is locked");
    const { result } = renderHook(() => useVoiceSwitch());
    await waitFor(() => expect(getVoiceSwitch).toHaveBeenCalled());
    expect(result.current).toBeNull();
    expect(voiceShown(result.current)).toBe(false);
  });

  it("follows a switch made elsewhere, and a late first read can't undo it", async () => {
    let answer: (s: VoiceSwitch) => void = () => {};
    getVoiceSwitch.mockReturnValue(new Promise<VoiceSwitch>((r) => (answer = r)));
    setVoiceSwitch.mockResolvedValue({ on: true });
    const { result } = renderHook(() => useVoiceSwitch());

    await act(async () => {
      await setVoiceEnabled(true);
    });
    expect(result.current).toEqual({ on: true });
    await act(async () => answer({ on: false }));
    expect(result.current).toEqual({ on: true });
  });
});

describe("setVoiceEnabled", () => {
  it("passes the removal choice through and announces the answer", async () => {
    getVoiceSwitch.mockResolvedValue({ on: true });
    setVoiceSwitch.mockResolvedValue({ on: false });
    const { result } = renderHook(() => useVoiceSwitch());
    await waitFor(() => expect(result.current).toEqual({ on: true }));

    await act(async () => {
      await setVoiceEnabled(false, true);
    });
    expect(setVoiceSwitch).toHaveBeenCalledWith(false, true);
    expect(result.current).toEqual({ on: false });
    // On never removes anything.
    setVoiceSwitch.mockResolvedValue({ on: true });
    await act(async () => {
      await setVoiceEnabled(true);
    });
    expect(setVoiceSwitch).toHaveBeenLastCalledWith(true, false);
  });

  it("re-reads after a failure, so a removal that failed still shows voice off", async () => {
    getVoiceSwitch.mockResolvedValue({ on: true });
    const { result } = renderHook(() => useVoiceSwitch());
    await waitFor(() => expect(result.current).toEqual({ on: true }));

    setVoiceSwitch.mockRejectedValue("Voice input is off, but PM couldn't remove its engine");
    getVoiceSwitch.mockResolvedValue({ on: false });
    await act(async () => {
      await expect(setVoiceEnabled(false, true)).rejects.toMatch(/couldn't remove/);
    });
    expect(result.current).toEqual({ on: false });
  });

  it("an install that failed leaves voice where it was", async () => {
    getVoiceSwitch.mockResolvedValue({ on: false });
    const { result } = renderHook(() => useVoiceSwitch());
    await waitFor(() => expect(result.current).toEqual({ on: false }));

    setVoiceSwitch.mockRejectedValue("could not download");
    await act(async () => {
      await expect(setVoiceEnabled(true)).rejects.toMatch(/download/);
    });
    expect(result.current).toEqual({ on: false });
  });
});
