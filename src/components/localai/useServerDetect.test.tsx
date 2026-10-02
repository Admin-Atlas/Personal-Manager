// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The port probe the start card and Model server share. It runs only while nothing is connected —
// once connected the tab reads the status, and a probe answer must never stand in for it — and a
// failed probe is "nothing found", never an error.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DetectedEndpoint } from "../../lib/types";

const probeLocalLlmPorts = vi.fn();

vi.mock("../../lib/ipc", () => ({
  probeLocalLlmPorts: () => probeLocalLlmPorts(),
}));

import { useServerDetect } from "./useServerDetect";

const OLLAMA: DetectedEndpoint = { url: "http://127.0.0.1:11434", label: "Ollama", models: [] };
/** The tab's config read hasn't landed yet. */
const UNREAD: { configured: boolean | null } = { configured: null };

beforeEach(() => {
  vi.clearAllMocks();
  probeLocalLlmPorts.mockResolvedValue([OLLAMA]);
});
afterEach(cleanup);

describe("useServerDetect", () => {
  it("looks on mount while nothing is connected, and forgets the answer once something is", async () => {
    const { result, rerender } = renderHook(({ configured }) => useServerDetect(configured), {
      initialProps: { configured: false },
    });
    await waitFor(() => expect(result.current.detected).toEqual([OLLAMA]));
    expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1);

    rerender({ configured: true });
    expect(result.current.detected).toBeNull();

    // Disconnecting looks again straight away.
    rerender({ configured: false });
    await waitFor(() => expect(probeLocalLlmPorts).toHaveBeenCalledTimes(2));
  });

  it("never probes while connected", async () => {
    renderHook(() => useServerDetect(true));
    await act(async () => {});
    expect(probeLocalLlmPorts).not.toHaveBeenCalled();
  });

  it("never probes before the tab knows whether anything is connected", async () => {
    // The tab mounts before its config read lands. Taking that moment for "not connected" sent a
    // long-connected user's every visit a probe of three local ports, its answer thrown away.
    const { result, rerender } = renderHook(
      ({ configured }: { configured: boolean | null }) => useServerDetect(configured),
      { initialProps: UNREAD },
    );
    await act(async () => {});
    expect(probeLocalLlmPorts).not.toHaveBeenCalled();
    expect(result.current.detected).toBeNull();
    rerender({ configured: true });
    await act(async () => {});
    expect(probeLocalLlmPorts).not.toHaveBeenCalled();
  });

  it("looks as soon as the read says nothing is connected", async () => {
    const { result, rerender } = renderHook(
      ({ configured }: { configured: boolean | null }) => useServerDetect(configured),
      { initialProps: UNREAD },
    );
    rerender({ configured: false });
    await waitFor(() => expect(result.current.detected).toEqual([OLLAMA]));
    expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1);
  });

  it("still looks when asked before it knows, and keeps the answer off the list", async () => {
    const { result } = renderHook(() => useServerDetect(null));
    let found: DetectedEndpoint[] = [];
    await act(async () => {
      found = await result.current.detect();
    });
    expect(found).toEqual([OLLAMA]);
    expect(result.current.detected).toBeNull();
  });

  it("shares a look already running instead of starting a second", async () => {
    let resolve: (v: DetectedEndpoint[]) => void = () => {};
    probeLocalLlmPorts.mockReturnValue(new Promise((r) => (resolve = r)));
    const { result } = renderHook(() => useServerDetect(false));
    expect(result.current.detecting).toBe(true);
    const again = result.current.detect();
    expect(probeLocalLlmPorts).toHaveBeenCalledTimes(1);
    await act(async () => resolve([OLLAMA]));
    await expect(again).resolves.toEqual([OLLAMA]);
    expect(result.current.detecting).toBe(false);
  });

  it("reads a failed probe as nothing found", async () => {
    probeLocalLlmPorts.mockRejectedValue(new Error("no route"));
    const { result } = renderHook(() => useServerDetect(false));
    await waitFor(() => expect(result.current.detected).toEqual([]));
  });

  it("drops an answer that lands after connecting", async () => {
    let resolve: (v: DetectedEndpoint[]) => void = () => {};
    probeLocalLlmPorts.mockReturnValue(new Promise((r) => (resolve = r)));
    const { result, rerender } = renderHook(({ configured }) => useServerDetect(configured), {
      initialProps: { configured: false },
    });
    rerender({ configured: true });
    await act(async () => resolve([OLLAMA]));
    expect(result.current.detected).toBeNull();
  });
});
