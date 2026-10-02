// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The download moved from the catalogue section into a hook the whole tab holds, so more than one
// control can start it. The flows it already had are pinned through the tab (LocalAiSettings.test);
// these pin what the move added: an error names the section that asked, a finished download is
// remembered and a cancelled one isn't, and an adopted download keeps the backend's start time.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalRecommendation } from "../../lib/types";

const activeLocalPull = vi.fn();
const cancelLocalPull = vi.fn();
const pullLocalModel = vi.fn();
const acceptLocalModelTerms = vi.fn();

vi.mock("../../lib/ipc", () => ({
  activeLocalPull: () => activeLocalPull(),
  cancelLocalPull: () => cancelLocalPull(),
  pullLocalModel: (...a: unknown[]) => pullLocalModel(...a),
  acceptLocalModelTerms: (...a: unknown[]) => acceptLocalModelTerms(...a),
}));

import { usePull } from "./usePull";

const TAG = "hf.co/acme/tiny-chat-GGUF:Q4_K_M";

/** Only the licence decides anything in the hook, so only the licence is filled in. */
const rec = (open: boolean) =>
  ({
    repo: "acme/tiny-chat-GGUF",
    display_name: "Tiny Chat",
    licence: {
      id: open ? "mit" : "acme-terms",
      name: open ? "MIT License" : "Acme Model Terms",
      url: "https://example.com/terms",
      open,
      summary: "Invented terms for a test.",
    },
  }) as LocalRecommendation;

const onError = vi.fn();
const mount = () =>
  renderHook(() =>
    usePull({
      recs: null,
      onRecs: () => {},
      onReload: () => Promise.resolve(),
      onRefreshRecs: () => Promise.resolve(),
      onError,
    }),
  );

beforeEach(() => {
  vi.clearAllMocks();
  activeLocalPull.mockResolvedValue(null);
  cancelLocalPull.mockResolvedValue(true);
  pullLocalModel.mockResolvedValue(undefined);
});
afterEach(cleanup);

describe("usePull", () => {
  it("routes a refusal to the section that asked for the download", async () => {
    pullLocalModel.mockRejectedValue(new Error("a model download is already running"));
    const { result } = mount();
    act(() => result.current.requestPull(rec(true), TAG, "start"));
    await waitFor(() =>
      expect(onError).toHaveBeenCalledWith("Error: a model download is already running", "start"),
    );
  });

  it("carries the asker through the licence dialog", async () => {
    acceptLocalModelTerms.mockRejectedValue(new Error("db is locked"));
    const { result } = mount();
    act(() => result.current.requestPull(rec(false), TAG, "start"));
    expect(result.current.termsFor?.origin).toBe("start");
    expect(pullLocalModel).not.toHaveBeenCalled();

    await act(() => result.current.acceptTermsAndPull());
    expect(onError).toHaveBeenCalledWith("Error: db is locked", "start");
    expect(pullLocalModel).not.toHaveBeenCalled();
  });

  it("remembers a download that finished, and not one that was cancelled", async () => {
    const first = mount();
    act(() => first.result.current.requestPull(rec(true), TAG));
    await waitFor(() => expect(first.result.current.lastPulledTag).toBe(TAG));
    expect(first.result.current.pulling).toBeNull();

    cleanup();
    // A cancelled pull resolves like a finished one; only the Cancel tells them apart.
    let finish: () => void = () => {};
    pullLocalModel.mockReturnValue(
      new Promise<void>((resolve) => {
        finish = () => resolve();
      }),
    );
    const second = mount();
    act(() => second.result.current.requestPull(rec(true), TAG));
    await waitFor(() => expect(second.result.current.pulling).toBe(TAG));
    act(() => second.result.current.cancel());
    await act(async () => finish());
    expect(cancelLocalPull).toHaveBeenCalledTimes(1);
    expect(second.result.current.pulling).toBeNull();
    expect(second.result.current.lastPulledTag).toBeNull();
  });

  it("dates a download it adopted from the backend's own start, not from the mount", async () => {
    // The tab router unmounts on every switch, so "since this view mounted" would restart the clock
    // every time someone looked away.
    activeLocalPull.mockResolvedValue({
      model: TAG,
      status: "downloading",
      completed_bytes: 1024,
      total_bytes: 4096,
      running: true,
      error: null,
      started_at_ms: 1_700_000_000_000,
    });
    const { result } = mount();
    await waitFor(() => expect(result.current.pulling).toBe(TAG));
    expect(result.current.startedAt).toBe(1_700_000_000_000);
  });

  it("reports an adopted download's failure as nobody's request", async () => {
    // No section asked for it here, so the tab decides where it is said.
    activeLocalPull.mockResolvedValueOnce({
      model: TAG,
      status: "downloading",
      completed_bytes: 1024,
      total_bytes: 4096,
      running: true,
      error: null,
      started_at_ms: 1_700_000_000_000,
    });
    activeLocalPull.mockResolvedValue({
      model: TAG,
      status: "downloading",
      completed_bytes: 1024,
      total_bytes: 4096,
      running: false,
      error: "couldn't download the model (disk full)",
      started_at_ms: 1_700_000_000_000,
    });
    const { result } = mount();
    await waitFor(() => expect(result.current.pulling).toBe(TAG));
    // The 1 s snapshot poll is what sees the end of a download this view didn't start.
    await waitFor(
      () => expect(onError).toHaveBeenCalledWith("couldn't download the model (disk full)", null),
      { timeout: 3000 },
    );
    expect(result.current.pulling).toBeNull();
    expect(result.current.lastPulledTag).toBeNull();
  });
});
