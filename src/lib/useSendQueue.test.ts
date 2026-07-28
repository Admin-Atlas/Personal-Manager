// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The type-ahead queue's promises (#152) — every one of them invisible, and one of them the reason
// the backend hasn't fallen over:
//
//   - sends are SERIALISED: never two user turns in flight, whatever the user does with the keyboard.
//     `chat::assert_user_turn_allowed` refuses a second consecutive user turn, so a queue that raced
//     would not merely look wrong, it would error at the write layer;
//   - a failure STOPS the queue with the failed message back at the head — order preserved, nothing
//     lost, and no burning the whole queue against a dead key one message at a time;
//   - the cap is on what's WAITING, so the message in flight never costs a slot;
//   - clearing discards without sending, for when the user changes conversation.

import { act, renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { QUEUE_LIMIT, useSendQueue } from "./useSendQueue";

/** A send whose completion the test controls, recording call order and concurrency. */
function controllableSend() {
  const calls: string[] = [];
  const resolvers: Array<(ok: boolean) => void> = [];
  let inFlight = 0;
  let maxInFlight = 0;
  const send = vi.fn(async (text: string) => {
    calls.push(text);
    inFlight += 1;
    maxInFlight = Math.max(maxInFlight, inFlight);
    const ok = await new Promise<boolean>((resolve) => resolvers.push(resolve));
    inFlight -= 1;
    return ok;
  });
  return {
    send,
    calls,
    /** Complete the oldest outstanding send. */
    settle: async (ok = true) => {
      const resolve = resolvers.shift();
      if (!resolve) throw new Error("nothing in flight to settle");
      await act(async () => {
        resolve(ok);
      });
    },
    get maxInFlight() {
      return maxInFlight;
    },
  };
}

describe("serialisation", () => {
  // THE invariant. Three messages typed faster than PM can answer must still reach the backend one
  // at a time, in order — a second concurrent user turn is refused at the write layer.
  it("never has two sends in flight, however fast messages arrive", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));

    act(() => {
      result.current.enqueue("first");
      result.current.enqueue("second");
      result.current.enqueue("third");
    });
    expect(c.calls).toEqual(["first"]);
    expect(result.current.queued.map((m) => m.text)).toEqual(["second", "third"]);

    await c.settle();
    expect(c.calls).toEqual(["first", "second"]);
    await c.settle();
    expect(c.calls).toEqual(["first", "second", "third"]);
    await c.settle();

    expect(c.maxInFlight).toBe(1);
    expect(result.current.queued).toEqual([]);
  });

  // The message being sent is already on screen as the user's own bubble; listing it as "queued"
  // too would read as having said it twice.
  it("shows only what is still waiting, not what is in flight", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("waiting");
    });
    expect(result.current.queued.map((m) => m.text)).toEqual(["waiting"]);
    await c.settle();
    await c.settle();
  });

  it("starts draining again after going idle", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("one");
    });
    await c.settle();
    expect(result.current.queued).toEqual([]);
    // The drain loop exited; a later message must restart it rather than sit there forever.
    act(() => {
      result.current.enqueue("two");
    });
    expect(c.calls).toEqual(["one", "two"]);
    await c.settle();
  });
});

describe("a failure", () => {
  it("stops the queue and keeps the failed message at the head", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("fails");
      result.current.enqueue("after");
    });

    await c.settle(false);
    // Not sent, not dropped, not reordered — and "after" never went out on top of a broken turn.
    expect(c.calls).toEqual(["fails"]);
    expect(result.current.queued.map((m) => m.text)).toEqual(["fails", "after"]);
    expect(result.current.stalled).toBe(true);
  });

  it("resumes from the message that failed, in order", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("fails");
      result.current.enqueue("after");
    });
    await c.settle(false);

    act(() => {
      result.current.resume();
    });
    expect(result.current.stalled).toBe(false);
    expect(c.calls).toEqual(["fails", "fails"]);
    await c.settle();
    expect(c.calls).toEqual(["fails", "fails", "after"]);
    await c.settle();
  });

  it("clears the stall when the failed message is taken back", async () => {
    // Nothing is blocking the queue any more, so a warning that says otherwise describes a state
    // that no longer exists.
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("fails");
    });
    await c.settle(false);
    expect(result.current.stalled).toBe(true);

    act(() => {
      result.current.remove(result.current.queued[0].id);
    });
    expect(result.current.queued).toEqual([]);
    expect(result.current.stalled).toBe(false);
  });
});

describe("editing a waiting message", () => {
  it("sends the edited text, not the original", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("teh typo");
    });

    act(() => {
      result.current.edit(result.current.queued[0].id, "the typo, fixed");
    });
    expect(result.current.queued.map((m) => m.text)).toEqual(["the typo, fixed"]);

    await c.settle();
    await c.settle();
    expect(c.calls).toEqual(["in flight", "the typo, fixed"]);
  });

  // THE reason `hold` exists. Without it a reply landing mid-edit dispatches the pre-edit text and
  // the row disappears from under the cursor: the correction discarded, the version being corrected
  // sent anyway.
  it("does not send while an editor is open, then sends the edit", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("being edited");
    });

    act(() => {
      result.current.hold(true);
    });
    await c.settle(); // the reply lands mid-edit
    expect(c.calls).toEqual(["in flight"]);
    expect(result.current.queued.map((m) => m.text)).toEqual(["being edited"]);

    act(() => {
      result.current.edit(result.current.queued[0].id, "edited in time");
      result.current.hold(false);
    });
    expect(c.calls).toEqual(["in flight", "edited in time"]);
    await c.settle();
  });

  it("treats an emptied message as taking it back", async () => {
    // A blank message is not something to send; queueing one would just be rejected downstream.
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("never mind");
    });
    act(() => {
      result.current.edit(result.current.queued[0].id, "   ");
    });
    expect(result.current.queued).toEqual([]);
    await c.settle();
    expect(c.calls).toEqual(["in flight"]);
  });

  it("cannot rewrite a message that has already gone", async () => {
    // A sent message can't be un-said, and silently rewriting the NEXT one instead would be worse
    // than doing nothing.
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("first");
      result.current.enqueue("second");
    });
    const goneId = result.current.queued[0].id - 1; // the in-flight one's id
    act(() => {
      result.current.edit(goneId, "too late");
    });
    await c.settle();
    await c.settle();
    expect(c.calls).toEqual(["first", "second"]);
  });

  it("does not strand a hold when the conversation changes", async () => {
    // A hold belongs to an editor that is going away with the chat; leaving it set would freeze the
    // next conversation's queue with nothing on screen to explain why.
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("being edited");
      result.current.hold(true);
    });
    act(() => {
      result.current.clear();
    });
    await c.settle();

    act(() => {
      result.current.enqueue("new chat");
    });
    expect(c.calls).toEqual(["in flight", "new chat"]);
    await c.settle();
  });
});

describe("the cap", () => {
  it("counts what is waiting, not what is in flight", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    let accepted: boolean[] = [];
    act(() => {
      // One goes out immediately; QUEUE_LIMIT then fit behind it.
      accepted = Array.from({ length: QUEUE_LIMIT + 1 }, (_, i) => result.current.enqueue(`m${i}`));
    });
    expect(accepted.every(Boolean)).toBe(true);
    expect(result.current.queued.length).toBe(QUEUE_LIMIT);
    expect(result.current.full).toBe(true);

    // One more is refused — reported, never silently swallowed, so the caller can keep the draft.
    let overflow = true;
    act(() => {
      overflow = result.current.enqueue("too many");
    });
    expect(overflow).toBe(false);
    expect(result.current.queued.length).toBe(QUEUE_LIMIT);

    for (let i = 0; i <= QUEUE_LIMIT; i++) await c.settle();
    expect(c.calls).not.toContain("too many");
  });

  it("frees a slot as each message goes out", async () => {
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      for (let i = 0; i <= QUEUE_LIMIT; i++) result.current.enqueue(`m${i}`);
    });
    expect(result.current.full).toBe(true);
    await c.settle();
    expect(result.current.full).toBe(false);
    for (let i = 0; i < QUEUE_LIMIT; i++) await c.settle();
  });
});

describe("leaving the conversation", () => {
  it("discards everything waiting, without sending it", async () => {
    // A queued message was written for the chat that was on screen. Delivering it into whatever the
    // user opened next is worse than losing it.
    const c = controllableSend();
    const { result } = renderHook(() => useSendQueue(c.send));
    act(() => {
      result.current.enqueue("in flight");
      result.current.enqueue("abandoned");
    });

    act(() => {
      result.current.clear();
    });
    expect(result.current.queued).toEqual([]);
    expect(result.current.stalled).toBe(false);

    await c.settle();
    expect(c.calls).toEqual(["in flight"]);
  });
});
