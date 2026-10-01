// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The release settings left the lifecycle section for a hook the whole tab shares, and picked up the
// one job no section could do on its own: noticing that what the server holds has changed without
// anything on the page being touched. These pin that job — re-read on the status fields that mean
// "memory changed hands", and not on the ones that re-arrive with every push — plus the two things
// the move must not lose: an instance handed nothing to do asks nothing, and a failed write says
// which setting it was.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { INERT_POWER_VIEW } from "../../lib/powerRoute";
import type { LocalGpuResidency, LocalLlmStatus } from "../../lib/types";

const localGpuResidency = vi.fn();
const releaseLocalGpu = vi.fn();
const getLocalReleasePolicy = vi.fn();
const setLocalReleasePolicy = vi.fn();

vi.mock("../../lib/ipc", () => ({
  localGpuResidency: () => localGpuResidency(),
  releaseLocalGpu: () => releaseLocalGpu(),
  getLocalReleasePolicy: () => getLocalReleasePolicy(),
  setLocalReleasePolicy: (...a: unknown[]) => setLocalReleasePolicy(...a),
}));

import { useReleaseSettings } from "./useReleaseSettings";

const residency = (over: Partial<LocalGpuResidency> = {}): LocalGpuResidency => ({
  resident: [],
  vram_gb: 8,
  dgpu_displays: [],
  policy: "server",
  idle_minutes: 5,
  no_unload_route: false,
  ...over,
});

const status = (over: Partial<LocalLlmStatus> = {}): LocalLlmStatus => ({
  configured: true,
  reachable: true,
  in_cooldown: false,
  cooldown_remaining_s: 0,
  probed_now: false,
  chat_local_model: "tiny-chat:1b",
  background_local_model: null,
  served_window: null,
  served_window_proven: false,
  window_source: null,
  chat_answering: false,
  background_answering: false,
  chat_loaded: false,
  background_loaded: null,
  chat_released: false,
  background_released: false,
  power: INERT_POWER_VIEW,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  localGpuResidency.mockResolvedValue(residency());
  releaseLocalGpu.mockResolvedValue(0);
  getLocalReleasePolicy.mockResolvedValue({
    policy: "server",
    idle_minutes: 5,
    battery_idle_minutes: 0,
  });
  setLocalReleasePolicy.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("useReleaseSettings", () => {
  it("has no residency answer until the first read lands", async () => {
    // undefined is "not asked yet", which is neither null ("couldn't ask") nor [] ("holds nothing").
    localGpuResidency.mockReturnValue(new Promise(() => {}));
    const { result } = renderHook(() => useReleaseSettings({ status: null }));
    expect(result.current.residency).toBeUndefined();

    cleanup();
    localGpuResidency.mockResolvedValue(residency({ resident: null }));
    const second = renderHook(() => useReleaseSettings({ status: null }));
    await waitFor(() => expect(second.result.current.residency).not.toBeUndefined());
    expect(second.result.current.residency?.resident).toBeNull();
  });

  it("re-reads what the server holds when the status says memory changed hands", async () => {
    // The stale readout this exists for: a test loads the model, and the section went on saying the
    // graphics card was free, because nothing re-read it after mount.
    const { rerender } = renderHook(({ s }) => useReleaseSettings({ status: s }), {
      initialProps: { s: status() },
    });
    await waitFor(() => expect(localGpuResidency).toHaveBeenCalledTimes(1));

    // A push that changes nothing about memory — the window, here — is not a reason to ask again.
    rerender({ s: status({ served_window: 8192, served_window_proven: true }) });
    await act(async () => {});
    expect(localGpuResidency).toHaveBeenCalledTimes(1);

    rerender({ s: status({ chat_loaded: true }) });
    await waitFor(() => expect(localGpuResidency).toHaveBeenCalledTimes(2));

    // A call starting or ending is NOT a reason: each read is a request to the user's server, and
    // what a call loads is reported by `chat_loaded` when it completes.
    rerender({ s: status({ chat_loaded: true, chat_answering: true }) });
    await act(async () => {});
    expect(localGpuResidency).toHaveBeenCalledTimes(2);

    // A release counts.
    rerender({ s: status({ chat_loaded: false, chat_released: true }) });
    await waitFor(() => expect(localGpuResidency).toHaveBeenCalledTimes(3));
  });

  it("asks nothing at all when it isn't the instance in use", async () => {
    // A section handed the tab's instance still has to call its own (hooks can't be conditional).
    // That one must stay silent, or every read and every re-read happens twice.
    const { rerender } = renderHook(({ s }) => useReleaseSettings({ status: s, enabled: false }), {
      initialProps: { s: status() },
    });
    rerender({ s: status({ chat_loaded: true }) });
    await act(async () => {});
    expect(localGpuResidency).not.toHaveBeenCalled();
    expect(getLocalReleasePolicy).not.toHaveBeenCalled();
  });

  it("says which setting a failed write was for", async () => {
    // The policy and the battery row can be shown in different sections, and each must say its own
    // "couldn't save that" — never the other one's.
    const { result } = renderHook(() => useReleaseSettings({ status: null }));
    await waitFor(() => expect(result.current.policy).toBe("server"));

    setLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    act(() => result.current.change("idle", 5));
    await waitFor(() =>
      expect(result.current.saveError).toEqual({ field: "policy", kind: "restored" }),
    );
    expect(result.current.policy).toBe("server");

    setLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    getLocalReleasePolicy.mockRejectedValueOnce(new Error("vault locked"));
    act(() => result.current.changeBatteryIdle(5));
    await waitFor(() =>
      expect(result.current.saveError).toEqual({ field: "battery", kind: "unknown" }),
    );
    expect(result.current.batteryIdle).toBeNull();
  });
});
