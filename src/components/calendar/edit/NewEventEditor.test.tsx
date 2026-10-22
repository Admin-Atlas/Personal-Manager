// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The full editor's create mode (#884, C10): one guard for every way to save (Save, Ctrl/Cmd+S,
// Enter in the title), never on a repeat or while composing text, nothing editable or closable while
// a save is out, an answer that never arrived shown locked as it was sent with Check again under the
// same token, a token that ended unproven never called "nothing saved", what may be in Google left
// as a notice once, when the editor really goes (under StrictMode too), a failure that keeps the
// draft, the words that warn before a save PM's calendar won't show, a date it can't read (here or
// in quick create) refusing every save until its own field is given one (that field named, marked,
// and where a save it stops puts focus), and a calendar list the keyboard can reach.

import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

import { NewEventEditor, type CreateHandover } from "./NewEventEditor";

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
  draft: { ...seed, summary, recurrence: null } as NewEventDraft,
});

const saved: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };
const ambiguous: WriteOutcome = { outcome: "ambiguous", retry_token: "ref-1" };

/** A promise whose answer (or failure) the test gives when it chooses. */
function later() {
  let answer: (o: WriteOutcome) => void = () => {};
  let fail: (e: Error) => void = () => {};
  const promise = new Promise<WriteOutcome>((resolve, reject) => {
    answer = resolve;
    fail = reject;
  });
  return { promise, answer, fail };
}

const lost: WriteOutcome = { outcome: "failed", message: CREATE_LOST };
const unreachable: WriteOutcome = { outcome: "failed", message: "PM couldn't reach Google." };
const LIST = { name: "Calendars that can take a new event" };
const CTRL_S = { key: "s", code: "KeyS", ctrlKey: true };

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

function open(
  over: {
    calendarId?: string;
    hidden?: Set<string>;
    onClose?: () => void;
    handover?: CreateHandover;
    /** Inside <StrictMode>, as the app's development build runs: every effect set up twice. */
    strict?: boolean;
  } = {},
) {
  const onClose = over.onClose ?? vi.fn();
  const editor = (
    <NewEventEditor
      seed={seed}
      targets={createTargets(overview)}
      calendarId={over.calendarId ?? PERSONAL}
      overview={overview}
      hidden={over.hidden ?? new Set()}
      colorOf={() => "var(--accent)"}
      shapeOf={() => undefined}
      handover={over.handover}
      onClose={onClose}
    />
  );
  const view = render(over.strict ? <StrictMode>{editor}</StrictMode> : editor);
  return { onClose, unmount: view.unmount };
}

/** Unmount, and let what the editor leaves as it really goes (after StrictMode's rehearsal of an
 *  unmount can no longer be the one) be said. */
async function leave(unmount: () => void) {
  unmount();
  await act(async () => {});
}

/** What quick create hands over for `draft`, with nothing held, unproven or refused. */
const handed = (draft: EditorFields, over: Partial<CreateHandover> = {}): CreateHandover => ({
  draft,
  calendarId: PERSONAL,
  held: null,
  unproven: null,
  showAsTouched: false,
  timeTouched: false,
  badDate: null,
  ...over,
});

const title = () => screen.getByLabelText<HTMLInputElement>("Title");

/** Type `text` over what `input` shows, and leave it (a change to the same text is no change). */
function typeOver(input: HTMLInputElement, text: string) {
  input.focus();
  fireEvent.change(input, { target: { value: "" } });
  fireEvent.change(input, { target: { value: text } });
  fireEvent.blur(input);
}

async function ctrlS() {
  await act(async () => void fireEvent.keyDown(window, CTRL_S));
}

function pickCalendar(from: string, to: string) {
  fireEvent.click(screen.getByRole("button", { name: `Calendar: ${from}` }));
  fireEvent.click(screen.getByRole("radio", { name: to }), { detail: 1 });
}

describe("NewEventEditor", () => {
  it("with no zone, the time controls and Save are disabled, and it says nothing can be saved", async () => {
    deviceZone = null;
    open();
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(
      screen.getByText(
        "PM couldn't tell which time zone this computer is in, so it can't save a new event. Set the time zone in your system settings.",
      ),
    ).toBeTruthy();
    // The rest can still be typed: only the save waits for a zone.
    expect(title().disabled).toBe(false);
    // Enter in the title sends nothing, and leaves the caret where it was.
    fireEvent.change(title(), { target: { value: "Dentist" } });
    title().focus();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
  });

  it("puts focus on the title when it opens", () => {
    open();
    expect(document.activeElement).toBe(title());
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
      "recurrence",
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
    // The bar's Close too: it would do nothing, so it doesn't look or announce as if it would.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Close" }).disabled).toBe(true);
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
    await leave(unmount);
    expect(noteHeldCreate.mock.calls).toEqual([[sentFor("Dentist"), "ref-1"]]);
  });

  it("an answer that arrives once the editor has gone is said as a notice", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // Gone with its tab while the save is out.
    await leave(unmount);
    expect(noteHeldCreate).not.toHaveBeenCalled();
    await act(async () => answer.answer(ambiguous));
    expect(noteCreateResult).toHaveBeenCalledWith(sentFor("Dentist"), null, ambiguous, {
      name: "Personal",
      hidden: false,
    });
  });

  it("a first save with no answer at all once the editor has gone says it wasn't created", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await leave(unmount);
    // Turned down before it was sent (this window, the owner or the vault), and no token to keep.
    await act(async () => answer.fail(new Error("The vault is locked.")));
    expect(noteCreateResult.mock.calls).toEqual([
      [
        sentFor("Dentist"),
        null,
        { outcome: "failed", message: "The vault is locked." },
        { name: "Personal", hidden: false },
      ],
    ]);
    expect(noteHeldCreate).not.toHaveBeenCalled();
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

  it("More options from quick create keeps the title, time and calendar", () => {
    const draft: EditorFields = {
      ...seed,
      summary: "Dentist",
      time: {
        kind: "timed",
        start_date: "2026-10-12",
        start_time: "11:00",
        start_zone: "Europe/London",
        end_date: "2026-10-12",
        end_time: "11:30",
        end_zone: "Europe/London",
      },
    };
    const { onClose } = open({
      handover: handed(draft, { calendarId: WORK_CAL, timeTouched: true }),
    });
    expect(title().value).toBe("Dentist");
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").value).toBe("11:00");
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("11:30");
    expect(screen.getByRole("button", { name: "Calendar: Work" })).toBeTruthy();
    // The time was the user's there, so the calendar's length leaves it alone here too.
    pickCalendar("Work", "Personal");
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("11:30");
    // What was typed there counts as a change: closing asks first.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByText("Discard this new event?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("a retry token handed over opens locked on Check again under the same token", async () => {
    createEvent.mockResolvedValue(saved);
    const sent = sentFor("Dentist");
    const { onClose } = open({
      handover: handed({ ...seed, summary: "Dentist" }, { held: { token: "ref-1", sent } }),
    });
    expect(screen.getByText(/^PM may already have made “Dentist”/)).toBeTruthy();
    expect(title().disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sent.draft, "ref-1"]]);
  });

  // Quick create's token ended unproven before More options: its words go on here, as they would
  // have gone on there.
  it("words handed over from a token that ended unproven are said, asked with, and left on close", async () => {
    const sent = sentFor("Dentist");
    const { onClose, unmount } = open({
      handover: handed({ ...seed, summary: "Dentist" }, { unproven: { sent, outcome: lost } }),
    });
    const words =
      "PM lost touch with Google and can't tell whether “Dentist” was made. Look for it in Google Calendar before creating it again, so it isn't made twice.";
    expect(screen.getByText(words)).toBeTruthy();
    expect(title().disabled).toBe(false);
    fireEvent.keyDown(window, { key: "Escape" });
    const confirm = screen.getByRole("dialog", { name: "Discard this new event?" });
    expect(confirm.textContent).not.toContain("Nothing has been saved");
    expect(within(confirm).getByText(words)).toBeTruthy();
    fireEvent.click(within(confirm).getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([
      [sent, null, lost, { name: "Personal", hidden: false }],
    ]);
    expect(createEvent).not.toHaveBeenCalled();
  });

  // The app's development build runs every new mount's effects as setup, cleanup, setup. An editor
  // opened on quick create's token or words holds them from its first render, so only a real unmount
  // may leave them: once, and not while the editor still says them itself.
  it("under StrictMode, a handed-over token is left as a notice once, and only when it goes", async () => {
    const sent = sentFor("Dentist");
    const { unmount } = open({
      strict: true,
      handover: handed({ ...seed, summary: "Dentist" }, { held: { token: "ref-1", sent } }),
    });
    await act(async () => {});
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
    expect(noteHeldCreate).not.toHaveBeenCalled();
    expect(noteCreateResult).not.toHaveBeenCalled();
    await leave(unmount);
    expect(noteHeldCreate.mock.calls).toEqual([[sent, "ref-1"]]);
    expect(noteCreateResult).not.toHaveBeenCalled();
  });

  it("under StrictMode, handed-over words are left as a notice once, and only when it goes", async () => {
    const sent = sentFor("Dentist");
    const { unmount } = open({
      strict: true,
      handover: handed({ ...seed, summary: "Dentist" }, { unproven: { sent, outcome: lost } }),
    });
    await act(async () => {});
    expect(screen.getByText(/^PM lost touch with Google/)).toBeTruthy();
    expect(noteCreateResult).not.toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([
      [sent, null, lost, { name: "Personal", hidden: false }],
    ]);
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  // Quick create's date field refused what was typed and put its date back: More options carries the
  // refusal here, on the start's field, so the date put back is never what's saved.
  it("a date quick create couldn't read refuses every save here until the start is given one", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({
      handover: handed(
        { ...seed, summary: "Dentist" },
        { badDate: "31-02-2026", timeTouched: true },
      ),
    });
    const refused = () => screen.queryByText(/^Start date: “31-02-2026” isn't a date PM can read/);
    expect(refused()).not.toBeNull();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    expect(start.getAttribute("aria-invalid")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    // The save it stopped put focus on the field it names.
    expect(document.activeElement).toBe(start);
    // The end's date is the other half: it doesn't end the start's refusal.
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    end.focus();
    fireEvent.change(end, { target: { value: "14-10-2026" } });
    fireEvent.blur(end);
    expect(refused()).not.toBeNull();
    start.focus();
    fireEvent.change(start, { target: { value: "13-10-2026" } });
    fireEvent.blur(start);
    expect(refused()).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].time).toMatchObject({ start_date: "2026-10-13" });
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

  it("another calendar alone is a change: closing asks first", () => {
    const { onClose } = open();
    // Same account, same length: the time stays as seeded, only the calendar differs.
    pickCalendar("Personal", "Family");
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("10:00");
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByText("Discard this new event?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  // A token that ended without Google showing the event made (lost after hours held, or made and
  // since deleted): PM may have made it, so discarding never says nothing was saved, and the words
  // outlive the editor.
  it("after a token ends unproven, discarding says what PM knows and leaves it as a notice", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(lost);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    fireEvent.keyDown(window, { key: "Escape" });
    const confirm = screen.getByRole("dialog", { name: "Discard this new event?" });
    expect(confirm.textContent).not.toContain("Nothing has been saved");
    expect(
      within(confirm).getByText(
        "PM lost touch with Google and can't tell whether “Dentist” was made. Look for it in Google Calendar before creating it again, so it isn't made twice.",
      ),
    ).toBeTruthy();
    fireEvent.click(within(confirm).getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
    expect(noteCreateResult).not.toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([
      [sentFor("Dentist"), null, lost, { name: "Personal", hidden: false }],
    ]);
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  it("closing unchanged after a token ends on gone asks nothing, and still leaves the notice", async () => {
    const gone: WriteOutcome = { outcome: "gone" };
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(gone);
    const { onClose, unmount } = open();
    // The seed as it is, untitled: nothing to discard.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    expect(onClose).toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([
      [sentFor(""), null, gone, { name: "Personal", hidden: false }],
    ]);
  });

  it("made again after the look-first words, it says Created and nothing more", async () => {
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockResolvedValueOnce(lost)
      .mockResolvedValueOnce(saved);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    fireEvent.click(await screen.findByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    await leave(unmount);
    // A new create, under no token, and only its Created.
    expect(createEvent.mock.calls.map((c) => c[2])).toEqual([null, "ref-1", null]);
    expect(noteCreateResult.mock.calls).toEqual([
      [sentFor("Dentist"), null, saved, { name: "Personal", hidden: false }],
    ]);
  });

  // Check again out when the editor goes with its tab: the answer, or its absence, keeps the token.
  it("a Check again answered once the editor has gone passes its token on", async () => {
    const busy: WriteOutcome = { outcome: "busy" };
    const check = later();
    createEvent.mockResolvedValueOnce(ambiguous).mockReturnValueOnce(check.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await leave(unmount);
    // The save still out says how it ended, not the close.
    expect(noteHeldCreate).not.toHaveBeenCalled();
    await act(async () => check.answer(busy));
    expect(noteCreateResult.mock.calls).toEqual([
      [sentFor("Dentist"), "ref-1", busy, { name: "Personal", hidden: false }],
    ]);
  });

  it("a Check again with no answer at all once the editor has gone keeps Check again", async () => {
    const check = later();
    createEvent.mockResolvedValueOnce(ambiguous).mockReturnValueOnce(check.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await leave(unmount);
    await act(async () => check.fail(new Error("The app's channel closed.")));
    expect(noteHeldCreate.mock.calls).toEqual([[sentFor("Dentist"), "ref-1"]]);
    expect(noteCreateResult).not.toHaveBeenCalled();
  });

  it("a Check again with no answer at all keeps the draft locked, with Check again", async () => {
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockRejectedValueOnce(new Error("The app's channel closed."));
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    expect(await screen.findByText("The app's channel closed.")).toBeTruthy();
    expect(title().disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("an Enter or Ctrl+S that's part of composing text saves nothing", async () => {
    createEvent.mockResolvedValue(saved);
    open();
    fireEvent.change(title(), { target: { value: "歯医者" } });
    fireEvent.keyDown(title(), { key: "Enter", isComposing: true });
    // WebKit's Enter that ends a composition comes after it, marked only by its key code.
    fireEvent.keyDown(title(), { key: "Enter", keyCode: 229 });
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true, isComposing: true });
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true, keyCode: 229 });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("a held key's repeats send nothing more once an answer unlocks Save", async () => {
    createEvent.mockResolvedValue(unreachable);
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true });
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true, repeat: true });
    fireEvent.keyDown(title(), { key: "Enter", repeat: true });
    await act(async () => {});
    expect(createEvent).toHaveBeenCalledTimes(1);
    // A held Enter on Save (where the answer puts focus) would click it on each repeat: the
    // repeat's default is stopped, a first press's isn't. jsdom clicks nothing for Enter itself.
    const save = screen.getByRole("button", { name: "Save" });
    expect(fireEvent.keyDown(save, { key: "Enter", repeat: true })).toBe(false);
    expect(fireEvent.keyDown(save, { key: "Enter" })).toBe(true);
  });

  it("Ctrl+S under the discard question sends nothing and leaves focus in it", async () => {
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(window, { key: "Escape" });
    const confirm = screen.getByRole("dialog", { name: "Discard this new event?" });
    const focused = document.activeElement;
    expect(focused instanceof HTMLElement && confirm.contains(focused)).toBe(true);
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(focused);
  });

  // The line names the field, and a save it stops puts focus there: only a date typed or picked in
  // that field ends the refusal, and a blur there gives nothing.
  it("a save refused for a date it can't read puts focus on the field its line names", async () => {
    open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "99-99-2026" } });
    // Leaving the field commits it, and it's refused.
    act(() => title().focus());
    expect(
      screen.getByText(
        "Start date: “99-99-2026” isn't a date PM can read. Type the start date again as dd-mm-yyyy, or pick it from the calendar.",
      ),
    ).toBeTruthy();
    expect(document.activeElement).toBe(title());
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(start);
  });

  it("a save refused for a time the clocks skip keeps focus where it was", async () => {
    open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "29-03-2026" } });
    fireEvent.blur(start);
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    expect(screen.getByText(/01:30 doesn't happen in Europe\/London that night/)).toBeTruthy();
    act(() => title().focus());
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
  });

  // Nothing else changed, but what was typed would be lost.
  it("asks before closing when only a date it can't read stands", () => {
    const { onClose } = open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "99-99-2026" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Discard this new event?" })).toBeTruthy();
    expect(start.value).toBe("12-10-2026");
    expect(onClose).not.toHaveBeenCalled();
  });

  // The end refused, then a start that moves the end to the day wanted: what's on screen is right,
  // but the refusal stands. The line names End date, the field is marked and described by it, and a
  // save puts focus there, where typing the date it shows ends it.
  it("a refused end date is named, marked and focused by a save, after a start moved it", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    const text =
      "End date: “31-11-2026” isn't a date PM can read. Type the end date again as dd-mm-yyyy, or pick it from the calendar.";
    typeOver(end, "31-11-2026");
    expect(end.value).toBe("12-10-2026");
    typeOver(start, "30-11-2026");
    expect(end.value).toBe("30-11-2026");
    const line = screen.getByText(text);
    expect(line.getAttribute("role")).toBe("alert");
    expect(end.getAttribute("aria-invalid")).toBe("true");
    expect(end.getAttribute("aria-describedby")).toBe(line.id);
    expect(start.getAttribute("aria-invalid")).toBeNull();
    expect(start.getAttribute("aria-describedby")).toBeNull();
    // Save, pressed with focus on it.
    const save = screen.getByRole("button", { name: "Save" });
    act(() => save.focus());
    fireEvent.click(save);
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(end);
    // Leaving it from there gives nothing.
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(end);
    // The date it shows, typed there: the line and the mark go, and the event is made on it.
    typeOver(end, "30-11-2026");
    expect(screen.queryByText(text)).toBeNull();
    expect(end.getAttribute("aria-invalid")).toBeNull();
    expect(end.getAttribute("aria-describedby")).toBeNull();
    await ctrlS();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].time).toMatchObject({
      start_date: "2026-11-30",
      end_date: "2026-11-30",
    });
  });

  // The same text in both is two refusals, each named, and a save goes to the start's field.
  it("the same text refused in both date fields is said for each", async () => {
    open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    typeOver(end, "31-11-2026");
    typeOver(start, "31-11-2026");
    const startLine = screen.getByText(/^Start date: “31-11-2026” isn't a date PM can read/);
    const endLine = screen.getByText(/^End date: “31-11-2026” isn't a date PM can read/);
    expect(start.getAttribute("aria-describedby")).toBe(startLine.id);
    expect(end.getAttribute("aria-describedby")).toBe(endLine.id);
    // Each line has its own id, so each field is described by the line that names it: one id for
    // both would read the start's line in End date too.
    expect(startLine.id).not.toBe(endLine.id);
    const said = (field: HTMLElement) =>
      document.getElementById(field.getAttribute("aria-describedby") ?? "")?.textContent;
    expect(said(start)).toMatch(/^Start date:/);
    expect(said(end)).toMatch(/^End date:/);
    act(() => title().focus());
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(start);
  });

  // A refused Ctrl+S puts focus back in the field that refused, now showing the date it had. The
  // next Ctrl+S, or a press on Save, leaves that field again: the date it put back was given by
  // nobody, or the event would be made on the very date the user was changing.
  it("a date it can't read keeps refusing through Ctrl+S twice and then Save, until one is typed", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "31-11-2026" } });
    await ctrlS();
    expect(document.activeElement).toBe(start);
    expect(start.value).toBe("12-10-2026");
    await ctrlS();
    expect(document.activeElement).toBe(start);
    // A press on Save takes focus out of the field before its click.
    const save = screen.getByRole("button", { name: "Save" });
    act(() => save.focus());
    fireEvent.click(save);
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByText(/^Start date: “31-11-2026” isn't a date PM can read/)).toBeTruthy();
    expect(document.activeElement).toBe(start);
    // A date typed there ends it, and that's the date sent.
    start.focus();
    fireEvent.change(start, { target: { value: "13-10-2026" } });
    await ctrlS();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toHaveLength(1);
    expect(createEvent.mock.calls[0][1].time).toMatchObject({
      start_date: "2026-10-13",
      end_date: "2026-10-13",
    });
  });

  // Each refusal is its own field's: another field's date or refusal, a time or a Ctrl+S never ends
  // it.
  it("a refused date stands through a time and the other field's date, until its own is given one", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    const endRefused = () => screen.queryByText(/^End date: “99-99-2026” isn't a date PM can read/);
    const startRefused = () =>
      screen.queryByText(/^Start date: “31-11-2026” isn't a date PM can read/);
    typeOver(end, "99-99-2026");
    expect(endRefused()).not.toBeNull();
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "09:30" } });
    expect(endRefused()).not.toBeNull();
    // A start date moves the end with it, and the end's refusal stays.
    typeOver(start, "13-10-2026");
    expect(end.value).toBe("13-10-2026");
    expect(endRefused()).not.toBeNull();
    // Both refused, both said.
    typeOver(start, "31-11-2026");
    expect(startRefused()).not.toBeNull();
    expect(endRefused()).not.toBeNull();
    // The end's own date, typed over: the one it shows counts too. The start's still stands.
    typeOver(end, "13-10-2026");
    expect(endRefused()).toBeNull();
    expect(startRefused()).not.toBeNull();
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    typeOver(start, "13-10-2026");
    expect(startRefused()).toBeNull();
    await ctrlS();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].time).toMatchObject({
      start_date: "2026-10-13",
      start_time: "09:30",
    });
  });

  // Only a held token that ended unproven keeps the look-first words: a fresh save refused outright
  // made nothing, so discarding says so and nothing outlives the editor.
  it("after a fresh save fails outright, discarding says nothing was saved and leaves no notice", async () => {
    createEvent.mockResolvedValue(unreachable);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    fireEvent.keyDown(window, { key: "Escape" });
    const confirm = screen.getByRole("dialog", { name: "Discard this new event?" });
    expect(within(confirm).getByText("Nothing has been saved to Google yet.")).toBeTruthy();
    fireEvent.click(within(confirm).getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult).not.toHaveBeenCalled();
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  it("the calendar list closes for a save and stays closed after its answer", async () => {
    createEvent.mockResolvedValue(unreachable);
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    expect(screen.queryByRole("group", LIST)).not.toBeNull();
    fireEvent.keyDown(window, { key: "s", code: "KeyS", ctrlKey: true });
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    expect(title().disabled).toBe(false);
    expect(screen.queryByRole("group", LIST)).toBeNull();
  });

  // The list sits inside the dialog, so its focus trap and aria-modal cover it; opening it puts the
  // keyboard on the calendar picked, and Enter or Tab hands focus back to the button.
  it("the calendar list is inside the dialog, and the keyboard gets in and out", async () => {
    open();
    const trigger = screen.getByRole("button", { name: "Calendar: Personal" });
    trigger.focus();
    // Enter or Space on a button is its click, with no pointer behind it.
    fireEvent.click(trigger, { detail: 0 });
    const list = screen.getByRole("group", LIST);
    expect(screen.getByRole("dialog", { name: "New event" }).contains(list)).toBe(true);
    await waitFor(() =>
      expect(document.activeElement).toBe(within(list).getByRole("radio", { name: "Personal" })),
    );
    // An arrow key: the browser moves focus and the choice, with a click no pointer made.
    const family = within(list).getByRole("radio", { name: "Family" });
    family.focus();
    fireEvent.keyDown(family, { key: "ArrowDown" });
    fireEvent.click(family, { detail: 0 });
    expect(screen.queryByRole("group", LIST)).not.toBeNull();
    fireEvent.keyDown(family, { key: "Enter" });
    expect(screen.queryByRole("group", LIST)).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(trigger.getAttribute("aria-label")).toBe("Calendar: Family");
    // Tab leaves an open list for its button.
    fireEvent.click(trigger, { detail: 0 });
    const again = screen.getByRole("group", LIST);
    const picked = within(again).getByRole("radio", { name: "Family" });
    await waitFor(() => expect(document.activeElement).toBe(picked));
    fireEvent.keyDown(picked, { key: "Tab" });
    expect(screen.queryByRole("group", LIST)).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });
});
