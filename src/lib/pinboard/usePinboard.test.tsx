// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's PERSISTENCE contract — the half of usePinboard that can lose the user's board.
// grid.ts and history.ts are pure and tested directly; this is the part that isn't, and both bugs
// pinned here shipped green because nothing ever ran the hook against a store that says no.
//
// What makes the stakes asymmetric: the board is one JSON blob in the encrypted `settings` table, so
// every write is a FULL overwrite. Being wrong about *when* to write costs the whole board, not one
// field — and the board is hand-made content that exists nowhere else.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { usePinboard } from "./usePinboard";
import { BOARD_VERSION, PINBOARD_PREF_KEY, type Board } from "./types";

const ipc = vi.hoisted(() => ({ getPref: vi.fn(), setPref: vi.fn() }));
vi.mock("../ipc", () => ipc);

/** A stored board with a note in it, so "we wrote the empty default over it" is visible rather than
 *  inferred: any write that doesn't carry this text has destroyed it. */
const STORED: Board = {
  version: BOARD_VERSION,
  widgets: [
    { id: "kept", kind: "note", rect: { x: 0, y: 0, w: 7, h: 5 }, text: "the user's own note" },
  ],
};

/** Let the mocked IPC promises settle. */
const settle = () => act(async () => {});
/** Run out the 500 ms persist debounce, then let the write settle. */
const flushDebounce = async () => {
  act(() => void vi.advanceTimersByTime(600));
  await settle();
};

beforeEach(() => {
  vi.useFakeTimers();
  ipc.getPref.mockReset();
  ipc.setPref.mockReset();
  ipc.setPref.mockResolvedValue(undefined);
});
afterEach(() => vi.useRealTimers());

describe("usePinboard persistence", () => {
  it("NEVER writes a board that failed to load — not on a change, not on unmount", async () => {
    // The regression guard. A failed load leaves the empty default on screen; if that arms
    // persistence, the unmount flush (i.e. switching tabs) silently overwrites the real stored board
    // with nothing. The board is the only copy — there is no vault file, no backup, behind it.
    ipc.getPref.mockRejectedValue(new Error("store not ready"));

    const { result, unmount } = renderHook(() => usePinboard());
    await settle();
    expect(result.current.load).toBe("failed");

    // Editing while the real board is unknown must not commit that guess...
    act(() => void result.current.addNote());
    await flushDebounce();

    // ...and neither must leaving the tab, which is what actually fired in the wild.
    unmount();
    await settle();

    expect(ipc.setPref).not.toHaveBeenCalled();
  });

  it("writes a board that DID load, on a change and on unmount", async () => {
    // The other half of the guard above: proving the refusal is keyed on the failure, not on
    // persistence being broken outright. Without this, "never writes" passes vacuously.
    ipc.getPref.mockResolvedValue(JSON.stringify(STORED));

    const { result, unmount } = renderHook(() => usePinboard());
    await settle();
    expect(result.current.load).toBe("ready");
    expect(result.current.board.widgets).toHaveLength(1);

    act(() => void result.current.addNote());
    await flushDebounce();
    expect(ipc.setPref).toHaveBeenCalledWith(
      PINBOARD_PREF_KEY,
      expect.stringContaining("the user's own note"),
    );

    unmount();
    await settle();
    expect(ipc.setPref).toHaveBeenCalledTimes(2);
  });

  it("treats an empty store as a real answer — a fresh install still saves", async () => {
    // The failure mode of over-correcting the test above: refusing to write whenever the board looks
    // empty would brick every fresh install, where "no board yet" is the truth, not an error.
    ipc.getPref.mockResolvedValue(null);

    const { result } = renderHook(() => usePinboard());
    await settle();
    expect(result.current.load).toBe("ready");

    act(() => void result.current.addNote());
    await flushDebounce();
    expect(ipc.setPref).toHaveBeenCalledTimes(1);
  });

  it("arms persistence on a Retry that succeeds, having refused while it hadn't", async () => {
    ipc.getPref
      .mockRejectedValueOnce(new Error("store not ready"))
      .mockResolvedValueOnce(JSON.stringify(STORED));

    const { result } = renderHook(() => usePinboard());
    await settle();
    expect(result.current.load).toBe("failed");

    act(() => void result.current.retryLoad());
    await settle();
    expect(result.current.load).toBe("ready");
    expect(result.current.board.widgets).toHaveLength(1);

    act(() => void result.current.addNote());
    await flushDebounce();
    expect(ipc.setPref).toHaveBeenCalledTimes(1);
  });

  it("reports a failed write instead of swallowing it, and clears it when one lands", async () => {
    // The second regression guard. The header promises "Saved on this device" — a promise that can
    // quietly be false is worse than none, because the user keeps typing into it.
    ipc.getPref.mockResolvedValue(JSON.stringify(STORED));
    ipc.setPref.mockRejectedValueOnce(new Error("write failed"));

    const { result } = renderHook(() => usePinboard());
    await settle();
    expect(result.current.saveFailed).toBe(false);

    act(() => void result.current.addNote());
    await flushDebounce();
    expect(result.current.saveFailed).toBe(true);

    // Every write sends the whole board, so the next change re-sends what the failed one carried:
    // a transient failure heals itself, and the warning goes away on its own.
    act(() => void result.current.addNote());
    await flushDebounce();
    expect(result.current.saveFailed).toBe(false);
  });
});
