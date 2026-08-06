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

/** A stored board holding a game folder with two notes in it. */
const GAME_BOARD: Board = {
  version: BOARD_VERSION,
  widgets: [
    {
      id: "f",
      kind: "folder",
      rect: { x: 0, y: 0, w: 3, h: 3 },
      game: "roulette",
      gameOn: true,
      children: [
        { id: "a", kind: "note", rect: { x: 0, y: 0, w: 7, h: 5 }, text: "first" },
        { id: "b", kind: "note", rect: { x: 7, y: 0, w: 7, h: 5 }, text: "second" },
      ],
    },
  ],
};

const folderOf = (b: Board) => b.widgets.find((w) => w.id === "f")!;

describe("usePinboard — a game round outlives the app, but not the folder", () => {
  it("keeps a drawn card greyed across a reload, and never re-offers it", async () => {
    // The whole reason the round is stored rather than held in memory: shut the laptop, come back,
    // and the game should not hand you the job you already took.
    ipc.getPref.mockResolvedValue(JSON.stringify(GAME_BOARD));
    const { result, unmount } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    await flushDebounce();
    expect(folderOf(result.current.board).spent).toEqual(["a"]);

    // What actually reached the store is what a relaunch will read back.
    const written = JSON.parse(ipc.setPref.mock.lastCall![1] as string) as Board;
    expect(folderOf(written).spent).toEqual(["a"]);
    unmount();

    ipc.getPref.mockResolvedValue(JSON.stringify(written));
    const second = renderHook(() => usePinboard());
    await settle();
    expect(folderOf(second.result.current.board).spent).toEqual(["a"]);
  });

  it("forgets a card that left the folder while PM was closed", async () => {
    // A card popped out, deleted or dragged away is not "already drawn" — it is gone, and carrying
    // its id would quietly shorten the next round.
    const stale: Board = {
      version: BOARD_VERSION,
      widgets: [{ ...folderOf(GAME_BOARD), spent: ["a", "vanished"] }],
    };
    ipc.getPref.mockResolvedValue(JSON.stringify(stale));
    const { result } = renderHook(() => usePinboard());
    await settle();
    expect(folderOf(result.current.board).spent).toEqual(["a"]);
  });

  it("drops a spent list that isn't a list at all rather than trusting it", async () => {
    const corrupt = JSON.stringify({
      version: BOARD_VERSION,
      widgets: [{ ...folderOf(GAME_BOARD), spent: "a" }],
    });
    ipc.getPref.mockResolvedValue(corrupt);
    const { result } = renderHook(() => usePinboard());
    await settle();
    expect(folderOf(result.current.board).spent).toEqual([]);
  });

  it("empties the round when the last card is drawn, so the next spin starts fresh", async () => {
    ipc.getPref.mockResolvedValue(JSON.stringify(GAME_BOARD));
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    act(() => result.current.drawCard("f", "b"));
    expect(folderOf(result.current.board).spent).toEqual([]);
  });

  it("UNDO of an unrelated edit does not un-grey a card the game already drew", async () => {
    // The reason `carryGameState` exists. History is a stack of whole boards, so without it a
    // Ctrl+Z on a tint would roll the round back with it, mid-game.
    ipc.getPref.mockResolvedValue(JSON.stringify(GAME_BOARD));
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    act(() => result.current.updateWidget("f", { color: "st-due" }));
    act(() => result.current.drawCard("f", "b"));
    // Drawing "b" emptied the round; draw once more so there is something to lose.
    act(() => result.current.drawCard("f", "a"));
    expect(folderOf(result.current.board).spent).toEqual(["a"]);

    act(() => result.current.undo());
    // The tint edit came back out…
    expect(folderOf(result.current.board).color).toBeUndefined();
    // …and the round did NOT come back with it.
    expect(folderOf(result.current.board).spent).toEqual(["a"]);

    act(() => result.current.redo());
    expect(folderOf(result.current.board).color).toBe("st-due");
    expect(folderOf(result.current.board).spent).toEqual(["a"]);
  });

  it("a draw is not itself an undo step — a run through a folder can't bury real edits", async () => {
    ipc.getPref.mockResolvedValue(JSON.stringify(GAME_BOARD));
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.updateWidget("f", { title: "chores" }));
    act(() => result.current.drawCard("f", "a"));

    // One Ctrl+Z takes back the title, not the draw.
    act(() => result.current.undo());
    expect(folderOf(result.current.board).title).toBeUndefined();
  });

  it("moves the winner out to the board when the folder is set to, and doesn't call it spent", async () => {
    ipc.getPref.mockResolvedValue(
      JSON.stringify({
        version: BOARD_VERSION,
        widgets: [{ ...folderOf(GAME_BOARD), autoPopOut: true }],
      }),
    );
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    const board = result.current.board;
    expect(folderOf(board).children?.map((c) => c.id)).toEqual(["b"]);
    expect(board.widgets.find((w) => w.id === "a")).toBeDefined();
    // It left, so it is gone rather than "already drawn" — nothing to grey, nothing to prune later.
    expect(folderOf(board).spent).toEqual([]);
  });

  it("does NOT move out a card you dodged — winning a throw isn't being given work", async () => {
    // The verdict games take cards off the table both ways. A card you beat has had its turn this
    // round, but it is emphatically not a job, so auto pop-out must not fire for it.
    ipc.getPref.mockResolvedValue(
      JSON.stringify({
        version: BOARD_VERSION,
        widgets: [{ ...folderOf(GAME_BOARD), autoPopOut: true }],
      }),
    );
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a", false));
    const board = result.current.board;
    expect(folderOf(board).children?.map((c) => c.id)).toEqual(["a", "b"]);
    expect(board.widgets.find((w) => w.id === "a")).toBeUndefined();
    // It still counts as having had its turn, so the folder isn't offering it again.
    expect(folderOf(board).spent).toEqual(["a"]);
  });

  it("ignores a draw naming a card the folder doesn't hold", async () => {
    ipc.getPref.mockResolvedValue(JSON.stringify(GAME_BOARD));
    const { result } = renderHook(() => usePinboard());
    await settle();

    const before = result.current.board;
    act(() => result.current.drawCard("f", "nope"));
    expect(result.current.board).toBe(before);
  });
});

describe("usePinboard — a folder that repeats keeps no round at all", () => {
  const repeating = JSON.stringify({
    version: BOARD_VERSION,
    widgets: [{ ...folderOf(GAME_BOARD), repeat: true }],
  });

  it("records nothing when it draws, so the same card can come up twice running", async () => {
    ipc.getPref.mockResolvedValue(repeating);
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    act(() => result.current.drawCard("f", "a"));
    expect(folderOf(result.current.board).spent ?? []).toEqual([]);
    // And nothing left the folder either — a draw is a suggestion, not a move.
    expect(folderOf(result.current.board).children?.map((c) => c.id)).toEqual(["a", "b"]);
  });

  it("does not write a board per spin when there is nothing to record", async () => {
    // Every write is a full overwrite of the one stored blob. A game that saved on every press
    // would rewrite the whole board a dozen times an afternoon to say nothing.
    ipc.getPref.mockResolvedValue(repeating);
    const { result } = renderHook(() => usePinboard());
    await settle();

    await flushDebounce();
    const writes = ipc.setPref.mock.calls.length;

    const before = result.current.board;
    act(() => result.current.drawCard("f", "a"));
    act(() => result.current.drawCard("f", "b"));
    // The very same board object, so React never re-renders and the persist effect never runs.
    expect(result.current.board).toBe(before);
    await flushDebounce();
    expect(ipc.setPref.mock.calls.length).toBe(writes);
  });

  it("still moves the winner out when the folder is set to — that is a different promise", async () => {
    ipc.getPref.mockResolvedValue(
      JSON.stringify({
        version: BOARD_VERSION,
        widgets: [{ ...folderOf(GAME_BOARD), repeat: true, autoPopOut: true }],
      }),
    );
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    expect(folderOf(result.current.board).children?.map((c) => c.id)).toEqual(["b"]);
    expect(result.current.board.widgets.find((w) => w.id === "a")).toBeDefined();
    expect(folderOf(result.current.board).spent ?? []).toEqual([]);
  });

  it("clears a round left over from before it stopped keeping one", async () => {
    // Loading is where a stale list would otherwise survive: the folder repeats now, so the ids the
    // old round recorded must not quietly shorten a draw.
    ipc.getPref.mockResolvedValue(
      JSON.stringify({
        version: BOARD_VERSION,
        widgets: [{ ...folderOf(GAME_BOARD), repeat: true, spent: ["a"] }],
      }),
    );
    const { result } = renderHook(() => usePinboard());
    await settle();

    act(() => result.current.drawCard("f", "a"));
    expect(folderOf(result.current.board).spent).toEqual([]);
    // It was never held back in the first place — the list was already being ignored.
    expect(folderOf(result.current.board).children?.map((c) => c.id)).toEqual(["a", "b"]);
  });
});
