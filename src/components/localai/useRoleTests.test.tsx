// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The role tests used to live in the roles section, which the tab remounted (a `key`) whenever the
// endpoint moved — and the remount was what threw away a pass proved against the old server. Now
// they live in a hook the tab holds, so the reset has to be done on purpose. This pins that it is,
// and that the backend's job is re-read afterwards, as the remount used to.

import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LocalTestResult } from "../../lib/types";

const testLocalLlm = vi.fn();
const activeLocalTest = vi.fn();

vi.mock("../../lib/ipc", () => ({
  testLocalLlm: (...a: unknown[]) => testLocalLlm(...a),
  activeLocalTest: () => activeLocalTest(),
}));

import { useRoleTests } from "./useRoleTests";

const pass = (): LocalTestResult => ({
  model: "tiny-chat:1b",
  ok: true,
  reply: "ready",
  elapsed_ms: 1200,
  loaded_for_test: false,
  was_holding: [],
  message: null,
});

beforeEach(() => {
  vi.clearAllMocks();
  activeLocalTest.mockResolvedValue(null);
  testLocalLlm.mockResolvedValue(pass());
});
afterEach(cleanup);

describe("useRoleTests", () => {
  it("drops every result when the endpoint moves, then re-reads the backend's job", async () => {
    const { result, rerender } = renderHook(({ epoch }) => useRoleTests(epoch), {
      initialProps: { epoch: 0 },
    });
    await act(() => result.current.runTest("chat"));
    expect(result.current.tests.chat?.result?.ok).toBe(true);
    const readsBefore = activeLocalTest.mock.calls.length;

    // A pass against the old server says nothing about the new one.
    rerender({ epoch: 1 });
    expect(result.current.tests).toEqual({});
    expect(result.current.testing).toBeNull();
    await waitFor(() => expect(activeLocalTest.mock.calls.length).toBeGreaterThan(readsBefore));
  });

  it("keeps results while the endpoint stays put, and clears only the row asked", async () => {
    const { result, rerender } = renderHook(({ epoch }) => useRoleTests(epoch), {
      initialProps: { epoch: 0 },
    });
    await act(() => result.current.runTest("chat"));
    await act(() => result.current.runTest("background"));

    rerender({ epoch: 0 });
    expect(result.current.tests.chat?.result?.ok).toBe(true);

    act(() => result.current.clearTest("chat"));
    expect(result.current.tests.chat).toEqual({ result: null, error: null });
    expect(result.current.tests.background?.result?.ok).toBe(true);
  });
});
