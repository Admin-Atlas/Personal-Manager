// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Repeat row's two reads (#884, C11 §5.2): only the newest answer is ever shown (an older one
// arriving late describes a start or rule the editor no longer holds), no repeat asks nothing, and a
// read that fails says so in words rather than crashing or looking fine.

import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RepeatPreset, RepeatRule, RepeatStart, RepeatView } from "../../../lib/types";
import type { RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";

const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();
const describeRecurrence = vi.fn<(rule: RepeatRule, start: RepeatStart) => Promise<RepeatView>>();

vi.mock("../../../lib/ipc", () => ({
  recurrencePresets: (day: string) => recurrencePresets(day),
  describeRecurrence: (rule: RepeatRule, start: RepeatStart) => describeRecurrence(rule, start),
}));

import { REPEAT_UNCHECKED, usePresets, useRepeatView } from "./useRepeat";

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

/** A promise the test answers when it chooses. */
function later<T>() {
  let answer: (v: T) => void = () => {};
  let fail: (e: Error) => void = () => {};
  const promise = new Promise<T>((resolve, reject) => {
    answer = resolve;
    fail = reject;
  });
  return { promise, answer, fail };
}

const weekly = (day: "mon" | "tue" | "wed"): RepeatRule => ({
  pattern: { kind: "weekly", days: [day] },
  interval: 1,
  ends: { kind: "never" },
});
const preset = (label: string, rule: RepeatRule): RepeatPreset => ({ label, rule });
const at = (date: string): RepeatStart => ({
  kind: "timed",
  date,
  time: "09:00",
  zone: "Europe/London",
});
const ruleOf = (rule: RepeatRule): RepeatValue => ({ kind: "rule", rule });

describe("usePresets", () => {
  it("a stale presets answer never replaces a later one", async () => {
    const tuesday = later<RepeatPreset[]>();
    const wednesday = later<RepeatPreset[]>();
    recurrencePresets.mockImplementation((day) =>
      day === "2026-10-13" ? tuesday.promise : wednesday.promise,
    );
    const { result, rerender } = renderHook(({ day }) => usePresets(day), {
      initialProps: { day: "2026-10-13" as string | null },
    });
    expect(result.current).toBeNull();
    rerender({ day: "2026-10-14" });
    await act(async () => wednesday.answer([preset("Weekly on Wednesday", weekly("wed"))]));
    expect(result.current?.map((p) => p.label)).toEqual(["Weekly on Wednesday"]);
    // Tuesday's answer, arriving last, is for a day the event no longer starts on.
    await act(async () => tuesday.answer([preset("Weekly on Tuesday", weekly("tue"))]));
    expect(result.current?.map((p) => p.label)).toEqual(["Weekly on Wednesday"]);
    expect(recurrencePresets.mock.calls).toEqual([["2026-10-13"], ["2026-10-14"]]);
  });

  it("another day's presets are loading until its own come, and no day asks nothing", async () => {
    recurrencePresets.mockResolvedValueOnce([preset("Daily", weekly("tue"))]);
    recurrencePresets.mockReturnValueOnce(later<RepeatPreset[]>().promise);
    const { result, rerender } = renderHook(({ day }) => usePresets(day), {
      initialProps: { day: "2026-10-13" as string | null },
    });
    await act(async () => {});
    expect(result.current).toHaveLength(1);
    rerender({ day: "2026-10-14" });
    expect(result.current).toBeNull();
    rerender({ day: null });
    expect(result.current).toBeNull();
    expect(recurrencePresets).toHaveBeenCalledTimes(2);
  });
});

describe("useRepeatView", () => {
  it("a stale view answer never replaces a later one", async () => {
    const first = later<RepeatView>();
    const second = later<RepeatView>();
    describeRecurrence.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { result, rerender } = renderHook(
      ({ value, start }) => useRepeatView(value, start).state,
      { initialProps: { value: ruleOf(weekly("mon")), start: at("2026-10-13") } },
    );
    expect(result.current).toEqual({ state: "loading" });
    rerender({ value: ruleOf(weekly("wed")), start: at("2026-10-13") });
    await act(async () =>
      second.answer({
        outcome: "ready",
        summary: "Weekly on Wednesday",
        starts: "2026-10-14",
        moved: true,
      }),
    );
    expect(result.current).toEqual({
      state: "ready",
      summary: "Weekly on Wednesday",
      starts: "2026-10-14",
      moved: true,
    });
    await act(async () =>
      first.answer({
        outcome: "ready",
        summary: "Weekly on Monday",
        starts: "2026-10-19",
        moved: true,
      }),
    );
    expect(result.current).toMatchObject({ summary: "Weekly on Wednesday" });
    expect(describeRecurrence.mock.calls).toEqual([
      [weekly("mon"), at("2026-10-13")],
      [weekly("wed"), at("2026-10-13")],
    ]);
  });

  it("once and kept ask nothing", async () => {
    const { result, rerender } = renderHook(
      ({ value }) => useRepeatView(value, at("2026-10-13")).state,
      { initialProps: { value: { kind: "once" } as RepeatValue } },
    );
    expect(result.current).toEqual({ state: "none" });
    rerender({ value: { kind: "kept", words: "Repeats" } });
    expect(result.current).toEqual({ state: "none" });
    await act(async () => {});
    expect(describeRecurrence).not.toHaveBeenCalled();
  });

  it("a thrown read is refused in words", async () => {
    describeRecurrence.mockRejectedValueOnce(new Error("channel closed"));
    const { result, rerender } = renderHook(
      ({ value }) => useRepeatView(value, at("2026-10-13")).state,
      { initialProps: { value: ruleOf(weekly("tue")) } },
    );
    await act(async () => {});
    expect(result.current).toEqual({ state: "refused", message: REPEAT_UNCHECKED });
    expect(REPEAT_UNCHECKED).toBe("PM couldn't check this repeat. Try again.");
    // A throw before the read even returns a promise reads the same way.
    describeRecurrence.mockImplementationOnce(() => {
      throw new Error("no backend");
    });
    rerender({ value: ruleOf(weekly("wed")) });
    await act(async () => {});
    expect(result.current).toEqual({ state: "refused", message: REPEAT_UNCHECKED });
  });

  it("says the backend's refusal as it is", async () => {
    describeRecurrence.mockResolvedValueOnce({
      outcome: "refused",
      message: "Google allows at most 730 occurrences of a repeating event.",
    });
    const { result } = renderHook(
      () => useRepeatView(ruleOf(weekly("tue")), at("2026-10-13")).state,
    );
    await act(async () => {});
    expect(result.current).toEqual({
      state: "refused",
      message: "Google allows at most 730 occurrences of a repeating event.",
    });
  });

  // A save reads its draft as it is then: a start the blur before it just committed hasn't been asked
  // about, so the last answer, for the start before, says nothing of it.
  it("a save may go only on a ready answer for exactly its rule and start", async () => {
    const pending = later<RepeatView>();
    describeRecurrence
      .mockResolvedValueOnce({
        outcome: "ready",
        summary: "Weekly on Tuesday",
        starts: "2026-10-13",
        moved: false,
      })
      .mockReturnValueOnce(pending.promise);
    const { result, rerender } = renderHook(
      ({ start }) => useRepeatView(ruleOf(weekly("tue")), start),
      {
        initialProps: { start: at("2026-10-13") },
      },
    );
    const { saveable } = result.current;
    expect(saveable(ruleOf(weekly("tue")), at("2026-10-13"))).toBe(false);
    await act(async () => {});
    expect(saveable(ruleOf(weekly("tue")), at("2026-10-13"))).toBe(true);
    // Another start, or another rule: not asked yet.
    expect(saveable(ruleOf(weekly("tue")), at("2026-10-20"))).toBe(false);
    expect(saveable(ruleOf(weekly("wed")), at("2026-10-13"))).toBe(false);
    // No repeat needs nothing.
    expect(saveable({ kind: "once" }, at("2026-10-20"))).toBe(true);
    rerender({ start: at("2026-10-20") });
    expect(result.current.saveable(ruleOf(weekly("tue")), at("2026-10-20"))).toBe(false);
    await act(async () =>
      pending.answer({
        outcome: "refused",
        message: "The end date is before the first occurrence.",
      }),
    );
    expect(result.current.saveable(ruleOf(weekly("tue")), at("2026-10-20"))).toBe(false);
    expect(result.current.saveable).toBe(saveable);
  });
});
