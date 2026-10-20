// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The full editor's create mode (#884, C10): one guard for every way to save (Save, Ctrl/Cmd+S,
// Enter in the title), nothing editable or closable while a save is out, an answer that never
// arrived shown locked as it was sent with Check again under the same token, a failure that keeps
// the draft, and the words that warn before a save PM's calendar won't show.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Calendar,
  CalendarAccount,
  CalendarOverview,
  NewEventDraft,
  WriteOutcome,
} from "../../../lib/types";
import { CREATE_LOST } from "../../../lib/calendarEdit/editReasons";
import type { EditorFields } from "../../../lib/calendarEdit/eventDraft";
import { createTargets } from "../../../lib/calendarEdit/createTargets";

const createEvent =
  vi.fn<
    (calendarId: string, draft: NewEventDraft, token: string | null) => Promise<WriteOutcome>
  >();
const noteCreateResult = vi.fn();
const noteHeldCreate = vi.fn();
let deviceZone: string | null = "Europe/London";

vi.mock("./useEventWrites", () => ({
  // Every argument passes through, so an extra one, or a dropped token, shows in the calls.
  createEvent: (...a: [string, NewEventDraft, string | null]) => createEvent(...a),
  noteCreateResult: (...a: unknown[]) => noteCreateResult(...a),
  noteHeldCreate: (...a: unknown[]) => noteHeldCreate(...a),
}));

vi.mock("../../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => deviceZone,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { NewEventEditor } from "./NewEventEditor";

const ME = "gcal:me@x.com";
const WORK = "gcal:work@x.com";
const PERSONAL = `${ME}:me@x.com`;
const FAMILY = `${ME}:family`;
const WORK_CAL = `${WORK}:work@x.com`;

function account(id: string): CalendarAccount {
  const email = id.slice(id.indexOf(":") + 1);
  return { id, provider: "google", email, label: email, state: "ok", last_synced_at: null };
}

function calendar(source: string, remote: string, over: Partial<Calendar> = {}): Calendar {
  return {
    id: `${source}:${remote}`,
    source_id: source,
    provider: "google",
    remote_id: remote,
    name: remote,
    color: null,
    selected: true,
    is_primary: false,
    quiet: false,
    kind: null,
    access_role: "owner",
    time_zone: "Europe/London",
    default_reminders: [],
    conference_types: [],
    data_owner: null,
    ...over,
  };
}

const overview: CalendarOverview = {
  google_client_configured: true,
  microsoft_client_configured: false,
  accounts: [account(ME), account(WORK)],
  calendars: [
    calendar(ME, "me@x.com", { name: "Personal", is_primary: true }),
    calendar(ME, "family", { name: "Family", access_role: "writerWithoutPrivateAccess" }),
    calendar(WORK, "work@x.com", { name: "Work", is_primary: true }),
  ],
  last_sync: null,
  window_days: 7,
  mirror_start: "2026-09-01T00:00:00Z",
  mirror_end: "2027-11-01T00:00:00Z",
  editing: { [ME]: "on", [WORK]: "on" },
  // Work's account makes half-hour events; Personal's has none read, so an hour.
  default_length: { [WORK]: 30 },
};

const seed: EditorFields = {
  summary: "",
  location: "",
  description: "",
  time: {
    kind: "timed",
    start_date: "2026-10-12",
    start_time: "09:00",
    start_zone: "Europe/London",
    end_date: "2026-10-12",
    end_time: "10:00",
    end_zone: "Europe/London",
  },
  show_as: "busy",
  visibility: "default",
};

/** What the editor sends for the seed titled `summary`, on `calendarId`. */
const sentFor = (summary: string, calendarId = PERSONAL) => ({
  calendarId,
  draft: { ...seed, summary } as NewEventDraft,
});

const saved: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };
const ambiguous: WriteOutcome = { outcome: "ambiguous", retry_token: "ref-1" };

/** A promise whose answer the test gives when it chooses. */
function later() {
  let answer: (o: WriteOutcome) => void = () => {};
  const promise = new Promise<WriteOutcome>((resolve) => (answer = resolve));
  return { promise, answer };
}

// jsdom ships no ResizeObserver, and the calendar list's popover (escapeClipping) builds one.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  deviceZone = "Europe/London";
});

afterEach(() => {
  cleanup();
  // Reset, not clear: an answer one test queued and never used must not reach the next.
  vi.resetAllMocks();
});

function open(over: { calendarId?: string; hidden?: Set<string>; onClose?: () => void } = {}) {
  const onClose = over.onClose ?? vi.fn();
  const view = render(
    <NewEventEditor
      seed={seed}
      targets={createTargets(overview)}
      calendarId={over.calendarId ?? PERSONAL}
      overview={overview}
      hidden={over.hidden ?? new Set()}
      colorOf={() => "var(--accent)"}
      shapeOf={() => undefined}
      onClose={onClose}
    />,
  );
  return { onClose, unmount: view.unmount };
}

const title = () => screen.getByLabelText<HTMLInputElement>("Title");

function pickCalendar(from: string, to: string) {
  fireEvent.click(screen.getByRole("button", { name: `Calendar: ${from}` }));
  fireEvent.click(screen.getByRole("radio", { name: to }), { detail: 1 });
}

describe("NewEventEditor", () => {
  it("with no zone, the time controls and Save are disabled", () => {
    deviceZone = null;
    open();
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(screen.getByText(/couldn't tell which time zone this computer is in/)).toBeTruthy();
    // The rest can still be typed: only the save waits for a zone.
    expect(title().disabled).toBe(false);
  });

  it("Save creates and closes on saved, remembering the calendar", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    // The calendar, every field of the draft, and no token: nobody to notify, no repeat, no guests.
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sentFor("Dentist").draft, null]]);
    expect(Object.keys(createEvent.mock.calls[0][1]).sort()).toEqual([
      "description",
      "location",
      "show_as",
      "summary",
      "time",
      "visibility",
    ]);
    // The store says Created and remembers the calendar (useEventWrites' own tests).
    expect(noteCreateResult).toHaveBeenCalledWith(sentFor("Dentist"), null, saved, {
      name: "Personal",
      hidden: false,
    });
  });

  it("a failed create keeps the draft open", async () => {
    createEvent.mockResolvedValue({ outcome: "failed", message: "PM couldn't reach Google." });
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google."),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(title().value).toBe("Dentist");
    expect(title().disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();
    expect(noteCreateResult).not.toHaveBeenCalled();
  });

  it("sends nothing more and closes nothing while a save is out", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const saving = screen.getByRole<HTMLButtonElement>("button", { name: "Saving…" });
    expect(saving.disabled).toBe(true);
    // Every field, the calendar and Cancel are off.
    expect(title().disabled).toBe(true);
    expect(screen.getByLabelText<HTMLInputElement>("Location").disabled).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Calendar: Personal" }).disabled,
    ).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);
    // Every other way to save goes through the same guard: Enter in the title, Ctrl+S, Save.
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true });
    fireEvent.click(saving);
    // Escape doesn't close it, and asks nothing.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(createEvent).toHaveBeenCalledTimes(1);
    await act(async () => answer.answer(saved));
    expect(onClose).toHaveBeenCalled();
  });

  it("Ctrl+S, ⌘S and Enter in the title save; Shift+Enter and Enter elsewhere don't", async () => {
    createEvent.mockResolvedValue(saved);
    for (const press of [
      () => fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true }),
      () => fireEvent.keyDown(window, { key: "s", code: "KeyS", metaKey: true }),
      () => fireEvent.keyDown(title(), { key: "Enter" }),
    ]) {
      const { onClose } = open();
      fireEvent.change(title(), { target: { value: "Dentist" } });
      press();
      await waitFor(() => expect(onClose).toHaveBeenCalled());
      cleanup();
    }
    expect(createEvent).toHaveBeenCalledTimes(3);
    createEvent.mockClear();
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter", shiftKey: true });
    fireEvent.keyDown(screen.getByLabelText("Location"), { key: "Enter" });
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true, shiftKey: true });
    fireEvent.keyDown(window, { key: "s", code: "KeyS" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("ambiguous locks the fields and Check again sends the same token", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(/^PM lost touch with Google while creating “Dentist”/),
    ).toBeTruthy();
    // Shown exactly as sent, and nothing can change it.
    expect(title().value).toBe("Dentist");
    expect(title().disabled).toBe(true);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Calendar: Personal" }).disabled,
    ).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, sentFor("Dentist").draft, null],
      [PERSONAL, sentFor("Dentist").draft, "ref-1"],
    ]);
    expect(noteCreateResult).toHaveBeenCalledWith(sentFor("Dentist"), "ref-1", saved, {
      name: "Personal",
      hidden: false,
    });
  });

  it("keeps the token through sign-in and busy answers, and lets it go on gone or lost", async () => {
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockResolvedValueOnce({ outcome: "reauth" })
      .mockResolvedValueOnce({ outcome: "busy" })
      .mockResolvedValueOnce({ outcome: "gone" });
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    expect(await screen.findByText(/^“Dentist”: Google needs you to sign in again/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(await screen.findByText(/^“Dentist”: Google is busy right now/)).toBeTruthy();
    expect(title().disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(
      await screen.findByText("PM made “Dentist”, but it has since been deleted in Google."),
    ).toBeTruthy();
    // Unlocked, and the next Save makes it anew.
    expect(title().disabled).toBe(false);
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();
    expect(createEvent.mock.calls.map((c) => c[2])).toEqual([null, "ref-1", "ref-1", "ref-1"]);

    cleanup();
    createEvent.mockReset();
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockResolvedValueOnce({ outcome: "failed", message: CREATE_LOST });
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    expect(await screen.findByText(/can't tell whether “Dentist” was made/)).toBeTruthy();
    expect(title().disabled).toBe(false);
  });

  it("closing while a token is held leaves Check again behind, asking nothing", async () => {
    createEvent.mockResolvedValue(ambiguous);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("button", { name: "Check again" });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    expect(onClose).toHaveBeenCalled();
    expect(noteHeldCreate).not.toHaveBeenCalled();
    unmount();
    expect(noteHeldCreate).toHaveBeenCalledWith(sentFor("Dentist"), "ref-1");
  });

  it("an answer that arrives once the editor has gone is said as a notice", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // Gone with its tab while the save is out.
    unmount();
    expect(noteHeldCreate).not.toHaveBeenCalled();
    await act(async () => answer.answer(ambiguous));
    expect(noteCreateResult).toHaveBeenCalledWith(sentFor("Dentist"), null, ambiguous, {
      name: "Personal",
      hidden: false,
    });
  });

  it("the notice shows for a start outside the mirror and for a hidden calendar", () => {
    open({ calendarId: WORK_CAL, hidden: new Set([WORK_CAL]) });
    expect(
      screen.getByText(
        "Work is hidden in your calendar list, so the new event won't show until you show that calendar again.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/outside the months PM syncs/)).toBeNull();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "12-12-2027" } });
    fireEvent.blur(start);
    expect(screen.getByText(/^This starts outside the months PM syncs \(/)).toBeTruthy();
  });

  it("discard asks only when changed from the seed", () => {
    const first = open();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(first.onClose).toHaveBeenCalled();
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    cleanup();
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByText("Discard this new event?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("changing the calendar re-seeds an untouched end with that account's length", () => {
    open();
    const end = () => screen.getByLabelText<HTMLSelectElement>("End time");
    expect(end().value).toBe("10:00");
    pickCalendar("Personal", "Work");
    expect(end().value).toBe("09:30");
    // And back again: the length follows the calendar, not the first one picked.
    pickCalendar("Work", "Personal");
    expect(end().value).toBe("10:00");
    // Once the time is the user's, another calendar leaves it alone.
    fireEvent.change(end(), { target: { value: "11:00" } });
    pickCalendar("Personal", "Work");
    expect(end().value).toBe("11:00");
  });

  it("Private is disabled on a no-private calendar", () => {
    open({ calendarId: FAMILY });
    const visibility = screen.getByLabelText<HTMLSelectElement>("Visibility");
    const privateOption = [...visibility.options].find((o) => o.value === "private");
    expect(privateOption?.disabled).toBe(true);
    expect(
      screen.getByText("This calendar's sharing doesn't let you make private events on it."),
    ).toBeTruthy();
    cleanup();
    open();
    const mine = screen.getByLabelText<HTMLSelectElement>("Visibility");
    expect([...mine.options].find((o) => o.value === "private")?.disabled).toBe(false);
    // A private draft can't move onto a calendar that can't hold it.
    fireEvent.change(mine, { target: { value: "private" } });
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    expect(screen.getByRole<HTMLInputElement>("radio", { name: /Family/ }).disabled).toBe(true);
  });
});
