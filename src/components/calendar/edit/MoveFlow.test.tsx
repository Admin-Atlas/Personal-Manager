// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// What PM asks after a drop (#884, C16b), over a mocked write hook: a repeating event asks which
// occurrences with "This event" picked, one with guests whether Google emails them with nothing
// picked, and Back puts it back. A question the backend asks after the answer is asked again once,
// with its numbers; a second puts the event back with a notice to drag it again. Words said once
// are never lost, and a flow that goes away unanswered puts its event back (but not under
// StrictMode's double mount).

import { StrictMode } from "react";
import {
  act,
  cleanup,
  fireEvent,
  render,
  renderHook,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  GuestNotice,
  Notify,
  ScopeRow,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import type { Picked } from "../../../lib/calendarEdit/saveChoices";
import type { Timing } from "../../../lib/calendarDrag";
import { A11_NOTE, MOVE_CHECKED, MOVE_NEW_GUESTS } from "../../../lib/calendarEdit/editReasons";
import { PUT_BACK } from "../../../lib/calendarGesture";
import type { Drop } from "./useEventWrites";

const writes = vi.hoisted(() => ({
  beginMove: vi.fn(),
  choicesForMove: vi.fn(),
  moveEvent: vi.fn(),
  cancelMove: vi.fn(),
  noteSeriesNotice: vi.fn(),
  settleSplit: vi.fn(),
}));
vi.mock("./useEventWrites", () => ({
  beginMove: (drop: Drop) => writes.beginMove(drop) as string,
  dropOf: (event: CalendarEvent, from: Timing, to: Timing, resize: boolean): Drop => ({
    ...dropOf(event),
    from,
    to,
    resize,
  }),
  choicesForMove: (drop: Drop) => writes.choicesForMove(drop) as Promise<SeriesChoices>,
  moveEvent: (
    drop: Drop,
    picked: Picked | null,
    notify: Notify | null,
    asked: GuestNotice | null,
  ) => writes.moveEvent(drop, picked, notify, asked) as Promise<WriteOutcome>,
  cancelMove: (id: string) => writes.cancelMove(id),
  noteSeriesNotice: (text: string) => writes.noteSeriesNotice(text),
  settleSplit: (token: string, action: SplitAction, title: string) =>
    writes.settleSplit(token, action, title) as Promise<WriteOutcome | null>,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { MoveFlow } from "./MoveFlow";
import { useMoveDrops, type DropsHost, type MoveAsk } from "./useMoveDrops";

const ME = "me@x.com";
const oneOff: CalendarEvent = {
  id: "gcal:me@x.com:me@x.com:abc",
  calendar_id: "gcal:me@x.com:me@x.com",
  summary: "Gym",
  description: null,
  location: null,
  start: "2026-10-12T09:00:00Z",
  end: "2026-10-12T10:00:00Z",
  all_day: false,
  html_link: null,
  uid: "abc@google.com",
  organizer_self: true,
};
const withGuest: CalendarEvent = {
  ...oneOff,
  attendees: [
    { name: null, email: ME, response: "accepted", optional: false, organizer: true, self: true },
    {
      name: "Ann",
      email: "ann@x.org",
      response: "accepted",
      optional: false,
      organizer: false,
      self: false,
    },
  ],
};
const occurrence: CalendarEvent = { ...oneOff, series_id: "abc", recurring: true };

const notice = (n: number): GuestNotice => ({
  updated: n,
  invited: 0,
  removed: 0,
  told_cancelled: 0,
  cancelled: false,
  people: n,
  emails: n,
  complete: true,
});
const offered = (token: string): ScopeRow => ({ row: "offered", warnings: [], token });
const ready = (
  over: Partial<Extract<SeriesChoices, { outcome: "ready" }>> = {},
): SeriesChoices => ({
  outcome: "ready",
  this: offered("t"),
  following: offered("f"),
  all: offered("a"),
  ...over,
});
const saved: WriteOutcome = { outcome: "saved", warnings: [] };
const asks = (over: Partial<Extract<WriteOutcome, { outcome: "needs_choice" }>>): WriteOutcome => ({
  outcome: "needs_choice",
  scope: false,
  ...over,
});

function dropOf(event: CalendarEvent): Drop {
  const from = {
    kind: "timed" as const,
    start: { day: "2026-10-12", min: 540 },
    end: { day: "2026-10-12", min: 600 },
  };
  const to = {
    kind: "timed" as const,
    start: { day: "2026-10-12", min: 600 },
    end: { day: "2026-10-12", min: 660 },
  };
  return {
    event,
    from,
    to,
    resize: false,
    seen: { start: event.start, end: event.end ?? null, all_day: event.all_day },
    moveTo: {
      kind: "timed",
      start_date: "2026-10-12",
      start_time: "10:00",
      end_date: "2026-10-12",
      end_time: "11:00",
    },
    zone: "Europe/London",
  };
}

function flow(event: CalendarEvent, more: Partial<MoveAsk> = {}, strict = false) {
  const announce = vi.fn();
  const onDone = vi.fn();
  const ask: MoveAsk = { drop: dropOf(event), flow: "move-1", ...more };
  const el = <MoveFlow ask={ask} account={ME} announce={announce} onDone={onDone} />;
  const utils = render(strict ? <StrictMode>{el}</StrictMode> : el);
  return { ...utils, ask, announce, onDone };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

beforeEach(() => {
  writes.beginMove.mockReturnValue("move-1");
  writes.moveEvent.mockResolvedValue(saved);
  writes.choicesForMove.mockResolvedValue(ready());
  writes.settleSplit.mockResolvedValue(saved);
});
afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("MoveFlow", () => {
  it("a one-off without guests moves at once", async () => {
    const { ask, onDone } = flow(oneOff);
    expect(writes.moveEvent).toHaveBeenCalledWith(ask.drop, null, null, null);
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(writes.cancelMove).not.toHaveBeenCalled();
  });

  it("with guests it asks Send or Don't send first and nothing is checked", async () => {
    const { ask, onDone } = flow(withGuest);
    expect(screen.getByRole("dialog", { name: "Send update emails to guests?" })).toBeTruthy();
    expect(radio(/^Send/).checked).toBe(false);
    expect(radio(/^Don't send/).checked).toBe(false);
    expect(button("Save").disabled).toBe(true);
    expect(document.activeElement).toBe(button("Back"));
    expect(writes.moveEvent).not.toHaveBeenCalled();
    fireEvent.click(radio(/^Send/));
    fireEvent.click(button("Save"));
    expect(writes.moveEvent).toHaveBeenCalledWith(ask.drop, null, "all", notice(1));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it("an invitation the organiser lets guests change says Google's warning in the question", () => {
    flow({
      ...withGuest,
      organizer_self: false,
      guests_can_modify: true,
      attendees: [
        {
          name: "Org",
          email: "org@x.org",
          response: "accepted",
          optional: false,
          organizer: true,
          self: false,
        },
        {
          name: null,
          email: ME,
          response: "accepted",
          optional: false,
          organizer: false,
          self: true,
        },
      ],
    });
    expect(screen.getByText(A11_NOTE)).toBeTruthy();
  });

  it("a repeating event asks which occurrences with this event picked", async () => {
    flow(occurrence);
    expect(screen.getByRole("dialog", { name: "Edit repeating event" })).toBeTruthy();
    await waitFor(() => expect(radio(/^This event/).checked).toBe(true));
    expect(document.activeElement).toBe(button("Back"));
    expect(writes.choicesForMove).toHaveBeenCalledTimes(1);
  });

  it("the series path sends its pick and no asked", async () => {
    const { ask, onDone } = flow(occurrence);
    await waitFor(() => expect(radio(/^All events/)).toBeTruthy());
    fireEvent.click(radio(/^All events/));
    fireEvent.click(button("Save"));
    expect(writes.moveEvent).toHaveBeenCalledWith(
      ask.drop,
      { scope: "all", token: "a", asAll: false },
      null,
      null,
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it("Back puts it back", () => {
    const { ask, announce, onDone } = flow(withGuest);
    fireEvent.click(button("Back"));
    expect(writes.cancelMove).toHaveBeenCalledWith(ask.drop.event.id);
    expect(announce).toHaveBeenCalledWith(PUT_BACK);
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(writes.moveEvent).not.toHaveBeenCalled();
  });

  it("needs_choice for guests asks again with the backend's numbers and the drag's words", async () => {
    // A one-off PM's calendar showed without guests was sent at once; Google's copy has two.
    const { ask } = flow(oneOff, { start: asks({ guests: notice(2) }) as never });
    expect(screen.getByText(MOVE_NEW_GUESTS)).toBeTruthy();
    expect(screen.getByText("2 guests already invited will get an update.")).toBeTruthy();
    fireEvent.click(radio(/^Don't send/));
    fireEvent.click(button("Save"));
    expect(writes.moveEvent).toHaveBeenCalledWith(ask.drop, null, "none", notice(2));
  });

  it("an answer for numbers that changed is asked again once, in the editor's words for a move", async () => {
    writes.moveEvent.mockResolvedValueOnce(asks({ guests: notice(3), again: true }));
    const { ask, onDone } = flow(withGuest);
    fireEvent.click(radio(/^Send/));
    fireEvent.click(button("Save"));
    await waitFor(() => expect(screen.getByText(MOVE_CHECKED)).toBeTruthy());
    expect(radio(/^Send/).checked).toBe(false);
    // A second change under the user: put back, said, never a third question.
    writes.moveEvent.mockResolvedValueOnce(asks({ guests: notice(4), again: true }));
    fireEvent.click(radio(/^Send/));
    fireEvent.click(button("Save"));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(writes.cancelMove).toHaveBeenCalledWith(ask.drop.event.id);
    expect(writes.noteSeriesNotice).toHaveBeenCalledWith(
      "“Gym” changed in Google while you were choosing, so it wasn't moved. Drag it again.",
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a second needs_choice after a pick puts it back with a notice", async () => {
    writes.moveEvent.mockResolvedValueOnce(asks({ scope: true }));
    const { ask, onDone } = flow(occurrence);
    await waitFor(() => expect(radio(/^This event/).checked).toBe(true));
    fireEvent.click(button("Save"));
    // The plan changed since the choice: asked once more, saying so.
    await waitFor(() => expect(writes.choicesForMove).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(radio(/^This event/).checked).toBe(true));
    expect(screen.getByText(/changed/i)).toBeTruthy();
    writes.moveEvent.mockResolvedValueOnce(asks({ scope: true }));
    fireEvent.click(button("Save"));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(writes.cancelMove).toHaveBeenCalledWith(ask.drop.event.id);
    expect(writes.noteSeriesNotice).toHaveBeenCalledWith(
      "“Gym” changed in Google while you were choosing, so it wasn't moved. Drag it again.",
    );
  });

  it("guests changing after the occurrences were chosen puts it back", async () => {
    writes.moveEvent.mockResolvedValueOnce(asks({ guests: notice(2) }));
    const { onDone } = flow(occurrence);
    await waitFor(() => expect(radio(/^This event/).checked).toBe(true));
    fireEvent.click(button("Save"));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(writes.cancelMove).toHaveBeenCalled();
    expect(writes.choicesForMove).toHaveBeenCalledTimes(1);
  });

  it("a one-off Google says repeats asks which occurrences", async () => {
    flow(oneOff, { start: asks({ scope: true }) as never });
    await waitFor(() => expect(radio(/^This event/).checked).toBe(true));
    expect(writes.choicesForMove).toHaveBeenCalledTimes(1);
  });

  it("not repeating any more moves it as one event", async () => {
    writes.choicesForMove.mockResolvedValue({ outcome: "not_repeating" });
    const { ask, onDone } = flow(occurrence);
    await waitFor(() => expect(writes.moveEvent).toHaveBeenCalledWith(ask.drop, null, null, null));
    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it("words said once become a notice when the dialog closes without a choice", async () => {
    const said = "PM closed before it finished an earlier change to this repeating event.";
    writes.choicesForMove.mockResolvedValue(ready({ notice: said }));
    flow(occurrence);
    await waitFor(() => expect(screen.getByText(said)).toBeTruthy());
    fireEvent.click(button("Back"));
    expect(writes.noteSeriesNotice).toHaveBeenCalledWith(said);
    expect(writes.cancelMove).toHaveBeenCalled();
  });

  it("a refusal shows its words, and Back puts it back", async () => {
    writes.choicesForMove.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "conflict", fields: ["start"] },
    });
    const { onDone } = flow(occurrence);
    await waitFor(() =>
      expect(screen.getByText(/changed in Google since PM last showed it/)).toBeTruthy(),
    );
    fireEvent.click(button("Back"));
    expect(writes.cancelMove).toHaveBeenCalled();
    expect(onDone).toHaveBeenCalled();
  });

  it("settling an earlier change puts the drop back", async () => {
    writes.choicesForMove.mockResolvedValue(
      ready({
        following: { row: "pending", token: "split-1", offers: ["finish"], from: "2026-10-12" },
      }),
    );
    const { onDone } = flow(occurrence);
    const finish = await screen.findByRole("button", { name: /Finish/ });
    fireEvent.click(finish);
    await waitFor(() =>
      expect(writes.settleSplit).toHaveBeenCalledWith("split-1", "finish", "Gym"),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(writes.cancelMove).toHaveBeenCalled();
    expect(writes.moveEvent).not.toHaveBeenCalled();
  });

  it("going away mid-question puts it back, but StrictMode's double mount doesn't", () => {
    vi.useFakeTimers({ toFake: ["setTimeout"] });
    const { ask, unmount } = flow(withGuest, {}, true);
    const lapse = () =>
      act(() => {
        vi.runAllTimers();
      });
    lapse();
    expect(writes.cancelMove).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Send update emails to guests?" })).toBeTruthy();
    unmount();
    lapse();
    expect(writes.cancelMove).toHaveBeenCalledWith(ask.drop.event.id);
  });

  it("an answer that asks after the flow went away puts it back and says so", async () => {
    let answer: (o: WriteOutcome) => void = () => {};
    writes.moveEvent.mockReturnValueOnce(new Promise<WriteOutcome>((r) => (answer = r)));
    const { unmount } = flow(withGuest);
    fireEvent.click(radio(/^Send/));
    fireEvent.click(button("Save"));
    unmount();
    // Sent: going away (its check runs on the next turn) didn't put it back.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(writes.cancelMove).not.toHaveBeenCalled();
    await act(async () => answer(asks({ guests: notice(2) })));
    expect(writes.cancelMove).toHaveBeenCalled();
    expect(writes.noteSeriesNotice).toHaveBeenCalledWith(
      "“Gym” wasn't moved. PM needs to ask about it first. Drag it again.",
    );
  });
});

// The calendar's drops (C16 critique C1-H2): one flow per drop, each closing only its own.
describe("useMoveDrops", () => {
  const commit = (row: CalendarEvent) => {
    const d = dropOf(row);
    return { effect: "commit" as const, row, from: d.from, to: d.to, resize: false };
  };
  function drops(over: Partial<DropsHost> = {}) {
    const host: DropsHost = {
      accountOf: () => ME,
      free: () => true,
      focus: vi.fn(),
      ...over,
    };
    return { host, ...renderHook(() => useMoveDrops(host)) };
  }
  const second: CalendarEvent = { ...oneOff, id: "gcal:me@x.com:me@x.com:def", summary: "Swim" };

  it("a drop that asks nothing is sent at once, and a second while the first saves is sent too", async () => {
    let first: (o: WriteOutcome) => void = () => {};
    writes.moveEvent.mockReturnValueOnce(new Promise<WriteOutcome>((r) => (first = r)));
    const { result, host } = drops();
    act(() => result.current.drop(commit(oneOff)));
    act(() => result.current.drop(commit(second)));
    expect(writes.beginMove).toHaveBeenCalledTimes(2);
    expect(writes.moveEvent).toHaveBeenCalledTimes(2);
    expect(writes.moveEvent.mock.calls.map(([d]) => (d as Drop).event.id)).toEqual([
      oneOff.id,
      second.id,
    ]);
    expect(host.focus).toHaveBeenLastCalledWith(second.id);
    await act(async () => first(saved));
    expect(result.current.ask).toBeNull();
  });

  it("a drop that asks opens its question and sends nothing yet", () => {
    const { result } = drops();
    act(() => result.current.drop(commit(occurrence)));
    expect(result.current.ask?.drop.event).toBe(occurrence);
    expect(result.current.ask?.flow).toBe("move-1");
    act(() => result.current.drop(commit(withGuest)));
    expect(result.current.ask?.drop.event).toBe(withGuest);
    expect(writes.moveEvent).not.toHaveBeenCalled();
  });

  it("an answer that asks is asked, with its question, when nothing else is open", async () => {
    const question = asks({ guests: notice(2) });
    writes.moveEvent.mockResolvedValueOnce(question);
    const { result } = drops();
    act(() => result.current.drop(commit(oneOff)));
    await waitFor(() => expect(result.current.ask?.start).toBe(question));
    expect(writes.cancelMove).not.toHaveBeenCalled();
  });

  it("the first answers needs_choice while the second's question is open: put back, said", async () => {
    let first: (o: WriteOutcome) => void = () => {};
    writes.moveEvent.mockReturnValueOnce(new Promise<WriteOutcome>((r) => (first = r)));
    const { result } = drops();
    act(() => result.current.drop(commit(oneOff)));
    act(() => result.current.drop(commit(occurrence)));
    const open = result.current.ask;
    await act(async () => first(asks({ scope: true })));
    expect(result.current.ask).toBe(open);
    expect(writes.cancelMove).toHaveBeenCalledWith(oneOff.id);
    expect(writes.noteSeriesNotice).toHaveBeenCalledWith(
      "“Gym” wasn't moved. PM needs to ask about it first. Drag it again.",
    );
  });

  it("an answer that asks while the details or a dialog are open, or after the calendar went, is put back", async () => {
    writes.moveEvent.mockResolvedValueOnce(asks({ scope: true }));
    const busy = drops({ free: () => false });
    act(() => busy.result.current.drop(commit(oneOff)));
    await waitFor(() => expect(writes.cancelMove).toHaveBeenCalledTimes(1));
    expect(busy.result.current.ask).toBeNull();
    let late: (o: WriteOutcome) => void = () => {};
    writes.moveEvent.mockReturnValueOnce(new Promise<WriteOutcome>((r) => (late = r)));
    const gone = drops();
    act(() => gone.result.current.drop(commit(second)));
    gone.unmount();
    await act(async () => late(asks({ scope: true })));
    expect(writes.cancelMove).toHaveBeenLastCalledWith(second.id);
  });

  it("a flow that ends late never closes a newer drop's", () => {
    const { result } = drops();
    act(() => result.current.drop(commit(occurrence)));
    const older = result.current.ask as MoveAsk;
    writes.beginMove.mockReturnValueOnce("move-2");
    act(() => result.current.drop(commit(withGuest)));
    const newer = result.current.ask;
    act(() => result.current.end(older));
    expect(result.current.ask).toBe(newer);
    act(() => result.current.end(newer as MoveAsk));
    expect(result.current.ask).toBeNull();
  });
});
