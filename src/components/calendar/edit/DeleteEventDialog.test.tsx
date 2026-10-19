// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// Deleting an event (#884): a one-off confirms as it always has; a repeating one asks which
// occurrences first, like Google Calendar, from Google's answer for the row the user saw, and the
// delete names the occurrences picked with their token. Words the choices say once are never lost.

import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  ScopeRow,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";

const choicesForDelete = vi.fn<(event: CalendarEvent) => Promise<SeriesChoices>>();
const settleSplit =
  vi.fn<(token: string, action: SplitAction, title: string) => Promise<WriteOutcome | null>>();
const noteSeriesNotice = vi.fn();

vi.mock("./useEventWrites", () => ({
  choicesForDelete: (event: CalendarEvent) => choicesForDelete(event),
  settleSplit: (token: string, action: SplitAction, title: string) =>
    settleSplit(token, action, title),
  noteSeriesNotice: (text: string) => noteSeriesNotice(text),
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { DeleteEventDialog } from "./DeleteEventDialog";

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
};
const occurrence: CalendarEvent = { ...oneOff, series_id: "abc", recurring: true };
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
const NOTICE = "PM closed before it finished an earlier change to this repeating event.";

function open(event: CalendarEvent, props: { again?: boolean; forceRepeating?: boolean } = {}) {
  const onConfirm = vi.fn();
  const onClose = vi.fn();
  const utils = render(
    <DeleteEventDialog
      event={event}
      calendarName="Personal"
      when="12-10 · 10:00–11:00"
      onConfirm={onConfirm}
      onClose={onClose}
      {...props}
    />,
  );
  return { ...utils, onConfirm, onClose };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-18T12:00:00"));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("DeleteEventDialog", () => {
  it("confirms a one-off as before, asking Google nothing", () => {
    const { onConfirm } = open(oneOff);
    expect(screen.getByRole("dialog", { name: "Delete this event?" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledWith(oneOff, null);
    expect(choicesForDelete).not.toHaveBeenCalled();
  });

  it("asks which occurrences of a repeating one, and deletes those with their token", async () => {
    let answer: (c: SeriesChoices) => void = () => {};
    choicesForDelete.mockReturnValue(new Promise((r) => (answer = r)));
    const { onConfirm } = open(occurrence);
    expect(screen.getByRole("dialog", { name: "Delete repeating event" })).toBeTruthy();
    expect(screen.getByText("Checking “Gym” in Google…")).toBeTruthy();
    const del = screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement;
    expect(del.disabled).toBe(true);
    expect(choicesForDelete).toHaveBeenCalledWith(occurrence);
    await act(async () => answer(ready()));
    // It names when and where, and which one "This event" is.
    expect(
      screen.getByText(/“Gym” \(12-10 · 10:00–11:00\) should be deleted from Personal/),
    ).toBeTruthy();
    expect(screen.getByText("Only the one on 12-10.")).toBeTruthy();
    expect(radio(/^This event/).checked).toBe(true);
    fireEvent.click(radio(/^This and following events/));
    expect(screen.getByText(/can't restore these from its Trash/)).toBeTruthy();
    fireEvent.click(del);
    expect(onConfirm).toHaveBeenCalledWith(occurrence, { scope: "following", token: "f" });
    // The held Undo still applies.
    expect(screen.getByText(/You can undo it for a few seconds/)).toBeTruthy();
  });

  it("says Google changed it, offering no rows", async () => {
    choicesForDelete.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "conflict", fields: ["summary"] },
    });
    open(occurrence);
    expect(
      await screen.findByText(
        "“Gym” wasn't deleted. It changed in Google since you opened it (the title).",
      ),
    ).toBeTruthy();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect((screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
  });

  // C9b-1b review F7: the delete is checked against PM's calendar, which still says it repeats, so
  // a one-event confirm could never delete it; it says to refresh instead.
  it("says to refresh when PM's calendar says it repeats and Google says it doesn't", async () => {
    choicesForDelete.mockResolvedValue({ outcome: "not_repeating" });
    const { onConfirm } = open(occurrence);
    expect(await screen.findByText("Refresh the calendar, then try again.")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Delete this event?" })).toBeNull();
    expect((screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // C9b-1 critique 2 L8: reopened because the backend wanted a choice, a "doesn't repeat" answer
  // would only lead back here, so it says to refresh instead.
  it("says to refresh when asked again and Google says it doesn't repeat", async () => {
    choicesForDelete.mockResolvedValue({ outcome: "not_repeating" });
    const { onConfirm } = open(oneOff, { forceRepeating: true });
    expect(await screen.findByText("Refresh the calendar, then try again.")).toBeTruthy();
    expect(screen.queryByRole("dialog", { name: "Delete this event?" })).toBeNull();
    expect((screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("asks a row that looked like a one-off when forced, and says why when asked again", async () => {
    choicesForDelete.mockResolvedValue(ready());
    open(oneOff, { forceRepeating: true, again: true });
    await screen.findByRole("radio", { name: /^This event/ });
    expect(
      screen.getByText(/changed since you chose which occurrences to delete\. Choose again/),
    ).toBeTruthy();
  });

  it("settles an earlier change from the dialog, then closes", async () => {
    choicesForDelete.mockResolvedValue(
      ready({
        notice: NOTICE,
        following: { row: "pending", token: "p", offers: ["finish"], from: "2026-11-02" },
        all: { row: "pending", token: "p", offers: ["finish"], from: "2026-11-02" },
      }),
    );
    settleSplit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { onClose, onConfirm } = open(occurrence);
    fireEvent.click(await screen.findByRole("button", { name: "Finish" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(settleSplit).toHaveBeenCalledWith("p", "finish", "Gym");
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  // C9b-1b review F5: Cancel stays open while a settle is on its way. Closed meanwhile, the settle's
  // end closes nothing more: by then another delete's dialog may be up.
  it("closes nothing more when a settle ends after the dialog was closed", async () => {
    choicesForDelete.mockResolvedValue(
      ready({
        following: { row: "pending", token: "p", offers: ["finish"], from: "2026-11-02" },
        all: { row: "pending", token: "p", offers: ["finish"], from: "2026-11-02" },
      }),
    );
    let settled: (r: WriteOutcome | null) => void = () => {};
    settleSplit.mockReturnValue(new Promise((r) => (settled = r)));
    const { onClose, rerender } = open(occurrence);
    fireEvent.click(await screen.findByRole("button", { name: "Finish" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    const props = { calendarName: "Personal", when: "12-10", onConfirm: vi.fn(), onClose };
    rerender(<DeleteEventDialog event={null} {...props} />);
    rerender(<DeleteEventDialog event={{ ...occurrence, id: "other" }} {...props} />);
    await act(async () => settled({ outcome: "saved", warnings: [] }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("dialog", { name: "Delete repeating event" })).toBeTruthy();
  });

  it("keeps the words said once when closed without a choice, and only then", async () => {
    choicesForDelete.mockResolvedValue(ready({ notice: NOTICE }));
    const first = open(occurrence);
    expect(await screen.findByText(NOTICE)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    expect(first.onClose).toHaveBeenCalled();
    first.unmount();
    noteSeriesNotice.mockClear();
    // Chosen: the words were read beside the rows.
    const second = open(occurrence);
    await screen.findByText(NOTICE);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(second.onConfirm).toHaveBeenCalled();
    expect(noteSeriesNotice).not.toHaveBeenCalled();
  });

  it("keeps the words of an answer that arrives after the dialog closed", async () => {
    let answer: (c: SeriesChoices) => void = () => {};
    choicesForDelete.mockReturnValue(new Promise((r) => (answer = r)));
    const { unmount } = open(occurrence);
    unmount();
    await act(async () => answer(ready({ notice: NOTICE })));
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
  });

  // StrictMode mounts the effect twice; the store hands both the same answer, which shows once.
  it("shows one answer once under StrictMode", async () => {
    choicesForDelete.mockReturnValue(Promise.resolve(ready({ notice: NOTICE })));
    render(
      <StrictMode>
        <DeleteEventDialog
          event={occurrence}
          calendarName="Personal"
          when="12-10"
          onConfirm={vi.fn()}
          onClose={vi.fn()}
        />
      </StrictMode>,
    );
    expect(await screen.findByText(NOTICE)).toBeTruthy();
    expect(noteSeriesNotice).not.toHaveBeenCalled();
  });

  it("keeps late words as one notice under StrictMode", async () => {
    let answer: (c: SeriesChoices) => void = () => {};
    choicesForDelete.mockReturnValue(new Promise((r) => (answer = r)));
    const { unmount } = render(
      <StrictMode>
        <DeleteEventDialog
          event={occurrence}
          calendarName="Personal"
          when="12-10"
          onConfirm={vi.fn()}
          onClose={vi.fn()}
        />
      </StrictMode>,
    );
    unmount();
    await act(async () => answer(ready({ notice: NOTICE })));
    expect(noteSeriesNotice).toHaveBeenCalledTimes(1);
  });
});
