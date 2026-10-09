// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The held delete as the webview sees it (#884): the row hides at once, Undo is offered for the
// window, and the backend's `calendar://delete-settled` says how it ended. The store is module-level
// (it outlives the Calendar tab unmounting), so each test loads a fresh copy.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent, DeleteSettled, DeleteStart, HeldDeleteInfo } from "../../../lib/types";

const deleteCalendarEvent = vi.fn<(id: string, seen: unknown) => Promise<DeleteStart>>();
const cancelCalendarDelete = vi.fn<(token: string) => Promise<boolean>>();
const listHeldDeletes = vi.fn<() => Promise<HeldDeleteInfo[]>>();
let settledHandler: ((s: DeleteSettled) => void) | null = null;

vi.mock("../../../lib/ipc", () => ({
  deleteCalendarEvent: (id: string, seen: unknown) => deleteCalendarEvent(id, seen),
  cancelCalendarDelete: (token: string) => cancelCalendarDelete(token),
  listHeldDeletes: () => listHeldDeletes(),
  onCalendarDeleteSettled: (h: (s: DeleteSettled) => void) => {
    settledHandler = h;
    return Promise.resolve(() => {});
  },
}));

const event: CalendarEvent = {
  id: "gcal:me@x.com:me@x.com:abc",
  calendar_id: "gcal:me@x.com:me@x.com",
  summary: "Dentist",
  description: null,
  location: "High St",
  start: "2026-10-12T09:00:00Z",
  end: "2026-10-12T10:00:00Z",
  all_day: false,
  html_link: null,
  uid: "abc@google.com",
};

const held: DeleteStart = { outcome: "held", undo_token: "t1", undo_seconds: 8 };

/** A fresh store, read through the hook the calendar uses. */
async function load() {
  vi.resetModules();
  const mod = await import("./useEventWrites");
  const { renderHook, act } = await import("@testing-library/react");
  const hook = renderHook(() => mod.useEventWrites());
  // Let the mount's listener and held-delete read resolve.
  await act(async () => {});
  return { mod, hook, act };
}

beforeEach(() => {
  vi.useFakeTimers();
  settledHandler = null;
  listHeldDeletes.mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("a held delete", () => {
  it("hides the row and offers Undo, then keeps it hidden until the mirror catches up", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    // What the user saw is what decides the delete later.
    expect(deleteCalendarEvent).toHaveBeenCalledWith(event.id, {
      summary: "Dentist",
      start: event.start,
      end: event.end,
      all_day: false,
      location: "High St",
    });
    expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    expect(hook.result.current.notices).toEqual([
      {
        id: 1,
        tone: "ok",
        text: "Deleting “Dentist”.",
        undoToken: "t1",
        undoLabel: "Undo deleting “Dentist”",
      },
    ]);
    // The window closes: no more Undo.
    act(() => {
      vi.advanceTimersByTime(8000);
    });
    expect(hook.result.current.notices[0].undoToken).toBeUndefined();
    // Google confirms. The read the backend's write-outcome starts may not have landed yet, so the
    // row stays hidden until a read without it comes back.
    act(() =>
      settledHandler!({
        undo_token: "t1",
        event_id: event.id,
        result: { outcome: "saved", warnings: [] },
      }),
    );
    expect(hook.result.current.notices).toEqual([
      { id: 1, tone: "ok", text: "Deleted “Dentist”." },
    ]);
    expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    act(() => mod.pruneRemoved([event])); // a read from before the delete landed
    expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    act(() => mod.pruneRemoved([])); // the read after it
    expect(hook.result.current.hiddenIds.size).toBe(0);
  });

  it("keeps the event when undone in time, and a second click sends nothing", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    cancelCalendarDelete.mockResolvedValue(true);
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    await act(async () => {
      const first = mod.undoDelete("t1");
      const second = mod.undoDelete("t1"); // the button is gone, but a fast second click
      await Promise.all([first, second]);
    });
    expect(cancelCalendarDelete).toHaveBeenCalledTimes(1);
    expect(hook.result.current.hiddenIds.size).toBe(0);
    expect(hook.result.current.notices).toEqual([{ id: 1, tone: "ok", text: "Kept “Dentist”." }]);
  });

  it("says so when the Undo came too late, then reports how it ended and shows the row again", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    cancelCalendarDelete.mockResolvedValue(false);
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    await act(() => mod.undoDelete("t1"));
    expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    expect(hook.result.current.notices[0]).toMatchObject({ tone: "warn" });
    act(() =>
      settledHandler!({
        undo_token: "t1",
        event_id: event.id,
        result: { outcome: "conflict", fields: ["start"] },
      }),
    );
    expect(hook.result.current.hiddenIds.size).toBe(0);
    expect(hook.result.current.notices[0]).toEqual({
      id: 1,
      tone: "warn",
      text: "“Dentist” wasn't deleted. It changed in Google since you opened it (the time).",
    });
  });

  it("leaves a settled outcome alone when the Undo's answer comes back after it", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    let answer: (v: boolean) => void = () => {};
    cancelCalendarDelete.mockReturnValue(new Promise((r) => (answer = r)));
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    await act(async () => {
      const undo = mod.undoDelete("t1");
      settledHandler!({
        undo_token: "t1",
        event_id: event.id,
        result: { outcome: "saved", warnings: [] },
      });
      answer(false);
      await undo;
    });
    expect(hook.result.current.notices).toEqual([
      { id: 1, tone: "ok", text: "Deleted “Dentist”." },
    ]);
  });

  it("words a refusal at the start: PM's own pending write, or the reason", async () => {
    deleteCalendarEvent.mockResolvedValueOnce({ outcome: "refused", result: { outcome: "busy" } });
    deleteCalendarEvent.mockResolvedValueOnce({
      outcome: "refused",
      result: { outcome: "read_only", reason: "editing_off" },
    });
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    await act(() => mod.startDelete(event));
    expect(hook.result.current.hiddenIds.size).toBe(0);
    const [busy, off] = hook.result.current.notices;
    expect(busy.text).toBe("PM is already changing “Dentist”. Wait a moment, then try again.");
    expect(off.tone).toBe("error");
    expect(off.text).toMatch(/^“Dentist” wasn't deleted\. Editing isn't on/);
  });

  it("picks up deletes still waiting after the webview reloaded", async () => {
    listHeldDeletes.mockResolvedValue([
      { undo_token: "t9", event_id: event.id, summary: "Dentist", seconds_left: 5 },
    ]);
    const { hook } = await load();
    expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    expect(hook.result.current.notices[0]).toMatchObject({
      text: "Deleting “Dentist”.",
      undoToken: "t9",
    });
  });

  it("lets a finished notice go by itself, but keeps a failure up", async () => {
    deleteCalendarEvent.mockRejectedValue(
      new Error("Calendar changes can only be made from PM's main window."),
    );
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(hook.result.current.notices).toHaveLength(1);
    act(() => mod.dismissNotice(hook.result.current.notices[0].id));
    expect(hook.result.current.notices).toEqual([]);
  });
});
