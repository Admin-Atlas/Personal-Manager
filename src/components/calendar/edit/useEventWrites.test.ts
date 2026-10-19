// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The held delete as the webview sees it (#884): the row hides at once, Undo is offered for the
// window, and the backend's `calendar://delete-settled` says how it ended. The store is module-level
// (it outlives the Calendar tab unmounting), so each test loads a fresh copy.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  ChoiceRequest,
  DeleteSettled,
  DeleteStart,
  EventPatchDraft,
  HeldDeleteInfo,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";

type Scope = string | null;
const deleteCalendarEvent =
  vi.fn<(id: string, seen: unknown, scope: Scope, choice: Scope) => Promise<DeleteStart>>();
const cancelCalendarDelete = vi.fn<(token: string) => Promise<boolean>>();
const listHeldDeletes = vi.fn<() => Promise<HeldDeleteInfo[]>>();
const updateCalendarEvent =
  vi.fn<
    (session: string, draft: EventPatchDraft, scope: Scope, choice: Scope) => Promise<WriteOutcome>
  >();
const seriesChoices = vi.fn<(request: ChoiceRequest) => Promise<SeriesChoices>>();
const settleSeriesSplit = vi.fn<(token: string, action: SplitAction) => Promise<WriteOutcome>>();
let settledHandler: ((s: DeleteSettled) => void) | null = null;

vi.mock("../../../lib/ipc", () => ({
  deleteCalendarEvent: (id: string, seen: unknown, scope: Scope, choice: Scope) =>
    deleteCalendarEvent(id, seen, scope, choice),
  cancelCalendarDelete: (token: string) => cancelCalendarDelete(token),
  listHeldDeletes: () => listHeldDeletes(),
  updateCalendarEvent: (session: string, draft: EventPatchDraft, scope: Scope, choice: Scope) =>
    updateCalendarEvent(session, draft, scope, choice),
  seriesChoices: (request: ChoiceRequest) => seriesChoices(request),
  settleSeriesSplit: (token: string, action: SplitAction) => settleSeriesSplit(token, action),
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
    // What the user saw is what decides the delete later; a one-off names no occurrences.
    expect(deleteCalendarEvent).toHaveBeenCalledWith(
      event.id,
      {
        summary: "Dentist",
        start: event.start,
        end: event.end,
        all_day: false,
        location: "High St",
      },
      null,
      null,
    );
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
    // Done and fine, on screen: it goes by itself after 5 s.
    act(() => {
      vi.advanceTimersByTime(4_999);
    });
    expect(hook.result.current.notices).toHaveLength(1);
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(hook.result.current.notices).toEqual([]);
  });

  // Past its Undo window a delete is still being sent: its notice stays, however long Google takes.
  it("keeps a delete's notice up until it settles", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    act(() => {
      vi.advanceTimersByTime(8_000 + 60_000);
    });
    expect(hook.result.current.notices).toEqual([
      { id: 1, tone: "ok", text: "Deleting “Dentist”." },
    ]);
    act(() =>
      settledHandler!({
        undo_token: "t1",
        event_id: event.id,
        result: { outcome: "saved", warnings: [] },
      }),
    );
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(hook.result.current.notices).toEqual([]);
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

  // PM reopens on Focus after a reload: a delete that ends before the Calendar tab is opened still
  // says how it ended there, and only then starts going by itself.
  it("keeps how a delete ended until the Calendar tab shows it", async () => {
    vi.resetModules();
    const mod = await import("./useEventWrites");
    const { renderHook, act } = await import("@testing-library/react");
    listHeldDeletes.mockResolvedValue([
      { undo_token: "t9", event_id: event.id, summary: "Dentist", seconds_left: 5 },
    ]);
    await act(async () => mod.watchHeldDeletes());
    act(() =>
      settledHandler!({
        undo_token: "t9",
        event_id: event.id,
        result: { outcome: "saved", warnings: [] },
      }),
    );
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    const hook = renderHook(() => mod.useEventWrites());
    expect(hook.result.current.notices.map((n) => n.text)).toEqual(["Deleted “Dentist”."]);
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(hook.result.current.notices).toEqual([]);
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

// Ctrl+Z on the Calendar tab presses the newest Undo on screen (#884, F5).
describe("undoing the newest delete", () => {
  const second: CalendarEvent = { ...event, id: `${event.id}2`, summary: "Gym" };

  it("undoes the newest first, then the one before, then nothing", async () => {
    deleteCalendarEvent
      .mockResolvedValueOnce(held)
      .mockResolvedValueOnce({ outcome: "held", undo_token: "t2", undo_seconds: 8 });
    cancelCalendarDelete.mockResolvedValue(true);
    const { mod, act } = await load();
    await act(() => mod.startDelete(event));
    await act(() => mod.startDelete(second));
    await act(async () => {
      expect(mod.undoLatest()).toBe(true);
    });
    expect(cancelCalendarDelete).toHaveBeenLastCalledWith("t2");
    await act(async () => {
      expect(mod.undoLatest()).toBe(true);
    });
    expect(cancelCalendarDelete).toHaveBeenLastCalledWith("t1");
    await act(async () => {
      expect(mod.undoLatest()).toBe(false);
    });
    expect(cancelCalendarDelete).toHaveBeenCalledTimes(2);
  });

  it("never undoes what isn't on screen: a window that closed, or a notice dismissed", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(event));
    act(() => mod.dismissNotice(hook.result.current.notices[0].id));
    expect(mod.undoLatest()).toBe(false);
    await act(() => mod.startDelete(second));
    act(() => {
      vi.advanceTimersByTime(8000);
    });
    expect(mod.undoLatest()).toBe(false);
    expect(cancelCalendarDelete).not.toHaveBeenCalled();
  });

  it("never sends one undo twice, however fast the key is pressed", async () => {
    deleteCalendarEvent.mockResolvedValue(held);
    let answer: (v: boolean) => void = () => {};
    cancelCalendarDelete.mockReturnValue(new Promise((r) => (answer = r)));
    const { mod, act } = await load();
    await act(() => mod.startDelete(event));
    expect(mod.undoLatest()).toBe(true);
    // Its button is already gone, so a second press finds nothing newer to undo.
    expect(mod.undoLatest()).toBe(false);
    await act(async () => answer(true));
    expect(cancelCalendarDelete).toHaveBeenCalledTimes(1);
  });

  it("reads the newest from the notices", async () => {
    const { mod } = await load();
    expect(mod.latestUndo([])).toBeNull();
    expect(
      mod.latestUndo([
        { id: 1, tone: "ok", text: "a", undoToken: "old" },
        { id: 2, tone: "ok", text: "b", undoToken: "new" },
        { id: 3, tone: "ok", text: "Saved." },
      ]),
    ).toBe("new");
  });
});

// A repeating event's writes (#884, C9b-1): the choices, a scope always with its token, every row a
// delete of several occurrences reaches, and a change PM couldn't finish kept with its buttons.
describe("a repeating event", () => {
  const occurrence: CalendarEvent = { ...event, summary: "Gym", series_id: "abc", recurring: true };
  const later = [`${event.id}_2`, `${event.id}_3`];
  const partial = (
    offers: SplitAction[],
    token = "s1",
    note?: string,
  ): Extract<WriteOutcome, { outcome: "partial_split" }> => ({
    outcome: "partial_split",
    token,
    offers,
    from: "2026-11-02",
    ...(note ? { note } : {}),
  });

  beforeEach(() => {
    vi.setSystemTime(new Date("2026-10-18T12:00:00"));
  });

  it("hides every row a delete of several occurrences reaches, and Undo shows them all again", async () => {
    deleteCalendarEvent.mockResolvedValue({
      outcome: "held",
      undo_token: "t1",
      undo_seconds: 8,
      hidden: [occurrence.id, ...later],
    });
    cancelCalendarDelete.mockResolvedValue(true);
    const { mod, hook, act } = await load();
    await act(async () => {
      expect(await mod.startDelete(occurrence, { scope: "following", token: "f" })).toBe("held");
    });
    expect([...hook.result.current.hiddenIds]).toEqual([occurrence.id, ...later]);
    await act(() => mod.undoDelete("t1"));
    expect(hook.result.current.hiddenIds.size).toBe(0);
  });

  it("keeps every row it removed hidden until a read no longer has them", async () => {
    deleteCalendarEvent.mockResolvedValue({
      outcome: "held",
      undo_token: "t1",
      undo_seconds: 8,
      hidden: later,
    });
    const { mod, hook, act } = await load();
    await act(() => mod.startDelete(occurrence, { scope: "following", token: "f" }));
    act(() =>
      settledHandler!({
        undo_token: "t1",
        event_id: occurrence.id,
        result: { outcome: "saved", warnings: [] },
      }),
    );
    expect([...hook.result.current.hiddenIds].sort()).toEqual([occurrence.id, ...later].sort());
    act(() => mod.pruneRemoved([{ ...event, id: later[0] }]));
    expect([...hook.result.current.hiddenIds]).toEqual([later[0]]);
    act(() => mod.pruneRemoved([]));
    expect(hook.result.current.hiddenIds.size).toBe(0);
  });

  // C9b-1 critique 1 H3 / 2 M6: the calendar as it reads the mirror, with no hand-run prune. A
  // settle starts a read of its own; only a read that started after the settle can show again a
  // row the delete left in Google, and only when the delete said it left some.
  describe("the read a settle starts", () => {
    const rows = (ids: string[]) => ids.map((id) => ({ ...event, id }));
    let reads: ((events: CalendarEvent[]) => void)[] = [];

    /** A fresh store, read through a hook that reads the mirror as the calendar does. */
    async function loadCalendar() {
      vi.resetModules();
      const mod = await import("./useEventWrites");
      const { renderHook, act } = await import("@testing-library/react");
      const { useEffect, useRef } = await import("react");
      const read = () => new Promise<CalendarEvent[]>((r) => reads.push(r));
      const hook = renderHook(() => {
        const writes = mod.useEventWrites();
        const seen = useRef(writes.settles);
        useEffect(() => {
          if (writes.settles === seen.current) return;
          seen.current = writes.settles;
          void mod.readMirror(read);
        }, [writes.settles]);
        return writes;
      });
      await act(async () => {});
      return { mod, hook, act, read };
    }

    beforeEach(() => {
      reads = [];
      deleteCalendarEvent.mockResolvedValue({
        outcome: "held",
        undo_token: "t1",
        undo_seconds: 8,
        hidden: later,
      });
    });

    it("shows again a row the delete left in Google, once a read after it lands", async () => {
      const { mod, hook, act, read } = await loadCalendar();
      await act(() => mod.startDelete(occurrence, { scope: "following", token: "f" }));
      // The read the backend's landing starts, begun before the webview hears the settle.
      const early = mod.readMirror(read);
      act(() =>
        settledHandler!({
          undo_token: "t1",
          event_id: occurrence.id,
          result: {
            outcome: "saved",
            warnings: ["left_on_old_series"],
            left_behind: ["2026-11-02"],
          },
        }),
      );
      // The settle started a read of its own.
      expect(reads).toHaveLength(2);
      const all = [occurrence.id, ...later];
      await act(async () => {
        reads[0](rows(all));
        await early;
      });
      expect([...hook.result.current.hiddenIds].sort()).toEqual([...all].sort());
      // Google kept the occurrence changed on its own; the rest went.
      await act(async () => reads[1](rows([later[0]])));
      expect(hook.result.current.hiddenIds.size).toBe(0);
    });

    it("keeps a deleted row hidden while the mirror lags behind Google", async () => {
      const { mod, hook, act } = await loadCalendar();
      await act(() => mod.startDelete(occurrence, { scope: "following", token: "f" }));
      act(() =>
        settledHandler!({
          undo_token: "t1",
          event_id: occurrence.id,
          result: {
            outcome: "saved",
            warnings: ["mirror_refresh_pending", "left_on_old_series"],
            left_behind: ["2026-11-02"],
          },
        }),
      );
      const all = [occurrence.id, ...later];
      await act(async () => reads[0](rows(all)));
      expect([...hook.result.current.hiddenIds].sort()).toEqual([...all].sort());
      // A plain delete that landed: hidden while the mirror has it, whenever the read began.
      deleteCalendarEvent.mockResolvedValue({ outcome: "held", undo_token: "t2", undo_seconds: 8 });
      await act(() => mod.startDelete(event));
      act(() =>
        settledHandler!({
          undo_token: "t2",
          event_id: event.id,
          result: { outcome: "saved", warnings: [] },
        }),
      );
      await act(async () => reads[1](rows([...all, event.id])));
      expect(hook.result.current.hiddenIds.has(event.id)).toBe(true);
    });
  });

  it("picks up a held delete's rows after a reload", async () => {
    listHeldDeletes.mockResolvedValue([
      { undo_token: "t9", event_id: occurrence.id, summary: "Gym", seconds_left: 5, hidden: later },
    ]);
    const { hook } = await load();
    expect([...hook.result.current.hiddenIds]).toEqual([occurrence.id, ...later]);
  });

  it("answers ask, saying nothing, when the backend wants the occurrences chosen", async () => {
    deleteCalendarEvent.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "needs_choice" },
    });
    const { mod, hook, act } = await load();
    await act(async () => {
      expect(await mod.startDelete(occurrence)).toBe("ask");
      // Chosen, but the plan changed since: ask again, too.
      expect(await mod.startDelete(occurrence, { scope: "all", token: "a" })).toBe("ask");
    });
    expect(hook.result.current.notices).toEqual([]);
    expect(hook.result.current.hiddenIds.size).toBe(0);
    deleteCalendarEvent.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "read_only", reason: "editing_off" },
    });
    await act(async () => {
      expect(await mod.startDelete(occurrence)).toBe("refused");
    });
    expect(hook.result.current.notices[0].text).toMatch(/^“Gym” wasn't deleted\. Editing isn't on/);
  });

  it("never sends a scope without its token", async () => {
    deleteCalendarEvent.mockResolvedValue({ outcome: "held", undo_token: "t1", undo_seconds: 8 });
    updateCalendarEvent.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { mod, act } = await load();
    const changes: EventPatchDraft = { summary: "Retro" };
    await act(() => mod.startDelete(occurrence, { scope: "following", token: "f" }));
    expect(deleteCalendarEvent).toHaveBeenLastCalledWith(
      occurrence.id,
      expect.anything(),
      "following",
      "f",
    );
    await act(async () => {
      await mod.saveEdit("sess", changes, { scope: "all", token: "a" }, "Gym");
      await mod.saveEdit("sess", changes, null, "Gym");
    });
    expect(updateCalendarEvent.mock.calls).toEqual([
      ["sess", changes, "all", "a"],
      ["sess", changes, null, null],
    ]);
    // The very draft the choices were asked for goes to the save (the token binds it).
    expect(updateCalendarEvent.mock.calls[0][1]).toBe(changes);
  });

  it("asks for the choices as the token binds them, once per row while one is on its way", async () => {
    let answer: (c: SeriesChoices) => void = () => {};
    seriesChoices.mockReturnValue(new Promise((r) => (answer = r)));
    const { mod, act } = await load();
    const draft: EventPatchDraft = { summary: "Retro" };
    void mod.choicesForEdit("sess", draft);
    expect(seriesChoices).toHaveBeenLastCalledWith({ kind: "edit", session: "sess", draft });
    // StrictMode mounts the delete dialog's effect twice: one ask.
    const first = mod.choicesForDelete(occurrence);
    const second = mod.choicesForDelete(occurrence);
    expect(second).toBe(first);
    expect(seriesChoices).toHaveBeenCalledTimes(2);
    expect(seriesChoices).toHaveBeenLastCalledWith({
      kind: "delete",
      event_id: occurrence.id,
      seen: {
        summary: "Gym",
        start: event.start,
        end: event.end,
        all_day: false,
        location: "High St",
      },
    });
    await act(async () => {
      answer({ outcome: "not_repeating" });
      await first;
    });
    // Answered: the next open asks again.
    void mod.choicesForDelete(occurrence);
    expect(seriesChoices).toHaveBeenCalledTimes(3);
  });

  it("keeps a save that went partial with its buttons, whatever happens to the editor", async () => {
    updateCalendarEvent.mockResolvedValue(partial(["finish", "remove"]));
    const { mod, hook, act } = await load();
    let answer: Awaited<ReturnType<typeof mod.saveEdit>> | null = null;
    await act(async () => {
      answer = await mod.saveEdit(
        "sess",
        { summary: "Retro" },
        { scope: "following", token: "f" },
        "Gym",
      );
    });
    expect(answer).toMatchObject({ outcome: "partial_split", newPartial: true });
    const [notice] = hook.result.current.notices;
    expect(notice).toMatchObject({
      tone: "warn",
      split: { token: "s1", offers: ["finish", "remove"], title: "Gym" },
    });
    expect(notice.text).toMatch(
      /^“Gym”: PM couldn't finish this change: it may show twice from 02-11/,
    );
    // An earlier change blocking the next save: the same notice, not new.
    await act(async () => {
      answer = await mod.saveEdit(
        "sess",
        { summary: "Again" },
        { scope: "all", token: "a" },
        "Gym",
      );
    });
    expect(answer).toMatchObject({ newPartial: false });
    expect(hook.result.current.notices).toHaveLength(1);
  });

  it("notes a partial once per change, and it never goes by itself", async () => {
    const { mod, hook, act } = await load();
    act(() => {
      expect(mod.notePartial("Gym", partial(["check"]))).toBe(true);
    });
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(hook.result.current.notices).toHaveLength(1);
    act(() => {
      expect(mod.notePartial("Gym", partial(["finish"]))).toBe(false);
    });
    expect(hook.result.current.notices).toHaveLength(1);
    expect(hook.result.current.notices[0].split?.offers).toEqual(["finish"]);
  });

  it("settles one at a time, its buttons off meanwhile, kept while the change is still held", async () => {
    let answer: (o: WriteOutcome) => void = () => {};
    settleSeriesSplit.mockReturnValue(new Promise((r) => (answer = r)));
    const { mod, hook, act } = await load();
    act(() => {
      mod.notePartial("Gym", partial(["finish", "remove"]));
    });
    let first: Promise<WriteOutcome | null> = Promise.resolve(null);
    await act(async () => {
      first = mod.settleSplit("s1", "finish", "Gym");
      expect(await mod.settleSplit("s1", "finish", "Gym")).toBeNull();
    });
    expect(settleSeriesSplit).toHaveBeenCalledTimes(1);
    expect(hook.result.current.notices[0].settling).toBe(true);
    // Still held (Finish couldn't read everything): the buttons stay, with the offers now and why.
    const note =
      "PM couldn't read every occurrence it has to check, so it didn't finish. Try again in a moment.";
    await act(async () => {
      answer(partial(["remove"], "s1", note));
      await first;
    });
    const [notice] = hook.result.current.notices;
    expect(notice).toMatchObject({
      tone: "warn",
      text: `“Gym”: ${note}`,
      split: { token: "s1", offers: ["remove"], title: "Gym" },
    });
    expect(notice.settling).toBeFalsy();
  });

  it("takes the buttons away once the change ends, and that notice goes by itself", async () => {
    settleSeriesSplit.mockResolvedValue({ outcome: "split_removed" });
    const { mod, hook, act } = await load();
    act(() => {
      mod.notePartial("Gym", partial(["remove", "check"]));
    });
    await act(() => mod.settleSplit("s1", "remove", "Gym"));
    expect(hook.result.current.notices).toEqual([
      {
        id: 1,
        tone: "ok",
        text: "Removed the new part of “Gym”. The repeating event is as it was.",
      },
    ]);
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(hook.result.current.notices).toEqual([]);
  });

  it("makes a notice for a settle from the dialog, and keeps the buttons when no answer came", async () => {
    settleSeriesSplit.mockRejectedValueOnce(new Error("PM's backend didn't answer."));
    const { mod, hook, act } = await load();
    await act(() => mod.settleSplit("p", "check", "Gym"));
    expect(hook.result.current.notices).toHaveLength(1);
    expect(hook.result.current.notices[0]).toMatchObject({
      tone: "error",
      text: "“Gym”: PM's backend didn't answer.",
      split: { token: "p", offers: ["check"], title: "Gym" },
    });
  });

  it("keeps a notice's own offers when its settle gets no answer", async () => {
    const { mod, hook, act } = await load();
    act(() => {
      mod.notePartial("Gym", partial(["finish", "remove"]));
    });
    const id = hook.result.current.notices[0].id;
    settleSeriesSplit.mockRejectedValueOnce(new Error("PM's backend didn't answer."));
    await act(() => mod.settleSplit("s1", "finish", "Gym"));
    expect(hook.result.current.notices).toHaveLength(1);
    expect(hook.result.current.notices[0]).toMatchObject({
      id,
      tone: "error",
      split: { token: "s1", offers: ["finish", "remove"] },
    });
  });

  it("gives a delete an earlier change blocked that change's buttons, never 'Settle that first'", async () => {
    deleteCalendarEvent.mockResolvedValueOnce({
      outcome: "refused",
      result: partial(["finish"]),
    });
    deleteCalendarEvent.mockResolvedValueOnce({
      outcome: "held",
      undo_token: "t2",
      undo_seconds: 8,
    });
    const { mod, hook, act } = await load();
    await act(async () => {
      expect(await mod.startDelete(occurrence, { scope: "all", token: "a" })).toBe("refused");
    });
    expect(hook.result.current.notices[0]).toMatchObject({
      text: "“Gym” wasn't deleted. PM is still finishing an earlier change to it from 02-11. Finish it first.",
      split: { token: "s1", offers: ["finish"] },
    });
    // Held, then blocked at the send: its "Deleting" notice gives way to the one already up.
    await act(() => mod.startDelete(occurrence, { scope: "all", token: "a" }));
    expect(hook.result.current.notices).toHaveLength(2);
    act(() =>
      settledHandler!({ undo_token: "t2", event_id: occurrence.id, result: partial(["finish"]) }),
    );
    expect(hook.result.current.notices).toHaveLength(1);
    expect(hook.result.current.notices[0].split?.token).toBe("s1");
    expect(hook.result.current.notices.some((n) => /Settle/.test(n.text))).toBe(false);
  });

  it("keeps words said once until they're dismissed", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteSeriesNotice("PM closed before it finished changing this repeating event."));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(hook.result.current.notices).toEqual([
      { id: 1, tone: "warn", text: "PM closed before it finished changing this repeating event." },
    ]);
  });

  it("says how to change a repeating save back, with no Undo", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteSaved("Gym", { outcome: "saved", warnings: [] }, "this"));
    expect(hook.result.current.notices).toEqual([
      {
        id: 1,
        tone: "ok",
        text: "Saved “Gym” for this occurrence. To change it back, edit it again.",
      },
    ]);
    expect(hook.result.current.notices[0].undoToken).toBeUndefined();
    // A one-off's words are as they were.
    act(() => mod.noteSaved("Dentist", { outcome: "saved", warnings: [] }));
    expect(hook.result.current.notices[1].text).toBe("Saved “Dentist”.");
  });
});
