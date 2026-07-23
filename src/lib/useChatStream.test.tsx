// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The provider-honesty state added in PR6: `fallback` (transient, current-conversation only) and the
// `providers` map (committed on `done`). Drives the streamed events through a captured `sendMessage`
// callback — no Tauri — and pins the clear semantics (dismiss/next-send clear `fallback` but keep
// `providers`) and the leave-the-conversation guard.

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

import { useChatStream } from "./useChatStream";

const done = (over: Partial<Extract<ChatEvent, { type: "done" }>> = {}): ChatEvent => ({
  type: "done",
  message_id: 42,
  content: "yo",
  citations: [],
  served_by: "cloud",
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
});
