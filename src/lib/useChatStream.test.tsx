// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The provider-honesty state added in PR6: `fallback` (transient, current-conversation only) and the
// `providers` map (committed on `done`). Drives the streamed events through a captured `sendMessage`
// callback — no Tauri — and pins the clear semantics (dismiss/next-send clear `fallback` but keep
// `providers`) and the leave-the-conversation guard. The chat Thinking toggle's state rides the
// same harness below: `streamingThought` (transient) and `thoughts` (committed on `done`).

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatEvent } from "./types";

const h = vi.hoisted(() => ({
  onEvent: null as ((e: ChatEvent) => void) | null,
  resolve: null as (() => void) | null,
}));

vi.mock("./capabilities", () => ({ useDevMode: () => ({ devMode: false }) }));

vi.mock("./ipc", () => ({
  sendMessage: vi.fn((_c: number, _t: string, onEvent: (e: ChatEvent) => void) => {
    h.onEvent = onEvent;
    return new Promise<void>((res) => {
      h.resolve = res;
    });
  }),
}));

import { sendMessage } from "./ipc";
import { useChatStream } from "./useChatStream";

const done = (over: Partial<Extract<ChatEvent, { type: "done" }>> = {}): ChatEvent => ({
  type: "done",
  message_id: 42,
  content: "yo",
  citations: [],
  served_by: "cloud",
  on_battery: false,
  ...over,
});

describe("useChatStream provider honesty", () => {
  beforeEach(() => {
    h.onEvent = null;
    h.resolve = null;
  });

  it("captures a fallback for the current conversation and commits the provider on done", () => {
    const current = 1;
    const { result } = renderHook(() => useChatStream(() => current));

    act(() => {
      void result.current.send(1, "hi");
    });
    act(() => h.onEvent!({ type: "token", text: "y" }));
    act(() =>
      h.onEvent!({ type: "fallback", from_model: "llama", to_model: "gpt", reason: "cooldown" }),
    );
    expect(result.current.fallback).toEqual({
      from_model: "llama",
      to_model: "gpt",
      reason: "cooldown",
    });

    act(() => h.onEvent!(done({ message_id: 42, served_by: "cloud" })));
    expect(result.current.providers[42]).toBe("cloud");
    // `done` must NOT clear the strip — the user still needs to see it.
    expect(result.current.fallback).not.toBeNull();
  });

  it("dismiss clears the strip but keeps the committed providers", () => {
    const current = 1;
    const { result } = renderHook(() => useChatStream(() => current));
    act(() => {
      void result.current.send(1, "hi");
    });
    act(() =>
      h.onEvent!({
        type: "fallback",
        from_model: "a",
        to_model: "b",
        reason: "hard_failure:timeout",
      }),
    );
    act(() => h.onEvent!(done({ message_id: 7, served_by: "cloud" })));

    act(() => result.current.dismissFallback());
    expect(result.current.fallback).toBeNull();
    expect(result.current.providers[7]).toBe("cloud");
  });

  it("writes nothing for events on a conversation the user has left", () => {
    const current = 1; // showing conversation 1...
    const { result } = renderHook(() => useChatStream(() => current));
    act(() => {
      void result.current.send(2, "hi"); // ...but this reply is for conversation 2
    });
    act(() => h.onEvent!({ type: "fallback", from_model: "a", to_model: "b", reason: "cooldown" }));
    act(() => h.onEvent!(done({ message_id: 9, served_by: "local" })));

    expect(result.current.fallback).toBeNull();
    expect(result.current.providers[9]).toBeUndefined();
  });

  it("records a power-routed turn as such, and never as a fallback", () => {
    // The On battery policy (#432) is a choice the user made, not something that went wrong: the
    // footer words it, and the warning strip — the failure family's surface — stays down.
    const current = 1;
    const { result } = renderHook(() => useChatStream(() => current));
    act(() => {
      void result.current.send(1, "hi");
    });
    act(() => h.onEvent!(done({ message_id: 42, served_by: "cloud", on_battery: true })));

    expect(result.current.providers[42]).toBe("cloud-on-battery");
    expect(result.current.fallback).toBeNull();
  });
});

describe("useChatStream thinking", () => {
  const T0 = new Date("2026-10-02T10:00:00Z").getTime();

  beforeEach(() => {
    h.onEvent = null;
    h.resolve = null;
    localStorage.clear();
    vi.mocked(sendMessage).mockClear();
    vi.useFakeTimers();
    vi.setSystemTime(T0);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  /** A hook showing conversation 1, with one send to it in flight. */
  function sending() {
    const { result } = renderHook(() => useChatStream(() => 1));
    act(() => {
      void result.current.send(1, "hi");
    });
    return result;
  }

  /** `done` for a reply the local model gave: the only kind that carries thinking. */
  const answered = (message_id = 42) => done({ message_id, served_by: "local" });

  it("builds the live thought from thinking events and never touches the answer", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "Let me check " }));
    vi.setSystemTime(T0 + 3000);
    act(() => h.onEvent!({ type: "thinking", text: "the invoice." }));

    expect(result.current.streamingThought).toEqual({
      text: "Let me check the invoice.",
      startedAt: T0, // the FIRST thinking event, not the latest
      answeredAt: null,
    });
    // The answer is still the empty placeholder: thinking is never streamed as reply text.
    expect(result.current.streaming).toBe("");
  });

  it("stops the thinking clock on the first answer token, once", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "hm" }));
    vi.setSystemTime(T0 + 12_000);
    act(() => h.onEvent!({ type: "token", text: "Taxes" }));
    expect(result.current.streamingThought?.answeredAt).toBe(T0 + 12_000);
    vi.setSystemTime(T0 + 20_000);
    act(() => h.onEvent!({ type: "token", text: " are due" }));
    expect(result.current.streamingThought?.answeredAt).toBe(T0 + 12_000);
    expect(result.current.streaming).toBe("Taxes are due");
  });

  it("commits the turn's thinking under its message id on done, timed to the answer", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "Let me check the invoice." }));
    vi.setSystemTime(T0 + 48_400);
    act(() => h.onEvent!({ type: "token", text: "Paid." }));
    vi.setSystemTime(T0 + 60_000); // the rest of the answer streaming adds nothing to the time
    act(() => h.onEvent!(answered(42)));

    expect(result.current.thoughts[42]).toEqual({
      text: "Let me check the invoice.",
      seconds: 48,
      skipped: false,
    });
  });

  it("drops the live thought on fell_back and commits none for it", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "local thinking" }));
    act(() => h.onEvent!({ type: "thinking_note", note: "fell_back" }));
    expect(result.current.streamingThought).toBeNull();

    // The cloud answers; its reply must not sit under the local model's thinking.
    act(() => h.onEvent!({ type: "token", text: "cloud answer" }));
    act(() =>
      h.onEvent!({
        type: "fallback",
        from_model: "gemma4",
        to_model: "gpt",
        reason: "hard_failure:timeout",
      }),
    );
    act(() => h.onEvent!(done({ message_id: 42, served_by: "cloud" })));
    expect(result.current.streamingThought).toBeNull();
    expect(result.current.thoughts[42]).toBeUndefined();
  });

  it("commits a no_room turn as skipped, with nothing timed", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking_note", note: "no_room" }));
    act(() => h.onEvent!({ type: "token", text: "A straight answer." }));
    act(() => h.onEvent!(answered(42)));
    expect(result.current.thoughts[42]).toEqual({ text: "", seconds: null, skipped: true });
  });

  it("puts no 'Answered without thinking' line over a reply the cloud gave", () => {
    // No room to think, so the local leg went with thinking off — and then failed before answering.
    // No thinking was shown, so no `fell_back` note comes; the cloud answers. That line's reason
    // (and its "longer context" fix) is the local model's, and the cloud never thinks here.
    const result = sending();
    act(() => h.onEvent!({ type: "thinking_note", note: "no_room" }));
    act(() => h.onEvent!({ type: "token", text: "cloud answer" }));
    act(() =>
      h.onEvent!({
        type: "fallback",
        from_model: "gemma4",
        to_model: "gpt",
        reason: "hard_failure:timeout",
      }),
    );
    act(() => h.onEvent!(done({ message_id: 42, served_by: "cloud" })));
    expect(result.current.thoughts[42]).toBeUndefined();
    expect(result.current.providers[42]).toBe("cloud");
  });

  it("commits nothing for a turn with no thinking at all", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "token", text: "Hi" }));
    act(() => h.onEvent!(answered(42)));
    expect(result.current.thoughts).toEqual({});
  });

  it("writes nothing for a conversation the user has left", () => {
    const { result } = renderHook(() => useChatStream(() => 1));
    act(() => {
      void result.current.send(2, "hi"); // conversation 1 is on screen
    });
    act(() => h.onEvent!({ type: "thinking", text: "elsewhere" }));
    act(() => h.onEvent!({ type: "token", text: "x" }));
    act(() => h.onEvent!(answered(9)));
    expect(result.current.streamingThought).toBeNull();
    expect(result.current.thoughts[9]).toBeUndefined();
  });

  it("clearTransient drops the live thought and keeps the committed ones", async () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "first" }));
    act(() => h.onEvent!({ type: "token", text: "a" }));
    act(() => h.onEvent!(answered(42)));
    await act(async () => {
      h.resolve!();
    });

    act(() => {
      void result.current.send(1, "again");
    });
    act(() => h.onEvent!({ type: "thinking", text: "second" }));
    expect(result.current.streamingThought?.text).toBe("second");

    act(() => result.current.clearTransient());
    expect(result.current.streamingThought).toBeNull();
    expect(result.current.thoughts[42]?.text).toBe("first");
  });

  /** A hook whose on-screen conversation the test moves, with one send to conversation 1. */
  function switchable() {
    const view = { current: 1 };
    const { result } = renderHook(() => useChatStream(() => view.current));
    act(() => {
      void result.current.send(1, "hi");
    });
    // Away to another chat and back, as App's selectConversation does: each switch clears the
    // transient view, and the reply keeps streaming underneath.
    const awayAndBack = () => {
      view.current = 2;
      act(() => result.current.clearTransient());
      view.current = 1;
      act(() => result.current.clearTransient());
    };
    return { result, awayAndBack };
  }

  it("brings the live thinking back to a view left and returned to mid-thought", () => {
    const { result, awayAndBack } = switchable();
    act(() => h.onEvent!({ type: "thinking", text: "a" }));
    awayAndBack();
    expect(result.current.streaming).toBeNull();

    act(() => h.onEvent!({ type: "thinking", text: "b" }));
    // ChatView draws the live block only inside the streaming column, so `streaming` has to come
    // back too — not wait for the first answer token, which can be minutes away.
    expect(result.current.streaming).toBe("");
    expect(result.current.streamingThought?.text).toBe("ab");
  });

  it("brings the fold back to a view left and returned to mid-answer", () => {
    const { result, awayAndBack } = switchable();
    act(() => h.onEvent!({ type: "thinking", text: "hm" }));
    vi.setSystemTime(T0 + 12_000);
    act(() => h.onEvent!({ type: "token", text: "Taxes" }));
    awayAndBack();
    expect(result.current.streamingThought).toBeNull();

    vi.setSystemTime(T0 + 20_000);
    act(() => h.onEvent!({ type: "token", text: " are due" }));
    expect(result.current.streaming).toBe("Taxes are due");
    // Still timed to the FIRST answer token, before the user left.
    expect(result.current.streamingThought).toEqual({
      text: "hm",
      startedAt: T0,
      answeredAt: T0 + 12_000,
    });
  });

  it("carries where the user left the live fold onto the settled turn", () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "Let me check." }));
    act(() => h.onEvent!({ type: "token", text: "Paid." }));
    // The answer started, so the fold closed by itself; the user opens it again to read.
    act(() => result.current.noteLiveFold({ open: true, scrollTop: 120 }));
    act(() => h.onEvent!({ type: "token", text: " In full." }));
    // A live fold mounted again (the user came back mid-reply) starts from it...
    expect(result.current.streamingThought?.fold).toEqual({ open: true, scrollTop: 120 });

    act(() => h.onEvent!(answered(42)));
    // ...and so does the settled fold, a separate mount once the stream clears.
    expect(result.current.thoughts[42]?.fold).toEqual({ open: true, scrollTop: 120 });
  });

  it("starts each send's fold afresh", async () => {
    const result = sending();
    act(() => h.onEvent!({ type: "thinking", text: "first" }));
    act(() => result.current.noteLiveFold({ open: true, scrollTop: null }));
    act(() => h.onEvent!({ type: "token", text: "a" }));
    act(() => h.onEvent!(answered(42)));
    await act(async () => {
      h.resolve!();
    });

    act(() => {
      void result.current.send(1, "again");
    });
    act(() => h.onEvent!({ type: "thinking", text: "second" }));
    expect(result.current.streamingThought?.fold).toBeUndefined();
    act(() => h.onEvent!({ type: "token", text: "b" }));
    act(() => h.onEvent!(answered(43)));
    expect(result.current.thoughts[43]?.fold).toBeUndefined();
    expect(result.current.thoughts[42]?.fold).toEqual({ open: true, scrollTop: null });
  });

  it("sends the toggle as it stands at each send", async () => {
    // Read at send time, not captured when `send` was created: `send` is stable for the hook's
    // life, so a captured value would freeze the toggle at whatever it was on mount.
    const { result } = renderHook(() => useChatStream(() => 1));
    localStorage.setItem("pm.chat.showThinking", "1");
    act(() => {
      void result.current.send(1, "one");
    });
    await act(async () => {
      h.resolve!();
    });
    localStorage.removeItem("pm.chat.showThinking");
    act(() => {
      void result.current.send(1, "two");
    });

    const calls = vi.mocked(sendMessage).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][4]).toBe(true);
    expect(calls[1][4]).toBe(false);
  });
});
