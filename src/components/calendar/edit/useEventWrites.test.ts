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
  GuestNotice,
  HeldDeleteInfo,
  MilestoneMoves,
  NewEventDraft,
  Notify,
  Rsvp,
  RsvpScope,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
  WriteWarning,
} from "../../../lib/types";

type Scope = string | null;
/** Send or Don't send, and the numbers it was given for (C12). */
type Told = [notify: Notify | null, asked: GuestNotice | null];
const deleteCalendarEvent =
  vi.fn<
    (id: string, seen: unknown, scope: Scope, choice: Scope, ...told: Told) => Promise<DeleteStart>
  >();
const cancelCalendarDelete = vi.fn<(token: string) => Promise<boolean>>();
const listHeldDeletes = vi.fn<() => Promise<HeldDeleteInfo[]>>();
const updateCalendarEvent =
  vi.fn<
    (
      session: string,
      draft: EventPatchDraft,
      scope: Scope,
      choice: Scope,
      ...told: Told
    ) => Promise<WriteOutcome>
  >();
const seriesChoices = vi.fn<(request: ChoiceRequest) => Promise<SeriesChoices>>();
const respondToCalendarEvent =
  vi.fn<
    (
      eventId: string,
      response: Rsvp,
      comment: string | null,
      scope: RsvpScope | null,
    ) => Promise<WriteOutcome>
  >();
const settleSeriesSplit = vi.fn<(token: string, action: SplitAction) => Promise<WriteOutcome>>();
const createCalendarEvent =
  vi.fn<
    (calendarId: string, draft: NewEventDraft, token: Scope, ...told: Told) => Promise<WriteOutcome>
  >();
const writeLastCreate = vi.fn<(calendarId: string) => void>();
const getSeriesSummary = vi.fn<(eventId: string) => Promise<string | null>>();
const setMilestoneEvent =
  vi.fn<
    (id: number, uid: string | null, date: string | null, only: string | null) => Promise<void>
  >();
let settledHandler: ((s: DeleteSettled) => void) | null = null;

vi.mock("../../../lib/calendarPrefs", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  writeLastCreate: (calendarId: string) => writeLastCreate(calendarId),
}));

vi.mock("../../../lib/ipc", () => ({
  // Every argument passes through, so an extra one, or a dropped token, shows in the calls.
  createCalendarEvent: (...a: [string, NewEventDraft, Scope, ...Told]) => createCalendarEvent(...a),
  getSeriesSummary: (eventId: string) => getSeriesSummary(eventId),
  setMilestoneEvent: (id: number, uid: string | null, date: string | null, only: string | null) =>
    setMilestoneEvent(id, uid, date, only),
  deleteCalendarEvent: (id: string, seen: unknown, scope: Scope, choice: Scope, ...told: Told) =>
    deleteCalendarEvent(id, seen, scope, choice, ...told),
  cancelCalendarDelete: (token: string) => cancelCalendarDelete(token),
  listHeldDeletes: () => listHeldDeletes(),
  updateCalendarEvent: (
    session: string,
    draft: EventPatchDraft,
    scope: Scope,
    choice: Scope,
    ...told: Told
  ) => updateCalendarEvent(session, draft, scope, choice, ...told),
  seriesChoices: (request: ChoiceRequest) => seriesChoices(request),
  // Every argument passes through, as given.
  respondToCalendarEvent: (...a: [string, Rsvp, string | null, RsvpScope | null]) =>
    respondToCalendarEvent(...a),
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

/** One guest already on it, told of a change (C12). */
const GUESTS: GuestNotice = {
  updated: 1,
  invited: 0,
  removed: 0,
  told_cancelled: 0,
  cancelled: false,
  people: 1,
  emails: 1,
  complete: true,
};
/** The same guest, told it's cancelled. */
const TOLD: GuestNotice = { ...GUESTS, cancelled: true };

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
      // Nobody to email: no answer, no numbers.
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
    // Each read's failure, beside `reads`.
    let fails: (() => void)[] = [];

    /** A fresh store, read through the hook the calendar reads the mirror with, under StrictMode,
     *  which mounts every effect twice. `applied`: what each read that was applied found; `failed`:
     *  told when the newest read threw. */
    async function loadCalendar() {
      vi.resetModules();
      const mod = await import("./useEventWrites");
      const { StrictMode } = await import("react");
      const { renderHook, act } = await import("@testing-library/react");
      const applied: CalendarEvent[][] = [];
      const read = () =>
        new Promise<CalendarEvent[]>((resolve, reject) => {
          reads.push(resolve);
          fails.push(() => reject(new Error("no mirror")));
        });
      const apply = (events: CalendarEvent[]) => applied.push(events);
      const failed = vi.fn();
      const hook = renderHook(
        () => ({
          writes: mod.useEventWrites(),
          load: mod.useMirrorReads(read, apply, failed),
        }),
        { wrapper: StrictMode },
      );
      await act(async () => {});
      return { mod, hook, act, applied, failed, renderHook };
    }

    beforeEach(() => {
      reads = [];
      fails = [];
      deleteCalendarEvent.mockResolvedValue({
        outcome: "held",
        undo_token: "t1",
        undo_seconds: 8,
        hidden: later,
      });
    });

    it("shows again a row the delete left in Google, once a read after it lands", async () => {
      const { mod, hook, act } = await loadCalendar();
      await act(() => mod.startDelete(occurrence, { scope: "following", token: "f" }));
      // The read the backend's landing starts, begun before the webview hears the settle.
      const early = hook.result.current.load();
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
      expect([...hook.result.current.writes.hiddenIds].sort()).toEqual([...all].sort());
      // Google kept the occurrence changed on its own; the rest went.
      await act(async () => reads[1](rows([later[0]])));
      expect(hook.result.current.writes.hiddenIds.size).toBe(0);
    });

    // C9b-1b review F4: an older read landing after a newer one began is dropped whole. It would
    // otherwise release rows it no longer has while the calendar still shows the list from before
    // the delete, painting the deleted event again until the newer read lands.
    it("keeps a deleted row hidden when an older read lands first", async () => {
      deleteCalendarEvent.mockResolvedValue({ outcome: "held", undo_token: "t2", undo_seconds: 8 });
      const { mod, hook, act, applied } = await loadCalendar();
      await act(() => mod.startDelete(event));
      const early = hook.result.current.load();
      act(() =>
        settledHandler!({
          undo_token: "t2",
          event_id: event.id,
          result: { outcome: "saved", warnings: [] },
        }),
      );
      expect(reads).toHaveLength(2);
      await act(async () => {
        reads[0](rows([]));
        await early;
      });
      expect(applied).toHaveLength(0);
      expect(hook.result.current.writes.hiddenIds.has(event.id)).toBe(true);
      await act(async () => reads[1](rows([])));
      expect(applied).toEqual([[]]);
      expect(hook.result.current.writes.hiddenIds.size).toBe(0);
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
      expect([...hook.result.current.writes.hiddenIds].sort()).toEqual([...all].sort());
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
      expect(hook.result.current.writes.hiddenIds.has(event.id)).toBe(true);
    });

    // C9b-1b review round 2 F3: a read started before the Calendar tab unmounted lands on nothing.
    // Applied, it would release rows by its old mark, and hand the next mount its older list.
    it("drops a read that lands after the calendar unmounted", async () => {
      deleteCalendarEvent.mockResolvedValue({ outcome: "held", undo_token: "t2", undo_seconds: 8 });
      const { mod, hook, act, applied, renderHook } = await loadCalendar();
      await act(() => mod.startDelete(event));
      act(() =>
        settledHandler!({
          undo_token: "t2",
          event_id: event.id,
          result: { outcome: "saved", warnings: [] },
        }),
      );
      // The settle's own read, still on its way when the tab goes.
      expect(reads).toHaveLength(1);
      hook.unmount();
      await act(async () => reads[0](rows([])));
      expect(applied).toHaveLength(0);
      const back = renderHook(() => mod.useEventWrites());
      expect(back.result.current.hiddenIds.has(event.id)).toBe(true);
    });

    it("says the newest read failed, never an older one", async () => {
      const { hook, act, failed } = await loadCalendar();
      const first = hook.result.current.load();
      const second = hook.result.current.load();
      await act(async () => {
        fails[0]();
        await first;
      });
      expect(failed).not.toHaveBeenCalled();
      await act(async () => {
        fails[1]();
        await second;
      });
      expect(failed).toHaveBeenCalledTimes(1);
    });

    it("never says a read failed once the calendar unmounted", async () => {
      const { hook, act, failed } = await loadCalendar();
      const read = hook.result.current.load();
      hook.unmount();
      await act(async () => {
        fails[0]();
        await read;
      });
      expect(failed).not.toHaveBeenCalled();
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
      result: { outcome: "needs_choice", scope: true },
    });
    const { mod, hook, act } = await load();
    await act(async () => {
      expect(await mod.startDelete(occurrence)).toEqual({ ask: null, again: false });
      // Chosen, but the plan changed since: ask again, too.
      expect(await mod.startDelete(occurrence, { scope: "all", token: "a" })).toEqual({
        ask: null,
        again: false,
      });
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

  // C12: whether Google emails the guests is no choice of occurrences: the caller asks it, with the
  // backend's numbers, and nothing is held or said meanwhile.
  it("answers ask with the guests' numbers when the backend wants Send or Don't send", async () => {
    deleteCalendarEvent.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "needs_choice", scope: false, guests: TOLD },
    });
    const { mod, hook, act } = await load();
    await act(async () => {
      expect(await mod.startDelete(event)).toEqual({ ask: TOLD, again: false });
    });
    deleteCalendarEvent.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "needs_choice", scope: false, guests: TOLD, again: true },
    });
    await act(async () => {
      expect(await mod.startDelete(event, null, "all", GUESTS)).toEqual({ ask: TOLD, again: true });
    });
    expect(hook.result.current.notices).toEqual([]);
    expect(hook.result.current.hiddenIds.size).toBe(0);
  });

  // The delete dialog as the calendar runs it (C9b-1b review F1, F6, F8, F11; round 2 F1, F2): a
  // delete answered "choose" reopens it once, never over a dialog or the popover opened meanwhile
  // nor once the Calendar tab has gone, and a notice says to delete again otherwise, in words that
  // fit whether a choice was made. Run under StrictMode, which mounts every effect twice.
  describe("the delete dialog", () => {
    const ASK = {
      outcome: "refused",
      result: { outcome: "needs_choice", scope: true },
    } as DeleteStart;
    const CHANGED =
      "“Gym” wasn't deleted. This repeating event changed since you chose which occurrences to delete. Delete it again to choose.";
    const NEEDS =
      "“Gym” wasn't deleted. PM needs to ask which occurrences first. Delete it again to choose.";

    async function loadDialog() {
      vi.resetModules();
      const mod = await import("./useEventWrites");
      const { createElement, StrictMode, useRef } = await import("react");
      const { useDialogLayer } = await import("../../../lib/useDialogLayer");
      const { render, renderHook, act } = await import("@testing-library/react");
      const hook = renderHook(
        ({ popover }: { popover: boolean }) => ({
          writes: mod.useEventWrites(),
          dialog: mod.useDeleteDialog(() => popover),
        }),
        { initialProps: { popover: false }, wrapper: StrictMode },
      );
      await act(async () => {});
      /** Let a delete's answer reach the dialog. */
      const settle = () =>
        act(async () => {
          for (let i = 0; i < 10; i++) await Promise.resolve();
        });
      /** Some other dialog (the command palette, the editor), open until it unmounts. */
      function Other() {
        const ref = useRef<HTMLDivElement>(null);
        useDialogLayer(true, ref);
        return createElement("div", { ref });
      }
      const openOther = () => render(createElement(Other));
      return { mod, hook, act, settle, renderHook, openOther };
    }

    it("reopens once for a row that looked like a one-off, then says so", async () => {
      deleteCalendarEvent.mockResolvedValue(ASK);
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      expect(hook.result.current.dialog.asked).toEqual({ event: occurrence, reask: null });
      act(() => hook.result.current.dialog.confirm(occurrence, null));
      expect(hook.result.current.dialog.asked).toBeNull();
      await settle();
      expect(hook.result.current.dialog.asked).toEqual({
        event: occurrence,
        reask: { again: false },
      });
      expect(hook.result.current.writes.notices).toEqual([]);
      // Chosen this time, and the plan changed again: no third dialog.
      act(() => hook.result.current.dialog.confirm(occurrence, { scope: "this", token: "t" }));
      await settle();
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([CHANGED]);
    });

    // C12: the backend wants Send or Don't send: the dialog reopens once with its numbers (as a
    // one-off), and a second ask in a row is said instead, so nothing loops.
    it("reopens once for the guests' answer, then says so", async () => {
      deleteCalendarEvent.mockResolvedValue({
        outcome: "refused",
        result: { outcome: "needs_choice", scope: false, guests: TOLD },
      });
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(event));
      act(() => hook.result.current.dialog.confirm(event, null));
      await settle();
      expect(hook.result.current.dialog.asked).toEqual({
        event,
        reask: null,
        guests: { notice: TOLD, again: false },
      });
      deleteCalendarEvent.mockResolvedValue({
        outcome: "refused",
        result: { outcome: "needs_choice", scope: false, guests: TOLD, again: true },
      });
      act(() => hook.result.current.dialog.confirm(event, null, "all", TOLD));
      await settle();
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([
        "“Dentist” wasn't deleted. Who Google would email has changed since you chose. Delete it again to choose whether Google emails the guests.",
      ]);
    });

    // C12 critique C1-H2: guests Google added during the Undo window stop the delete at the send;
    // the dialog asks again with the backend's numbers, and the "Deleting" notice goes. With
    // something else open, a notice says to delete again instead. Each settles as the backend
    // answers: unanswered, a plain ask; answered for other numbers, `again` (C12c review).
    it("asks again when the send finds guests to tell", async () => {
      deleteCalendarEvent.mockResolvedValue(held);
      const { hook, act, settle, openOther } = await loadDialog();
      const sent = (token: string, again: boolean) =>
        act(() =>
          settledHandler?.({
            undo_token: token,
            event_id: event.id,
            result: { outcome: "needs_choice", scope: false, guests: TOLD, again },
          }),
        );
      act(() => hook.result.current.dialog.ask(event));
      act(() => hook.result.current.dialog.confirm(event, null));
      await settle();
      sent("t1", false);
      expect(hook.result.current.dialog.asked).toEqual({
        event,
        reask: null,
        guests: { notice: TOLD, again: false },
      });
      expect(hook.result.current.writes.notices).toEqual([]);
      expect(hook.result.current.writes.hiddenIds.size).toBe(0);
      // Answered, and the send found other numbers: reopened saying so.
      act(() => hook.result.current.dialog.close());
      deleteCalendarEvent.mockResolvedValue({ ...held, undo_token: "t2" });
      act(() => hook.result.current.dialog.confirm(event, null, "all", TOLD));
      await settle();
      sent("t2", true);
      expect(hook.result.current.dialog.asked).toEqual({
        event,
        reask: null,
        guests: { notice: TOLD, again: true },
      });
      expect(hook.result.current.writes.notices).toEqual([]);
      // The same with something else open: said instead, in the words for an answer given.
      act(() => hook.result.current.dialog.close());
      deleteCalendarEvent.mockResolvedValue({ ...held, undo_token: "t3" });
      act(() => hook.result.current.dialog.confirm(event, null, "all", TOLD));
      await settle();
      const other = openOther();
      sent("t3", true);
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([
        "“Dentist” wasn't deleted. Who Google would email has changed since you chose. Delete it again to choose whether Google emails the guests.",
      ]);
      // Unanswered, with something else open: said in the words for nobody having chosen.
      deleteCalendarEvent.mockResolvedValue({ ...held, undo_token: "t4" });
      act(() => hook.result.current.dialog.confirm(event, null));
      await settle();
      sent("t4", false);
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text).slice(1)).toEqual([
        "“Dentist” wasn't deleted. PM found guests who would be told. Delete it again to choose whether Google emails them.",
      ]);
      other.unmount();
    });

    // Occurrences chosen: their token binds who the delete tells, so a guests ask there is the plan
    // changing under the user, said as such, and never reopened as a one-off.
    it("says the plan changed when chosen occurrences are asked about guests", async () => {
      deleteCalendarEvent.mockResolvedValue({
        outcome: "refused",
        result: { outcome: "needs_choice", scope: false, guests: TOLD },
      });
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() =>
        hook.result.current.dialog.confirm(occurrence, { scope: "all", token: "a" }, "all"),
      );
      await settle();
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([CHANGED]);
    });

    it("reopens saying the plan changed when a choice was made", async () => {
      deleteCalendarEvent.mockResolvedValue(ASK);
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() => hook.result.current.dialog.confirm(occurrence, { scope: "all", token: "a" }));
      await settle();
      expect(hook.result.current.dialog.asked).toEqual({
        event: occurrence,
        reask: { again: true },
      });
    });

    it("never replaces a delete dialog opened while the delete was on its way", async () => {
      let answer: (s: DeleteStart) => void = () => {};
      deleteCalendarEvent.mockReturnValue(new Promise((r) => (answer = r)));
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() => hook.result.current.dialog.confirm(occurrence, { scope: "this", token: "t" }));
      act(() => hook.result.current.dialog.ask(event));
      answer(ASK);
      await settle();
      expect(hook.result.current.dialog.asked).toEqual({ event, reask: null });
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([CHANGED]);
    });

    it("never opens over a dialog opened while the delete was on its way", async () => {
      let answer: (s: DeleteStart) => void = () => {};
      deleteCalendarEvent.mockReturnValue(new Promise((r) => (answer = r)));
      const { hook, act, settle, openOther } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() => hook.result.current.dialog.confirm(occurrence, null));
      const other = openOther();
      answer(ASK);
      await settle();
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([NEEDS]);
      other.unmount();
    });

    it("never opens over the event popover opened while the delete was on its way", async () => {
      let answer: (s: DeleteStart) => void = () => {};
      deleteCalendarEvent.mockReturnValue(new Promise((r) => (answer = r)));
      const { hook, act, settle } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() => hook.result.current.dialog.confirm(occurrence, null));
      hook.rerender({ popover: true });
      answer(ASK);
      await settle();
      expect(hook.result.current.dialog.asked).toBeNull();
      expect(hook.result.current.writes.notices.map((n) => n.text)).toEqual([NEEDS]);
    });

    // Round 2 F1: the tab router unmounts the calendar while a delete is on its way. The answer
    // can't open the dialog, so the notice waits in the store for the tab's return.
    it("says to delete again when the Calendar tab went away while the delete was on its way", async () => {
      let answer: (s: DeleteStart) => void = () => {};
      deleteCalendarEvent.mockReturnValue(new Promise((r) => (answer = r)));
      const { mod, hook, act, settle, renderHook } = await loadDialog();
      act(() => hook.result.current.dialog.ask(occurrence));
      act(() => hook.result.current.dialog.confirm(occurrence, { scope: "all", token: "a" }));
      hook.unmount();
      answer(ASK);
      await settle();
      const back = renderHook(() => mod.useEventWrites());
      expect(back.result.current.notices.map((n) => n.text)).toEqual([CHANGED]);
    });
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
      null,
      null,
    );
    await act(async () => {
      await mod.saveEdit("sess", changes, { scope: "all", token: "a" }, "Gym");
      await mod.saveEdit("sess", changes, null, "Gym");
    });
    expect(updateCalendarEvent.mock.calls).toEqual([
      ["sess", changes, "all", "a", null, null],
      ["sess", changes, null, null, null, null],
    ]);
    // The very draft the choices were asked for goes to the save (the token binds it).
    expect(updateCalendarEvent.mock.calls[0][1]).toBe(changes);
  });

  // C12 §14.4: Send or Don't send, and the numbers it was given for, reach both writes.
  it("sends the email answer with a save and a delete", async () => {
    deleteCalendarEvent.mockResolvedValue({ outcome: "held", undo_token: "t1", undo_seconds: 8 });
    updateCalendarEvent.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { mod, act } = await load();
    const changes: EventPatchDraft = { summary: "Retro" };
    await act(async () => {
      await mod.saveEdit("sess", changes, null, "Gym", "all", GUESTS);
      await mod.startDelete(event, null, "none", TOLD);
    });
    expect(updateCalendarEvent).toHaveBeenLastCalledWith(
      "sess",
      changes,
      null,
      null,
      "all",
      GUESTS,
    );
    expect(deleteCalendarEvent).toHaveBeenLastCalledWith(
      event.id,
      expect.anything(),
      null,
      null,
      "none",
      TOLD,
    );
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
    // "This and following" from the first occurrence saved every one, and says so (C11c review).
    act(() =>
      mod.noteSaved(
        "Gym",
        { outcome: "saved", warnings: [] },
        "following",
        "2026-10-05",
        true,
        true,
      ),
    );
    expect(hook.result.current.notices[2].text).toBe(
      "Saved “Gym” as one event that doesn't repeat, on 05-10. There's no Undo: to make it repeat again, edit it and pick a repeat.",
    );
  });
});

// --- creating an event (#884, C10) ---

const CAL = "gcal:me@x.com:me@x.com";
const draft: NewEventDraft = {
  summary: "Dentist",
  time: {
    kind: "timed",
    start_date: "2026-10-12",
    start_time: "09:00",
    start_zone: "Europe/London",
    end_date: "2026-10-12",
    end_time: "10:00",
    end_zone: "Europe/London",
  },
  location: "",
  description: "",
  show_as: "busy",
  visibility: "default",
  recurrence: null,
  guests: [],
  reminders: null,
  colour: null,
};
const sent = { calendarId: CAL, draft };
const shown = { name: "Me", hidden: false };
const savedOutcome = (warnings: WriteWarning[] = []): WriteOutcome => ({
  outcome: "saved",
  warnings,
  left_behind: [],
});
/** One guest new to the event, invited. */
const ONE_INVITED: GuestNotice = {
  updated: 0,
  invited: 1,
  removed: 0,
  told_cancelled: 0,
  cancelled: false,
  people: 1,
  emails: 1,
  complete: true,
};

describe("replying to an invitation (C14)", () => {
  it("sends the reply with its note and scope as given, and hands back the outcome unchanged", async () => {
    const outcome: WriteOutcome = {
      outcome: "saved",
      warnings: ["kept_own_reply"],
      left_behind: ["2026-10-19"],
    };
    respondToCalendarEvent.mockResolvedValue(outcome);
    const { hook } = await load();
    const got = await hook.result.current.respond(event.id, "maybe", "late", "all");
    expect(respondToCalendarEvent).toHaveBeenCalledTimes(1);
    expect(respondToCalendarEvent).toHaveBeenCalledWith(event.id, "maybe", "late", "all");
    expect(got).toBe(outcome);
    // Left out, the note and scope go as null: keep Google's note, and no scope chosen.
    await hook.result.current.respond(event.id, "no");
    expect(respondToCalendarEvent).toHaveBeenLastCalledWith(event.id, "no", null, null);
    // A reply asks nothing first and holds nothing: no notice, no row hidden.
    expect(hook.result.current.notices).toEqual([]);
    expect(hook.result.current.hiddenIds.size).toBe(0);
  });
});

describe("creating an event", () => {
  // C12d: and the Send or Don't send for its guests, with the numbers it was given for; none
  // without one.
  it("createEvent passes the calendar id, the draft, the token and the answer", async () => {
    const { mod } = await load();
    createCalendarEvent.mockResolvedValue(savedOutcome());
    await mod.createEvent(CAL, draft, null);
    await mod.createEvent(CAL, draft, "ref-1");
    await mod.createEvent(CAL, draft, null, { notify: "all", asked: ONE_INVITED });
    expect(createCalendarEvent.mock.calls).toEqual([
      [CAL, draft, null, null, null],
      [CAL, draft, "ref-1", null, null],
      [CAL, draft, null, "all", ONE_INVITED],
    ]);
  });

  // The held create keeps its answer: Check again sends it with the draft (C12d).
  it("Check again sends the answer a new event with guests was sent with", async () => {
    const { mod, hook, act } = await load();
    const answered = { ...sent, answer: { notify: "none" as const, asked: ONE_INVITED } };
    act(() => mod.noteHeldCreate(answered, "ref-1"));
    const id = hook.result.current.notices[0].id;
    createCalendarEvent.mockResolvedValue(savedOutcome());
    await act(async () => {
      await mod.checkCreate(id, () => shown);
    });
    expect(createCalendarEvent.mock.calls).toEqual([[CAL, draft, "ref-1", "none", ONE_INVITED]]);
  });

  it("notes each way a create can end, and remembers the calendar only when it's made", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteCreateResult(sent, null, savedOutcome(), shown));
    expect(hook.result.current.notices).toEqual([
      { id: 1, tone: "ok", text: "Created “Dentist”." },
    ]);
    expect(writeLastCreate).toHaveBeenCalledWith(CAL);
    act(() => mod.noteCreateResult(sent, null, savedOutcome(), { name: "Me", hidden: true }));
    act(() => mod.noteCreateResult(sent, null, savedOutcome(["outside_mirror"]), shown));
    act(() => mod.noteCreateResult(sent, null, savedOutcome(["mirror_refresh_pending"]), shown));
    expect(hook.result.current.notices.slice(1).map((n) => [n.tone, n.text])).toEqual([
      ["warn", "Created “Dentist” in Me, which is hidden here."],
      [
        "warn",
        "Created “Dentist” in Google. It starts outside the months PM syncs, so it won't show here.",
      ],
      ["ok", "Created “Dentist” in Google. PM's calendar will show it after the next refresh."],
    ]);
    writeLastCreate.mockClear();
    act(() =>
      mod.noteCreateResult(
        sent,
        null,
        { outcome: "failed", message: "PM couldn't reach Google." },
        shown,
      ),
    );
    const notices = hook.result.current.notices;
    expect(notices[notices.length - 1]).toEqual({
      id: 5,
      tone: "error",
      text: "“Dentist” wasn't created: PM couldn't reach Google.",
    });
    expect(writeLastCreate).not.toHaveBeenCalled();
    // Done-and-fine notices go by themselves; the warnings stay.
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(hook.result.current.notices.map((n) => n.tone)).toEqual(["warn", "warn", "error"]);
  });

  it("keeps an ambiguous create with Check again, for good, holding what was sent", async () => {
    const { mod, hook, act } = await load();
    act(() =>
      mod.noteCreateResult(sent, null, { outcome: "ambiguous", retry_token: "ref-1" }, shown),
    );
    const [notice] = hook.result.current.notices;
    expect(notice.tone).toBe("warn");
    expect(notice.text).toMatch(/^PM lost touch with Google while creating “Dentist”/);
    expect(notice.create).toEqual({ token: "ref-1", sent });
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(hook.result.current.notices).toHaveLength(1);
    // A surface closing over the same token doesn't say it twice.
    act(() => mod.noteHeldCreate(sent, "ref-1"));
    expect(hook.result.current.notices).toEqual([
      {
        id: notice.id,
        tone: "warn",
        text: "PM may already have made “Dentist”. Check again: PM looks for it in Google first and sends it again only if it isn't there.",
        create: { token: "ref-1", sent },
      },
    ]);
  });

  it("Check again sends exactly what was sent, under its token, once at a time", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteHeldCreate(sent, "ref-1"));
    const id = hook.result.current.notices[0].id;
    let answer: (o: WriteOutcome) => void = () => {};
    createCalendarEvent.mockImplementation(
      () => new Promise<WriteOutcome>((resolve) => (answer = resolve)),
    );
    const whereOf = vi.fn(() => shown);
    let first: Promise<void> = Promise.resolve();
    act(() => {
      first = mod.checkCreate(id, whereOf);
    });
    expect(hook.result.current.notices[0].checking).toBe(true);
    // A second press while it runs sends nothing.
    await act(async () => {
      await mod.checkCreate(id, whereOf);
    });
    // No guests, no answer (C12d).
    expect(createCalendarEvent.mock.calls).toEqual([[CAL, draft, "ref-1", null, null]]);
    await act(async () => {
      answer(savedOutcome());
      await first;
    });
    expect(hook.result.current.notices).toEqual([{ id, tone: "ok", text: "Created “Dentist”." }]);
    expect(writeLastCreate).toHaveBeenCalledWith(CAL);
    expect(whereOf).toHaveBeenCalledWith(CAL);
  });

  it("keeps Check again on every answer that doesn't end the token", async () => {
    for (const outcome of [
      { outcome: "ambiguous", retry_token: "ref-1" },
      { outcome: "reauth" },
      { outcome: "busy" },
      { outcome: "read_only", reason: "editing_off" },
      {
        outcome: "failed",
        message: "PM has already sent this new event to Google more than once.",
      },
    ] as WriteOutcome[]) {
      const { mod, hook, act } = await load();
      act(() => mod.noteHeldCreate(sent, "ref-1"));
      const id = hook.result.current.notices[0].id;
      createCalendarEvent.mockResolvedValue(outcome);
      await act(async () => {
        await mod.checkCreate(id, () => shown);
      });
      const [notice] = hook.result.current.notices;
      expect(notice.create).toEqual({ token: "ref-1", sent });
      expect(notice.checking).toBeUndefined();
      expect(notice.text).not.toMatch(/wasn't created/);
    }
  });

  it("ends Check again on gone, or a token PM no longer holds", async () => {
    const lostWords =
      "PM lost touch with Google and can't tell whether this new event was made. Look for it in Google Calendar before creating it again, so it isn't made twice.";
    for (const [outcome, text] of [
      [{ outcome: "gone" }, "PM made “Dentist”, but it has since been deleted in Google."],
      [
        { outcome: "failed", message: lostWords },
        "PM lost touch with Google and can't tell whether “Dentist” was made. Look for it in Google Calendar before creating it again, so it isn't made twice.",
      ],
    ] as [WriteOutcome, string][]) {
      const { mod, hook, act } = await load();
      act(() => mod.noteHeldCreate(sent, "ref-1"));
      const id = hook.result.current.notices[0].id;
      createCalendarEvent.mockResolvedValue(outcome);
      await act(async () => {
        await mod.checkCreate(id, () => shown);
      });
      expect(hook.result.current.notices).toEqual([{ id, tone: "warn", text }]);
    }
  });

  it("keeps Check again when no answer comes at all", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteHeldCreate(sent, "ref-1"));
    const id = hook.result.current.notices[0].id;
    createCalendarEvent.mockRejectedValue(new Error("The app's channel closed."));
    await act(async () => {
      await mod.checkCreate(id, () => shown);
    });
    const [notice] = hook.result.current.notices;
    expect(notice.create).toEqual({ token: "ref-1", sent });
    expect(notice.text).toMatch(/The app's channel closed\.$/);
  });
});

// C9b-2: the popover's words, and the milestones a "this and following" change offers to move.
describe("how a repeating event repeats, and moving its milestones", () => {
  const moves: MilestoneMoves = {
    new_series_ical: "tail@google.com",
    earlier_series_ical: "abc@google.com",
    new_title: "Retro",
    earlier_title: "Gym",
    first_day: "2026-10-19",
    earlier_last_day: "2026-10-12",
    today: "2026-10-20",
    milestones: [
      { id: 7, label: "Launch", project: "Atlas", date: "2026-10-19" },
      { id: 8, label: "Review", project: "Atlas", date: "2026-10-26" },
    ],
  };
  const one: MilestoneMoves = { ...moves, milestones: [moves.milestones[0]] };
  const landed = (m: MilestoneMoves, warnings: WriteWarning[] = []): WriteOutcome => ({
    outcome: "saved",
    warnings,
    milestone_moves: m,
  });

  beforeEach(() => {
    vi.setSystemTime(new Date("2026-10-20T12:00:00Z"));
    setMilestoneEvent.mockResolvedValue(undefined);
  });

  /** A store with Launch's offer up: the store, and the offer's notice id. */
  async function offered() {
    const loaded = await load();
    loaded.act(() => loaded.mod.noteSaved("Retro", landed(one), "following", "2026-10-19"));
    const offer = loaded.hook.result.current.notices.find((n) => n.move);
    const notice = () => loaded.hook.result.current.notices.find((n) => n.id === offer!.id);
    return { ...loaded, id: offer!.id, notice };
  }

  it("describeSeries shares one request per row and answers null on a throw", async () => {
    const { mod } = await load();
    let answer!: (words: string | null) => void;
    getSeriesSummary.mockReturnValueOnce(
      new Promise((resolve) => {
        answer = resolve;
      }),
    );
    const first = mod.describeSeries("row");
    const second = mod.describeSeries("row");
    expect(getSeriesSummary).toHaveBeenCalledTimes(1);
    answer("Weekly on Monday");
    await expect(first).resolves.toBe("Weekly on Monday");
    await expect(second).resolves.toBe("Weekly on Monday");
    // Answered, so a later call asks again; a throw is null ("Repeats").
    getSeriesSummary.mockRejectedValueOnce(new Error("offline"));
    await expect(mod.describeSeries("row")).resolves.toBeNull();
    expect(getSeriesSummary).toHaveBeenCalledTimes(2);
  });

  it("a landed split offers one notice per milestone, that stays", async () => {
    const { mod, hook, act } = await load();
    act(() => mod.noteSaved("Retro", landed(moves), "following", "2026-10-19"));
    const [, launch, review] = hook.result.current.notices;
    expect(launch).toMatchObject({
      tone: "ok",
      text: "Move the “Launch” milestone in Atlas to the new part of “Retro”, which starts on 19-10? If you don't, it stays on the earlier part, which ended on 12-10, and shows as overdue.",
      move: {
        milestoneId: 7,
        newTitle: "Retro",
        earlierTitle: "Gym",
        toIcal: "tail@google.com",
        toDate: "2026-10-19",
        backIcal: "abc@google.com",
        backDate: "2026-10-19",
      },
    });
    expect(review.move).toMatchObject({ milestoneId: 8, backDate: "2026-10-26" });
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    // The saved notice went by itself; the questions wait for an answer.
    expect(hook.result.current.notices.map((n) => n.move?.milestoneId)).toEqual([7, 8]);
  });

  // C9b-2 review round 3, R3-F3: whether the earlier part has ended goes by today as the backend
  // read it in the user's zone, the day its dates are in, not this device's clock (20-10 here).
  it("words the earlier part's end by the day the change landed", async () => {
    const { mod, hook, act } = await load();
    const ahead = { ...one, earlier_last_day: "2026-10-21", today: "2026-10-20" };
    const ended = { ...one, earlier_last_day: "2026-10-21", today: "2026-10-22" };
    act(() => mod.noteSaved("Retro", landed(ahead), "following", "2026-10-19"));
    act(() => mod.noteSaved("Retro", landed(ended), "following", "2026-10-19"));
    const offers = hook.result.current.notices.filter((n) => n.move).map((n) => n.text);
    expect(offers).toEqual([
      expect.stringMatching(/, whose last day is 21-10, and shows as overdue after that\.$/),
      expect.stringMatching(/, which ended on 21-10, and shows as overdue\.$/),
    ]);
  });

  it("no milestone_moves, no offer", async () => {
    settleSeriesSplit.mockResolvedValueOnce({
      outcome: "partial_split",
      token: "s1",
      offers: ["finish"],
      from: "2026-10-19",
    });
    const { mod, hook, act } = await load();
    act(() => mod.noteSaved("Retro", { outcome: "saved", warnings: [] }, "following"));
    act(() => mod.noteSaved("Retro", { outcome: "failed", message: "No." }, "following"));
    await act(() => mod.settleSplit("s1", "check", "Gym"));
    expect(hook.result.current.notices.some((n) => n.move || n.moveBack)).toBe(false);
  });

  it("a finished split offers too", async () => {
    settleSeriesSplit.mockResolvedValueOnce(landed(one));
    const { mod, hook, act } = await load();
    await act(() => mod.settleSplit("s1", "finish", "Gym"));
    expect(hook.result.current.notices.map((n) => n.move?.milestoneId)).toEqual([undefined, 7]);
  });

  it("a new part PM couldn't find says which milestone stayed, with nothing to press", async () => {
    const { mod, hook, act } = await load();
    const unnamed = { ...one, new_series_ical: null, earlier_last_day: null };
    act(() =>
      mod.noteSaved("Retro", landed(unnamed, ["milestone_not_offered"]), "following", "2026-10-19"),
    );
    expect(hook.result.current.notices[1]).toEqual({
      id: 2,
      tone: "warn",
      text: "PM couldn't find the new part of “Retro” in Google, so “Launch” in Atlas stays on the earlier part, which has ended, and shows as overdue. Unlink it in Atlas to set its date yourself.",
    });
  });

  it("Move sends the new part and its first day, then offers Move back for 8 s", async () => {
    const { mod, act, id, notice } = await offered();
    await act(async () => {
      expect(await mod.moveMilestone(id)).toBe(true);
    });
    // Only while it's still on the earlier part.
    expect(setMilestoneEvent).toHaveBeenCalledWith(
      7,
      "tail@google.com",
      "2026-10-19",
      "abc@google.com",
    );
    expect(notice()).toMatchObject({
      tone: "ok",
      text: "Moved “Launch” to the new part of “Retro”.",
      moveBack: { milestoneId: 7 },
    });
    expect(notice()?.move).toBeUndefined();
    act(() => {
      vi.advanceTimersByTime(7_999);
    });
    expect(notice()?.moveBack).toBeDefined();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(notice()).toEqual({
      id,
      tone: "ok",
      text: "Moved “Launch” to the new part of “Retro”.",
    });
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(notice()).toBeUndefined();
  });

  it("Move back sends the earlier part and the milestone's date", async () => {
    const { mod, act, id, notice } = await offered();
    await act(() => mod.moveMilestone(id));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    await act(async () => {
      expect(await mod.moveMilestoneBack(id)).toBe(true);
    });
    expect(setMilestoneEvent).toHaveBeenLastCalledWith(
      7,
      "abc@google.com",
      "2026-10-19",
      "tail@google.com",
    );
    expect(notice()).toEqual({
      id,
      tone: "ok",
      text: "“Launch” is back on the earlier part of “Gym”.",
    });
    // The window it had is closed: nothing comes back later.
    act(() => {
      vi.advanceTimersByTime(8_000);
    });
    expect(notice()).toBeUndefined();
  });

  it("Move back waits while the pointer or focus is on its notice, then has what it had left", async () => {
    const { mod, act, id, notice } = await offered();
    // Focus on the Move button as it's pressed: the window starts held.
    act(() => mod.holdMoveBack(id, "focus", true));
    await act(() => mod.moveMilestone(id));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(notice()?.moveBack).toBeDefined();
    act(() => mod.holdMoveBack(id, "pointer", true));
    act(() => mod.holdMoveBack(id, "focus", false));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    expect(notice()?.moveBack).toBeDefined();
    act(() => mod.holdMoveBack(id, "pointer", false));
    act(() => {
      vi.advanceTimersByTime(3_000);
    });
    act(() => mod.holdMoveBack(id, "pointer", true));
    act(() => {
      vi.advanceTimersByTime(60_000);
    });
    act(() => mod.holdMoveBack(id, "pointer", false));
    act(() => {
      vi.advanceTimersByTime(4_999);
    });
    expect(notice()?.moveBack).toBeDefined();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(notice()?.moveBack).toBeUndefined();
  });

  it("a second press while moving sends nothing", async () => {
    let finish!: () => void;
    setMilestoneEvent.mockReturnValueOnce(
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
    );
    const { mod, act, id, notice } = await offered();
    let first!: Promise<boolean>;
    act(() => {
      first = mod.moveMilestone(id);
    });
    expect(notice()?.moving).toBe(true);
    await act(async () => {
      expect(await mod.moveMilestone(id)).toBe(false);
    });
    expect(setMilestoneEvent).toHaveBeenCalledTimes(1);
    finish();
    await act(async () => {
      expect(await first).toBe(true);
    });
    expect(notice()?.moving).toBeFalsy();
  });

  it("Move back takes no press in its first moment, where one meant for Move lands", async () => {
    const { mod, act, id, notice } = await offered();
    await act(() => mod.moveMilestone(id));
    await act(async () => {
      expect(await mod.moveMilestoneBack(id)).toBe(false);
    });
    act(() => {
      vi.advanceTimersByTime(499);
    });
    await act(async () => {
      expect(await mod.moveMilestoneBack(id)).toBe(false);
    });
    expect(setMilestoneEvent).toHaveBeenCalledTimes(1);
    expect(notice()?.moveBack).toBeDefined();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    await act(async () => {
      expect(await mod.moveMilestoneBack(id)).toBe(true);
    });
    expect(setMilestoneEvent).toHaveBeenCalledTimes(2);
  });

  it("a moved notice dismissed while Move back is offered stays gone", async () => {
    const { mod, act, id, notice } = await offered();
    await act(() => mod.moveMilestone(id));
    act(() => mod.dismissNotice(id));
    expect(notice()).toBeUndefined();
    // Its window ends, and nothing comes back, then or later.
    act(() => {
      vi.advanceTimersByTime(8_000);
    });
    expect(notice()).toBeUndefined();
    act(() => {
      vi.advanceTimersByTime(5_000);
    });
    expect(notice()).toBeUndefined();
  });

  it("a failed move keeps the offer with the error", async () => {
    setMilestoneEvent.mockRejectedValueOnce(new Error("That milestone no longer exists."));
    const { mod, act, id, notice } = await offered();
    await act(async () => {
      expect(await mod.moveMilestone(id)).toBe(false);
    });
    expect(notice()).toMatchObject({
      tone: "error",
      text: "Couldn't move “Launch”. That milestone no longer exists.",
      move: { milestoneId: 7 },
    });
    expect(notice()?.moving).toBeFalsy();
  });

  it("a failed move back says the way out, with nothing to press", async () => {
    const { mod, act, id, notice } = await offered();
    await act(() => mod.moveMilestone(id));
    act(() => {
      vi.advanceTimersByTime(500);
    });
    setMilestoneEvent.mockRejectedValueOnce(
      new Error("That milestone changed since; nothing was moved."),
    );
    await act(async () => {
      expect(await mod.moveMilestoneBack(id)).toBe(false);
    });
    expect(notice()).toEqual({
      id,
      tone: "error",
      text: "Couldn't move “Launch” back. That milestone changed since; nothing was moved. Unlink it in Atlas if it shouldn't follow the new part.",
    });
  });

  it("Ctrl+Z never moves a milestone", async () => {
    const { mod, act, id } = await offered();
    expect(mod.undoLatest()).toBe(false);
    expect(setMilestoneEvent).not.toHaveBeenCalled();
    await act(() => mod.moveMilestone(id));
    expect(mod.undoLatest()).toBe(false);
    expect(setMilestoneEvent).toHaveBeenCalledTimes(1);
  });
});
