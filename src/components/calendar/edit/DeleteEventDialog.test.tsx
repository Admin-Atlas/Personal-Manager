// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// Deleting an event (#884): a one-off confirms as it always has; a repeating one asks which
// occurrences first, like Google Calendar, from Google's answer for the row the user saw, and the
// delete names the occurrences picked with their token. Words the choices say once are never lost.
// One with guests asks whether Google emails them, Delete waiting for the answer (C12).

import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  GuestNotice,
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

function open(
  event: CalendarEvent,
  props: {
    again?: boolean;
    forceRepeating?: boolean;
    from?: string;
    removeFrom?: string;
    guests?: { notice: GuestNotice; again: boolean };
  } = {},
) {
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
    expect(onConfirm).toHaveBeenCalledWith(oneOff, null, null, null);
    expect(choicesForDelete).not.toHaveBeenCalled();
    // No guests to tell: no email question.
    expect(screen.queryByRole("radio")).toBeNull();
  });

  // Help mode (C20b §11, critique C2-M6): a one-off's delete explains itself; there are no
  // occurrences to choose between.
  it("carries the delete's help on a one-off", () => {
    open(oneOff);
    expect(
      screen.getByRole("dialog", { name: "Delete this event?" }).getAttribute("data-help"),
    ).toBe("calendar-delete");
  });

  // C12 §14.3: a one-off you organise with a guest asks whether Google emails the cancellation, in
  // the same dialog; Delete waits for the answer and focus starts on Cancel.
  it("asks a one-off with guests whether to email them, Delete waiting for the answer", () => {
    const withGuest: CalendarEvent = {
      ...oneOff,
      organizer_self: true,
      attendees: [
        {
          name: null,
          email: "me@x.com",
          response: "accepted",
          optional: false,
          organizer: true,
          self: true,
        },
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
    const { onConfirm } = open(withGuest, { from: "me@x.com" });
    expect(screen.getByText("Send cancellation emails to guests?")).toBeTruthy();
    expect(screen.getByText("1 guest will be told it's cancelled.")).toBeTruthy();
    expect(screen.getByText("Google sends the emails from me@x.com.")).toBeTruthy();
    const del = screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement;
    expect(del.disabled).toBe(true);
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(del);
    expect(onConfirm).not.toHaveBeenCalled();
    fireEvent.click(radio(/^Don't send/));
    expect(del.disabled).toBe(false);
    fireEvent.click(del);
    expect(onConfirm).toHaveBeenCalledWith(withGuest, null, "none", {
      updated: 1,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 1,
      emails: 1,
      complete: true,
    });
  });

  // C12 critique C1-H2: reopened with the backend's numbers (Google's copy had guests the row
  // didn't), it asks with those, as a one-off, saying so when an answer came for others.
  it("asks with the backend's numbers when reopened for the guests", () => {
    const notice = {
      updated: 2,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 2,
      emails: 2,
      complete: true,
    };
    const { onConfirm } = open(occurrence, { guests: { notice, again: true } });
    expect(screen.getByRole("dialog", { name: "Delete this event?" })).toBeTruthy();
    expect(choicesForDelete).not.toHaveBeenCalled();
    expect(screen.getByText("2 guests will be told it's cancelled.")).toBeTruthy();
    expect(
      screen.getByText(
        "It wasn't deleted. PM checked with Google before deleting, and these are the numbers it found.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/PM found guests/)).toBeNull();
    fireEvent.click(radio(/^Send/));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledWith(occurrence, null, "all", notice);
  });

  // C12c review: reopened for guests when nobody had answered (Google's copy has guests the row
  // didn't show, found before the hold or at the send), it says the delete didn't happen and why,
  // rather than coming back as if nothing had been tried. Round 2: the words stay true when the
  // backend asks from PM's older row after Google dropped the guests, so not "it has guests now".
  it("says it wasn't deleted when reopened for guests nobody had answered about", () => {
    const notice: GuestNotice = {
      updated: 1,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 1,
      emails: 1,
      complete: true,
    };
    const { onConfirm } = open(oneOff, { from: "me@x.com", guests: { notice, again: false } });
    expect(
      screen.getByText(
        "It wasn't deleted. PM found guests who would be told. Choose whether Google emails them.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/has guests now/)).toBeNull();
    expect(screen.queryByText(/PM checked with Google/)).toBeNull();
    expect(screen.getByText("1 guest will be told it's cancelled.")).toBeTruthy();
    fireEvent.click(radio(/^Don't send/));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onConfirm).toHaveBeenCalledWith(oneOff, null, "none", notice);
  });

  it("says nothing of a delete that didn't happen when it opens the first time", () => {
    open(oneOff);
    expect(screen.queryByText(/wasn't deleted/)).toBeNull();
  });

  // An answer is for the numbers on screen: new ones ask afresh (R9).
  it("asks afresh when the numbers change under an answer", () => {
    const notice: GuestNotice = {
      updated: 2,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 2,
      emails: 2,
      complete: true,
    };
    const props = {
      event: oneOff,
      calendarName: "Personal",
      when: "12-10",
      onConfirm: vi.fn(),
      onClose: vi.fn(),
    };
    const { rerender } = render(<DeleteEventDialog {...props} guests={{ notice, again: false }} />);
    fireEvent.click(radio(/^Send/));
    const del = screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement;
    expect(del.disabled).toBe(false);
    const three = { ...notice, updated: 3, people: 3, emails: 3 };
    rerender(<DeleteEventDialog {...props} guests={{ notice: three, again: true }} />);
    expect(screen.getByText("3 guests will be told it's cancelled.")).toBeTruthy();
    expect(radio(/^Send/).checked).toBe(false);
    expect(del.disabled).toBe(true);
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
    expect(onConfirm).toHaveBeenCalledWith(
      occurrence,
      { scope: "following", token: "f", asAll: false },
      null,
      null,
    );
    // The held Undo still applies.
    expect(screen.getByText(/You can undo it for a few seconds/)).toBeTruthy();
  });

  // C12 §14.3: a row that would tell guests asks there, and the answer goes with the token.
  it("passes the email answer of a repeating one's row", async () => {
    const notice: GuestNotice = {
      updated: 1,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 1,
      emails: 1,
      complete: true,
    };
    choicesForDelete.mockResolvedValue(
      ready({ all: { row: "offered", warnings: [], token: "a", guests: notice } }),
    );
    const { onConfirm } = open(occurrence, { from: "me@x.com" });
    fireEvent.click(await screen.findByRole("radio", { name: /^All events/ }));
    const del = screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement;
    expect(del.disabled).toBe(true);
    fireEvent.click(radio(/^Send/));
    fireEvent.click(del);
    expect(onConfirm).toHaveBeenCalledWith(
      occurrence,
      { scope: "all", token: "a", asAll: false },
      "all",
      null,
    );
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

  // C20 §6.5: Google's own help says deleting a birthday from Calendar leaves Contacts alone; PM
  // says so too, for every year (the choices, "This event" refused by the backend) or a one-off.
  it("deleting a birthday says Google Contacts keeps it", async () => {
    const contacts = /Deleting it here doesn't change Google Contacts or your Google Account\./;
    const yearly: CalendarEvent = { ...occurrence, summary: "Mum", event_type: "birthday" };
    const thisYear: ScopeRow = {
      row: "refused",
      message:
        "PM deletes a birthday or other special date for every year at once: choose “All events”.",
    };
    choicesForDelete.mockResolvedValue(ready({ this: thisYear }));
    const { unmount } = open(yearly);
    await screen.findByRole("radio", { name: /^All events/ });
    expect(screen.getByText(contacts)).toBeTruthy();
    expect(radio(/^This event/).disabled).toBe(true);
    unmount();
    open({ ...oneOff, event_type: "birthday" });
    expect(screen.getByText(contacts)).toBeTruthy();
  });

  it("a Gmail event's delete says nothing about Contacts", async () => {
    choicesForDelete.mockResolvedValue(ready());
    const { unmount } = open({ ...occurrence, event_type: "fromGmail" });
    await screen.findByRole("radio", { name: /^All events/ });
    expect(screen.queryByText(/Google Contacts/)).toBeNull();
    unmount();
    open({ ...oneOff, event_type: "fromGmail" });
    expect(screen.queryByText(/Google Contacts/)).toBeNull();
  });
});

// Someone else's event (C14 §10.4, §13.4): it asks to remove it, says who hears, and asks nothing
// about emails; a repeating one goes whole, waiting for the plan's token.
describe("DeleteEventDialog, an invitation", () => {
  const invitation: CalendarEvent = { ...oneOff, organizer_self: false };
  const repeating: CalendarEvent = { ...occurrence, organizer_self: false };
  const TITLE = "Remove “Gym” from your calendar?";
  const REFUSED =
    "Google Calendar removes someone else's repeating event from your calendar all at once, so choose “All events”.";
  const refused: ScopeRow = { row: "refused", message: REFUSED };
  const plan = (over: Partial<Extract<SeriesChoices, { outcome: "ready" }>> = {}) =>
    ready({ this: refused, following: refused, ...over });
  const remove = () => screen.getByRole<HTMLButtonElement>("button", { name: "Remove" });

  it("asks to remove it, says the organiser will be told, and asks nothing about emails", () => {
    const { onConfirm, onClose } = open({
      ...invitation,
      attendees: [
        {
          email: "boss@x.org",
          name: null,
          response: "accepted",
          optional: false,
          organizer: true,
          self: false,
        },
      ],
    });
    const dialog = screen.getByRole("dialog", { name: TITLE });
    expect(dialog.textContent).toContain(
      "Google Calendar tells the organiser you declined. You can undo it for a few seconds.",
    );
    expect(dialog.getAttribute("data-help")).toBe("calendar-delete");
    expect(screen.queryByRole("radio")).toBeNull();
    expect(choicesForDelete).not.toHaveBeenCalled();
    // Focus on Cancel, so Enter can't remove it, however the body grows.
    const cancel = screen.getByRole("button", { name: "Cancel" });
    expect(document.activeElement).toBe(cancel);
    expect(cancel.hasAttribute("data-autofocus")).toBe(true);
    fireEvent.click(remove());
    expect(onConfirm).toHaveBeenCalledWith(
      expect.objectContaining({ id: invitation.id }),
      null,
      null,
      null,
    );
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("a repeating invitation removes all of it and waits for the plan", async () => {
    let answer: (c: SeriesChoices) => void = () => {};
    choicesForDelete.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const { onConfirm } = open(repeating);
    const dialog = screen.getByRole("dialog", { name: TITLE });
    expect(dialog.textContent).toContain(
      "Google Calendar tells the organiser you declined. This removes every occurrence, as Google Calendar does. You can undo it for a few seconds.",
    );
    expect(dialog.getAttribute("data-help")).toBe("calendar-delete");
    expect(dialog.textContent).toContain("Checking “Gym” in Google…");
    expect(remove().disabled).toBe(true);
    expect(screen.queryByText("You can remove it now.")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => answer(plan({ all: offered("the-token") })));
    expect(dialog.textContent).not.toContain("Checking");
    expect(remove().disabled).toBe(false);
    // "Checking" going says nothing, so the status region says Remove is on (focus stays on Cancel).
    const said = screen.getByText("You can remove it now.");
    expect(said.className).toContain("sr-only");
    expect(said.closest("[role=status]")).toBe(within(dialog).getByRole("status"));
    // No occurrences to choose between: it isn't the choices dialog.
    expect(screen.queryByRole("radio")).toBeNull();
    fireEvent.click(remove());
    expect(onConfirm).toHaveBeenCalledWith(
      repeating,
      { scope: "all", token: "the-token" },
      null,
      null,
    );
  });

  it("this and following from the first occurrence doesn't stand in for All events", async () => {
    choicesForDelete.mockResolvedValue(
      plan({ following: { row: "as_all", warnings: [], token: "f" }, all: offered("a") }),
    );
    const { onConfirm } = open(repeating);
    await waitFor(() => expect(remove().disabled).toBe(false));
    fireEvent.click(remove());
    expect(onConfirm).toHaveBeenCalledWith(repeating, { scope: "all", token: "a" }, null, null);
  });

  it("a refused All events row says why and keeps Remove off", async () => {
    const why = "A part PM split off this repeating event is still there.";
    choicesForDelete.mockResolvedValue(plan({ all: { row: "refused", message: why } }));
    open(repeating);
    expect(await screen.findByText(why)).toBeTruthy();
    expect(remove().disabled).toBe(true);
    expect(screen.queryByText("You can remove it now.")).toBeNull();
  });

  // Whose guest entry declines, where "your calendar" and "you" wouldn't say: the strip's own names.
  it("names the calendar in the question and in what the organiser hears", async () => {
    const { unmount } = open(invitation, { removeFrom: "Boss" });
    const asked = screen.getByRole("dialog", { name: "Remove “Gym” from Boss?" });
    expect(asked.textContent).toContain(
      "Google Calendar tells the organiser Boss declined. You can undo it for a few seconds.",
    );
    expect(asked.textContent).not.toContain("you declined");
    unmount();
    choicesForDelete.mockResolvedValue(plan({ all: offered("a") }));
    open(repeating, { removeFrom: "Boss" });
    expect(screen.getByRole("dialog", { name: "Remove “Gym” from Boss?" }).textContent).toContain(
      "Google Calendar tells the organiser Boss declined. This removes every occurrence",
    );
    await waitFor(() => expect(remove().disabled).toBe(false));
  });

  it("promises every occurrence only where the plan doesn't say it was split", async () => {
    choicesForDelete.mockResolvedValue(
      plan({ all: { row: "offered", warnings: [{ warning: "split_sibling" }], token: "a" } }),
    );
    open(repeating);
    await waitFor(() => expect(remove().disabled).toBe(false));
    const dialog = screen.getByRole("dialog", { name: TITLE });
    expect(dialog.textContent).toContain("This removes every occurrence of this part.");
    expect(dialog.textContent).not.toContain("as Google Calendar does");
    expect(dialog.textContent).not.toContain("delete");
    expect(
      screen.getByText(
        /Google may remove that part too\. If it stays, you can remove it the same way\./,
      ),
    ).toBeTruthy();
  });

  it("says Google changed it, and keeps Remove off", async () => {
    choicesForDelete.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "conflict", fields: ["summary"] },
    });
    open(repeating);
    expect(
      await screen.findByText(
        "“Gym” wasn't removed. It changed in Google since you opened it (the title).",
      ),
    ).toBeTruthy();
    expect(remove().disabled).toBe(true);
  });

  it("says why when the plan can't be read, and keeps Remove off", async () => {
    choicesForDelete.mockRejectedValue(new Error("no store"));
    open(repeating);
    expect(await screen.findByText("“Gym” wasn't removed. no store")).toBeTruthy();
    expect(remove().disabled).toBe(true);
  });

  it("says to refresh when PM's calendar says it repeats and Google says it doesn't", async () => {
    choicesForDelete.mockResolvedValue({ outcome: "not_repeating" });
    open(repeating);
    expect(await screen.findByText("Refresh the calendar, then try again.")).toBeTruthy();
    expect(remove().disabled).toBe(true);
  });

  it("shows the plan's warning, and says why when asked again", async () => {
    choicesForDelete.mockResolvedValue(
      plan({ all: { row: "offered", warnings: [{ warning: "split_sibling" }], token: "a" } }),
    );
    open(repeating, { again: true });
    expect(
      await screen.findByText(
        "This repeating event changed since you asked to remove it. Nothing was removed.",
      ),
    ).toBeTruthy();
    expect(screen.getByText(/Google may remove that part too\./)).toBeTruthy();
    expect(remove().disabled).toBe(false);
  });

  it("asks a row that looked like a one-off when forced", async () => {
    choicesForDelete.mockResolvedValue(plan({ all: offered("a") }));
    const { onConfirm } = open(invitation, { forceRepeating: true });
    await waitFor(() => expect(remove().disabled).toBe(false));
    expect(screen.getByRole("dialog", { name: TITLE }).textContent).toContain("every occurrence");
    fireEvent.click(remove());
    expect(onConfirm).toHaveBeenCalledWith(invitation, { scope: "all", token: "a" }, null, null);
  });

  it("keeps the words said once when closed without removing", async () => {
    choicesForDelete.mockResolvedValue(plan({ all: offered("a"), notice: NOTICE }));
    open(repeating);
    expect(await screen.findByText(NOTICE)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
  });

  // A change PM couldn't finish holds it: its buttons are the choices dialog's.
  it("settles an earlier change before removing", async () => {
    settleSplit.mockResolvedValue(null);
    choicesForDelete.mockResolvedValue(
      plan({ all: { row: "pending", token: "s", offers: ["check"], from: "2026-11-02" } }),
    );
    const { onClose } = open(repeating);
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await waitFor(() => expect(settleSplit).toHaveBeenCalledWith("s", "check", "Gym"));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  // C20 §0.4 item 11: a birthday's or a Gmail event's organiser is Google's: deleted, not removed.
  it("a birthday or a Gmail event in someone else's name asks to delete it", async () => {
    for (const event_type of ["birthday", "fromGmail"]) {
      const { unmount } = open({ ...invitation, event_type });
      expect(screen.getByRole("dialog", { name: "Delete this event?" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
      unmount();
      choicesForDelete.mockResolvedValue(ready());
      const series = open({ ...repeating, event_type });
      expect(await screen.findByRole("dialog", { name: "Delete repeating event" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
      series.unmount();
    }
  });
});
