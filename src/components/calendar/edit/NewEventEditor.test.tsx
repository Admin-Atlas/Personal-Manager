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
//
// Repeating (C11): the rule chosen is sent, and the Repeat button, its list arrowed through, names
// exactly the rule a save sends; the list sits inside the dialog and Escape closes only it, never
// the editor; the backend's words say where it really starts, or why it can't be saved
// (a start the clocks skip said once), and every way to save but Check again waits for them; the
// zone it repeats in is said when it isn't this computer's; one from before the months PM syncs is
// said to show inside them; a rule's own day follows a moved start; and the Custom dialog, opened
// here or handed over by quick create, closes alone on Escape and holds every save while open.

import { StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Calendar,
  CalendarAccount,
  CalendarOverview,
  GuestNotice,
  NewEventDraft,
  RepeatPreset,
  RepeatRule,
  RepeatStart,
  RepeatView,
  WriteOutcome,
} from "../../../lib/types";
import { CREATE_LOST } from "../../../lib/calendarEdit/editReasons";
import {
  COPY_CALL_NOTE,
  toNewEvent,
  type EditorFields,
} from "../../../lib/calendarEdit/eventDraft";
import { createTargets } from "../../../lib/calendarEdit/createTargets";
import type { GuestSuggestion } from "../../../lib/calendarEdit/guests";
import type { CreateAnswer } from "./useEventWrites";

const createEvent =
  vi.fn<
    (
      calendarId: string,
      draft: NewEventDraft,
      token: string | null,
      answer: CreateAnswer | null,
    ) => Promise<WriteOutcome>
  >();
const noteCreateResult = vi.fn();
const noteHeldCreate = vi.fn();
const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();
const describeRecurrence = vi.fn<(rule: RepeatRule, start: RepeatStart) => Promise<RepeatView>>();
let deviceZone: string | null = "Europe/London";

vi.mock("./useEventWrites", () => ({
  // Every argument passes through, so an extra one, or a dropped token, shows in the calls.
  createEvent: (...a: [string, NewEventDraft, string | null, CreateAnswer | null]) =>
    createEvent(...a),
  noteCreateResult: (...a: unknown[]) => noteCreateResult(...a),
  noteHeldCreate: (...a: unknown[]) => noteHeldCreate(...a),
}));

// The repeat menu's two reads, answered as the test says.
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  recurrencePresets: (day: string) => recurrencePresets(day),
  describeRecurrence: (rule: RepeatRule, start: RepeatStart) => describeRecurrence(rule, start),
}));

vi.mock("../../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => deviceZone,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { NewEventEditor, type CopyOf, type CreateHandover } from "./NewEventEditor";

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
    calendar(ME, "family", {
      name: "Family",
      access_role: "writerWithoutPrivateAccess",
      // Its default notifications, which a new event on it shows (C13).
      default_reminders: [{ method: "popup", minutes: 10 }],
    }),
    calendar(WORK, "work@x.com", { name: "Work", is_primary: true }),
  ],
  last_sync: null,
  window_days: 7,
  mirror_start: "2026-09-01T00:00:00Z",
  mirror_end: "2027-11-01T00:00:00Z",
  editing: { [ME]: "on", [WORK]: "on" },
  // Work's account makes half-hour events; Personal's has none read, so an hour.
  default_length: { [WORK]: 30 },
  auto_meet: {},
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
  recurrence: { kind: "once" },
  guests: [],
  guest_perms: { modify: false, invite_others: true, see_others: true },
  meet: false,
  reminders: { use_default: true, overrides: [] },
  colour: { kind: "calendar" },
  show_as: "busy",
  visibility: "default",
};

/** What the editor sends for the seed titled `summary`, on `calendarId`. */
const sentFor = (summary: string, calendarId = PERSONAL) => ({
  calendarId,
  // Every field the seed has: no guests, and Google's own guest permissions, unsaid (C12d); the
  // calendar's notifications and colour, as null (C13).
  draft: {
    summary,
    time: seed.time,
    location: seed.location,
    description: seed.description,
    show_as: seed.show_as,
    visibility: seed.visibility,
    recurrence: null,
    guests: [],
    reminders: null,
    colour: null,
    meet: false,
  } as NewEventDraft,
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

/** A rule from the editor's set: `pattern`, every week or month, never ending. */
const ruleOf = (pattern: RepeatRule["pattern"]): RepeatRule => ({
  pattern,
  interval: 1,
  ends: { kind: "never" },
});
const MONDAYS = ruleOf({ kind: "weekly", days: ["mon"] });
const TUESDAYS = ruleOf({ kind: "weekly", days: ["tue"] });
const MON_WED = ruleOf({ kind: "weekly", days: ["mon", "wed"] });

/** The ready-made rules for a start date, as the backend gives them (the first two are enough). */
function presetsFor(day: string): RepeatPreset[] {
  const tuesday = day === "2026-10-13";
  return [
    { label: "Daily", rule: ruleOf({ kind: "daily" }) },
    tuesday
      ? { label: "Weekly on Tuesday", rule: TUESDAYS }
      : { label: "Weekly on Monday", rule: MONDAYS },
  ];
}

/** The backend's answer for a rule that starts where asked. */
const readyAt = (start: RepeatStart, summary = "Weekly on Monday"): RepeatView => ({
  outcome: "ready",
  summary,
  starts: start.date,
  moved: false,
});

beforeEach(() => {
  deviceZone = "Europe/London";
  recurrencePresets.mockImplementation(async (day) => presetsFor(day));
  describeRecurrence.mockImplementation(async (_rule, start) => readyAt(start));
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
    suggestions?: GuestSuggestion[];
    overview?: CalendarOverview;
    /** A copy to open on (Duplicate, C18c), its fields the seed. */
    copy?: { fields: EditorFields; of: CopyOf };
  } = {},
) {
  const onClose = over.onClose ?? vi.fn();
  const shown = over.overview ?? overview;
  const editor = (
    <NewEventEditor
      seed={over.copy?.fields ?? seed}
      targets={createTargets(shown)}
      calendarId={over.calendarId ?? PERSONAL}
      overview={shown}
      hidden={over.hidden ?? new Set()}
      suggestions={over.suggestions ?? []}
      colorOf={() => "var(--accent)"}
      shapeOf={() => undefined}
      handover={over.handover}
      copyOf={over.copy?.of}
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
  meetTouched: false,
  badDate: null,
  openCustom: false,
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
    // The calendar, every field of the draft, and no token: nobody to notify, no repeat, no guests,
    // the calendar's notifications and colour (C13), and no video call asked for (C18b).
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sentFor("Dentist").draft, null, null]]);
    expect(Object.keys(createEvent.mock.calls[0][1]).sort()).toEqual([
      "colour",
      "description",
      "guests",
      "location",
      "meet",
      "recurrence",
      "reminders",
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
      [PERSONAL, sentFor("Dentist").draft, null, null],
      [PERSONAL, sentFor("Dentist").draft, "ref-1", null],
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

  // C17: an hour dragged on the grid, handed over on Work (whose account makes half-hour events),
  // keeps its hour through All day and back.
  it("a handed-over range keeps its length through All day and back", () => {
    open({ handover: handed(seed, { calendarId: WORK_CAL, timeTouched: true }) });
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByLabelText("End time")).toBeNull();
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").value).toBe("09:00");
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("10:00");
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
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sent.draft, "ref-1", null]]);
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
    // In the dialog itself, never inside its scrolling body, which would cut it off.
    expect(list.parentElement).toBe(screen.getByRole("dialog", { name: "New event" }));
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

  // --- repeating (C11) ---

  const repeat = () => screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / });
  const saveButton = () => screen.getByRole<HTMLButtonElement>("button", { name: "Save" });
  /** The rule the Repeat button names. */
  const shownRepeat = () =>
    repeat()
      .getAttribute("aria-label")
      ?.replace(/^Repeat: /, "");
  const REPEATS = { name: "Repeat" };

  /** Open the Repeat list, once the menu has its ready-made rules. */
  async function openRepeat() {
    await act(async () => {});
    fireEvent.click(repeat());
    return screen.getByRole("group", REPEATS);
  }

  /** Click the Repeat row whose words are `label`; and let the backend's words for it land. */
  async function pickRepeat(label: string) {
    const list = await openRepeat();
    fireEvent.click(within(list).getByRole("radio", { name: label }), { detail: 1 });
    await act(async () => {});
  }

  /** Click Custom… in the Repeat list. */
  async function chooseCustom() {
    const list = await openRepeat();
    fireEvent.click(within(list).getByRole("button", { name: "Custom…" }), { detail: 1 });
  }

  /** A timed event on `date`, 09:00 to 10:00 in `zone`. */
  const timedOn = (date: string, zone = "Europe/London"): EditorFields["time"] => ({
    kind: "timed",
    start_date: date,
    start_time: "09:00",
    start_zone: zone,
    end_date: date,
    end_time: "10:00",
    end_zone: zone,
  });

  const GAP =
    "The first occurrence would be on 29-03-2027, when the clocks skip that time. Pick a time outside that hour.";

  it("a repeating create sends the rule", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    // Asked at the start, with where its first occurrence ends.
    expect(describeRecurrence).toHaveBeenLastCalledWith(MONDAYS, {
      kind: "timed",
      date: "2026-10-12",
      time: "09:00",
      zone: "Europe/London",
      end: { date: "2026-10-12", time: "10:00", zone: "Europe/London" },
    });
    expect(shownRepeat()).toBe("Weekly on Monday");
    // It starts on the day typed, so nothing says it moved.
    expect(screen.queryByText(/the first day this repeat allows/)).toBeNull();
    fireEvent.click(saveButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Dentist").draft, recurrence: MONDAYS }, null, null],
    ]);
  });

  // The arrow keys move the rule and the button follows, so a save sends what the button names.
  it("arrowed through the Repeat list, Ctrl+S sends exactly the rule the button names", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    await act(async () => {});
    // Enter or Space on a button is its click, with no pointer behind it.
    repeat().focus();
    fireEvent.click(repeat(), { detail: 0 });
    const list = screen.getByRole("group", REPEATS);
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(list).getByRole("radio", { name: "Does not repeat" }),
      ),
    );
    // An arrow key: the browser moves focus and the choice, with a click no pointer made.
    for (const name of ["Daily", "Weekly on Monday"]) {
      fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
      const next = within(list).getByRole("radio", { name });
      next.focus();
      fireEvent.click(next, { detail: 0 });
      await act(async () => {});
    }
    expect(shownRepeat()).toBe("Weekly on Monday");
    expect(screen.queryByRole("group", REPEATS)).not.toBeNull();
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    await ctrlS();
    // The save closes the list.
    expect(screen.queryByRole("group", REPEATS)).toBeNull();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Standup").draft, recurrence: MONDAYS }, null, null],
    ]);
  });

  // The list sits inside the dialog, so its focus trap and aria-modal cover it; Escape there closes
  // only the list, focus back on its button.
  it("the Repeat list is inside the dialog, and Escape closes only it", async () => {
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Gym" } });
    const list = await openRepeat();
    // In the dialog itself, never inside its scrolling body, which would cut it off.
    expect(list.parentElement).toBe(screen.getByRole("dialog", { name: "New event" }));
    const picked = within(list).getByRole("radio", { name: "Does not repeat" });
    await waitFor(() => expect(document.activeElement).toBe(picked));
    fireEvent.keyDown(picked, { key: "Escape" });
    expect(screen.queryByRole("group", REPEATS)).toBeNull();
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(repeat());
  });

  it("a long rule's words never widen the editor", async () => {
    const LONG = "Weekly on Monday, Wednesday, Thursday and Friday, until 20-12-2026";
    describeRecurrence.mockImplementation(async (_rule, start) => readyAt(start, LONG));
    open({
      handover: handed({ ...seed, summary: "Gym", recurrence: { kind: "rule", rule: MON_WED } }),
    });
    await act(async () => {});
    expect(shownRepeat()).toBe(LONG);
    // The button fits its row, its words wrapping, and the When fields may be narrower than the
    // widest thing in them.
    expect(repeat().className.split(" ")).toContain("max-w-full");
    expect(within(repeat()).getByText(LONG).className).not.toMatch(/truncate|whitespace-nowrap/);
    expect(screen.getByRole("group", { name: "When" }).className.split(" ")).toContain("min-w-0");
  });

  // The backend lists and lands its occurrences inside the months PM syncs, wherever it starts.
  it("a repeat that starts before the months PM syncs says only its occurrences there show", async () => {
    const draft: EditorFields = {
      ...seed,
      summary: "Gym",
      time: timedOn("2026-07-06"),
      recurrence: { kind: "rule", rule: MONDAYS },
    };
    open({ handover: handed(draft) });
    await act(async () => {});
    expect(
      screen.getByText(
        /^This starts before the months PM syncs \(.+\), so only its occurrences within them will show here\.$/,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/won't show here/)).toBeNull();
    // Not repeating, none of it would.
    await pickRepeat("Does not repeat");
    expect(
      screen.getByText(/^This starts outside the months PM syncs \(.+\), so it'll be saved/),
    ).toBeTruthy();
  });

  // The When fields already refuse a start the clocks skip, and the backend refuses a first
  // occurrence on it in other words: said once.
  it("a start the clocks skip is refused once, not again under Repeat", async () => {
    const SKIPPED =
      "The first occurrence would be on 29-03-2026, when the clocks skip that time. Pick a time outside that hour.";
    describeRecurrence.mockImplementation(async (_rule, start) =>
      start.kind === "timed" && start.time === "01:30"
        ? { outcome: "refused", message: SKIPPED }
        : { outcome: "refused", message: "PM couldn't check this repeat. Try again." },
    );
    open();
    fireEvent.change(title(), { target: { value: "Swim" } });
    typeOver(screen.getByLabelText<HTMLInputElement>("Start date"), "29-03-2026");
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    await pickRepeat("Daily");
    expect(screen.getAllByText(/the clocks skip/)).toHaveLength(1);
    expect(screen.getByText(/^01:30 doesn't happen in Europe\/London that night/)).toBeTruthy();
    expect(saveButton().disabled).toBe(true);
    // The time fixed, what the backend says of the rule is the Repeat row's to say.
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "09:00" } });
    await act(async () => {});
    expect(screen.queryByText(/the clocks skip/)).toBeNull();
    expect(screen.getByText("PM couldn't check this repeat. Try again.")).toBeTruthy();
  });

  it("Does not repeat sends recurrence null", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    await pickRepeat("Does not repeat");
    fireEvent.click(saveButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sentFor("Dentist").draft, null, null]]);
  });

  it("Weekly Mon, Wed chosen on a Tuesday says it starts Wednesday", async () => {
    createEvent.mockResolvedValue(saved);
    describeRecurrence.mockResolvedValue({
      outcome: "ready",
      summary: "Weekly on Monday and Wednesday",
      starts: "2026-10-14",
      moved: true,
    });
    const draft: EditorFields = {
      ...seed,
      summary: "Gym",
      time: timedOn("2026-10-13"),
      recurrence: { kind: "rule", rule: MON_WED },
    };
    const { onClose } = open({ handover: handed(draft) });
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Monday and Wednesday");
    expect(
      screen.getByText(/^Starts on Wed 14-10(-2026)?, the first day this repeat allows\.$/),
    ).toBeTruthy();
    // Said, never written into the date: the save moves it, and no repeat gives back what was typed.
    expect(screen.getByLabelText<HTMLInputElement>("Start date").value).toBe("13-10-2026");
    fireEvent.click(saveButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1]).toMatchObject({
      time: { start_date: "2026-10-13" },
      recurrence: MON_WED,
    });
  });

  it("a gap start shows the words and Save stays disabled", async () => {
    describeRecurrence.mockResolvedValue({ outcome: "refused", message: GAP });
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    expect(screen.getByText(GAP)).toBeTruthy();
    expect(saveButton().disabled).toBe(true);
    // Every way to save waits on it: Enter in the title and Ctrl+S too.
    title().focus();
    fireEvent.keyDown(title(), { key: "Enter" });
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
    // Not repeating, it saves.
    createEvent.mockResolvedValue(saved);
    await pickRepeat("Does not repeat");
    expect(screen.queryByText(GAP)).toBeNull();
    expect(saveButton().disabled).toBe(false);
  });

  it("nothing saves until the backend has said what the repeat means", async () => {
    let answer: (v: RepeatView) => void = () => {};
    describeRecurrence.mockReturnValue(new Promise<RepeatView>((r) => (answer = r)));
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    expect(saveButton().disabled).toBe(true);
    await ctrlS();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    await act(async () =>
      answer({ outcome: "ready", summary: "Weekly on Monday", starts: "2026-10-12", moved: false }),
    );
    expect(saveButton().disabled).toBe(false);
    await ctrlS();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent).toHaveBeenCalledTimes(1);
  });

  // Ctrl+S commits a date typed and not yet left as it saves: the rule was never asked about at that
  // start, so the answer for the start before it says nothing.
  it("a start typed and saved at once waits for the repeat's words at that start", async () => {
    createEvent.mockResolvedValue(saved);
    let answer: (v: RepeatView) => void = () => {};
    describeRecurrence.mockImplementation((_rule, start) =>
      start.date === "2026-10-12"
        ? Promise.resolve(readyAt(start))
        : new Promise<RepeatView>((r) => (answer = r)),
    );
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    expect(saveButton().disabled).toBe(false);
    const date = screen.getByLabelText<HTMLInputElement>("Start date");
    date.focus();
    fireEvent.change(date, { target: { value: "19-10-2026" } });
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    await act(async () => answer({ outcome: "refused", message: GAP }));
    expect(screen.getByText(GAP)).toBeTruthy();
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("Repeats in New York time shows when the zones differ, not when they match", async () => {
    const draft = (zone: string, repeats = true): EditorFields => ({
      ...seed,
      time: timedOn("2026-10-12", zone),
      recurrence: repeats ? { kind: "rule", rule: MONDAYS } : { kind: "once" },
    });
    const first = open({ handover: handed(draft("America/New_York")) });
    await act(async () => {});
    expect(screen.getByText("Repeats in New York time.")).toBeTruthy();
    first.unmount();
    // The same zone as this computer's: nothing to say.
    open({ handover: handed(draft("Europe/London")) });
    await act(async () => {});
    expect(screen.queryByText(/^Repeats in /)).toBeNull();
    cleanup();
    // Nor for an event that doesn't repeat.
    open({ handover: handed(draft("America/New_York", false)) });
    await act(async () => {});
    expect(screen.queryByText(/^Repeats in /)).toBeNull();
    cleanup();
    // It's this computer's zone that counts.
    deviceZone = "America/New_York";
    open({ handover: handed(draft("Europe/London")) });
    await act(async () => {});
    expect(screen.getByText("Repeats in London time.")).toBeTruthy();
  });

  it("moving the start date moves a one-day weekly rule", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    await pickRepeat("Weekly on Monday");
    typeOver(screen.getByLabelText<HTMLInputElement>("Start date"), "13-10-2026");
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Tuesday");
    expect(describeRecurrence).toHaveBeenLastCalledWith(
      TUESDAYS,
      expect.objectContaining({ date: "2026-10-13" }),
    );
    fireEvent.click(saveButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].recurrence).toEqual(TUESDAYS);
  });

  // An end date is the user's: a start moved past it leaves it as set, and the backend's refusal
  // stops the save (C11 critiques C1-M5, C2-M3).
  it("a start moved past the repeat's end date is refused, the end left as set", async () => {
    const ENDS = "The end date is before the first occurrence.";
    describeRecurrence.mockImplementation(async (rule, start) =>
      rule.ends.kind === "on_date" && start.date > rule.ends.date
        ? { outcome: "refused", message: ENDS }
        : readyAt(start),
    );
    const until: RepeatRule = { ...MONDAYS, ends: { kind: "on_date", date: "2026-10-19" } };
    open({
      handover: handed({ ...seed, summary: "Gym", recurrence: { kind: "rule", rule: until } }),
    });
    await act(async () => {});
    expect(saveButton().disabled).toBe(false);
    typeOver(screen.getByLabelText<HTMLInputElement>("Start date"), "27-10-2026");
    await act(async () => {});
    expect(describeRecurrence).toHaveBeenLastCalledWith(
      { ...TUESDAYS, ends: { kind: "on_date", date: "2026-10-19" } },
      expect.objectContaining({ date: "2026-10-27" }),
    );
    expect(screen.getByText(ENDS)).toBeTruthy();
    expect(saveButton().disabled).toBe(true);
  });

  it("Custom… sets the rule: Done shows it in the backend's words, focus back on the menu", async () => {
    createEvent.mockResolvedValue(saved);
    describeRecurrence.mockImplementation(async (rule, start) =>
      readyAt(
        start,
        rule.pattern.kind === "weekly" && rule.pattern.days.length === 2
          ? "Weekly on Monday and Wednesday"
          : "Weekly on Monday",
      ),
    );
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Gym" } });
    await chooseCustom();
    const custom = screen.getByRole("dialog", { name: "Custom recurrence" });
    // Weekly on the start's own day to begin with, as Google's dialog opens.
    expect(
      within(custom).getByRole("button", { name: "Monday" }).getAttribute("aria-pressed"),
    ).toBe("true");
    fireEvent.click(within(custom).getByRole("button", { name: "Wednesday" }));
    fireEvent.click(within(custom).getByRole("button", { name: "Done" }));
    await act(async () => {});
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    expect(shownRepeat()).toBe("Weekly on Monday and Wednesday");
    expect(document.activeElement).toBe(repeat());
    fireEvent.click(saveButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].recurrence).toEqual(MON_WED);
  });

  it("Escape in the custom dialog leaves the editor open", async () => {
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Gym" } });
    await chooseCustom();
    expect(screen.getByRole("dialog", { name: "Custom recurrence" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    // The editor stays, asking nothing, its repeat as it was, and focus back on the menu.
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
    expect(screen.queryByText("Discard this new event?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(shownRepeat()).toBe("Does not repeat");
    expect(document.activeElement).toBe(repeat());
  });

  it("nothing saves from behind the custom dialog", async () => {
    createEvent.mockResolvedValue(saved);
    open();
    fireEvent.change(title(), { target: { value: "Gym" } });
    await chooseCustom();
    await ctrlS();
    expect(createEvent).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Custom recurrence" })).toBeTruthy();
  });

  it("quick create's Custom… opens the dialog here; Cancel puts focus on the title", async () => {
    open({ handover: handed({ ...seed, summary: "Gym" }, { openCustom: true }) });
    const custom = screen.getByRole("dialog", { name: "Custom recurrence" });
    expect(document.activeElement).toBe(within(custom).getByLabelText("Repeat every"));
    fireEvent.click(within(custom).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(title()));
    expect(createEvent).not.toHaveBeenCalled();
  });

  // Check again sends exactly what was sent: what the Repeat row says of the fields now can't hold it.
  it("a handed-over retry token's Check again doesn't wait for the repeat's words", async () => {
    describeRecurrence.mockReturnValue(new Promise<RepeatView>(() => {}));
    createEvent.mockResolvedValue(saved);
    const draft: EditorFields = {
      ...seed,
      summary: "Gym",
      recurrence: { kind: "rule", rule: MONDAYS },
    };
    const sent = { calendarId: PERSONAL, draft: toNewEvent(draft) };
    expect(sent.draft.recurrence).toEqual(MONDAYS);
    const first = open({ handover: handed(draft, { held: { token: "ref-1", sent } }) });
    await act(async () => {});
    const again = screen.getByRole<HTMLButtonElement>("button", { name: "Check again" });
    expect(again.disabled).toBe(false);
    // With the words still to come, it sends the held body, rule and all, under the same token.
    fireEvent.click(again);
    await waitFor(() => expect(first.onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sent.draft, "ref-1", null]]);
    cleanup();
    // Ctrl+S too.
    createEvent.mockClear();
    const second = open({ handover: handed(draft, { held: { token: "ref-1", sent } }) });
    await act(async () => {});
    await ctrlS();
    await waitFor(() => expect(second.onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sent.draft, "ref-1", null]]);
  });
});

// --- guests (#884, C12d) ---

/** One guest new to the event, invited: what the question shows for one. */
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

/** The dentist inviting b@example.org, as a create sends it. */
const withGuest = (): NewEventDraft => ({
  ...sentFor("Dentist").draft,
  guests: [{ email: "b@example.org", optional: false }],
});

const question = () => screen.queryByRole("dialog", { name: "Send invitation emails to guests?" });

/** Type `email` into the guest box and add it. */
function addGuest(email: string) {
  const box = screen.getByRole("combobox", { name: "Add guests" });
  fireEvent.change(box, { target: { value: email } });
  fireEvent.keyDown(box, { key: "Enter" });
}

/** Title a new event "Dentist", invite b@example.org, and Save: the question that opens. */
async function inviteAndSave() {
  const opened = open();
  fireEvent.change(title(), { target: { value: "Dentist" } });
  addGuest("b@example.org");
  const save = screen.getByRole("button", { name: "Save" });
  fireEvent.click(save);
  const prompt = await screen.findByRole("dialog", { name: "Send invitation emails to guests?" });
  return { ...opened, save, prompt };
}

describe("NewEventEditor guests (C12d)", () => {
  it("a new event with a guest asks before creating; Back keeps the draft; Send creates with all", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose, save, prompt } = await inviteAndSave();
    const p = within(prompt);
    expect(p.getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    expect(p.getByText("Google sends the emails from me@x.com.")).toBeTruthy();
    // No default: focus starts on Back, and nothing is sent before an answer.
    expect(document.activeElement).toBe(p.getByRole("button", { name: "Back" }));
    expect(createEvent).not.toHaveBeenCalled();
    fireEvent.click(p.getByRole("button", { name: "Back" }));
    expect(question()).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(save));
    expect(title().value).toBe("Dentist");
    const rows = within(screen.getByRole("list", { name: "Guests" })).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(createEvent).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    // Asked again, and Send: created with all, for the numbers the question showed.
    fireEvent.click(save);
    const again = within(
      await screen.findByRole("dialog", { name: "Send invitation emails to guests?" }),
    );
    fireEvent.click(again.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(again.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const answer = { notify: "all", asked: ONE_INVITED };
    expect(createEvent.mock.calls).toEqual([[PERSONAL, withGuest(), null, answer]]);
    expect(noteCreateResult).toHaveBeenCalledWith(
      { calendarId: PERSONAL, draft: withGuest(), answer },
      null,
      saved,
      { name: "Personal", hidden: false },
    );
  });

  // Nothing goes from behind the question (C12 critique C2-L3): the editor's own Save, Ctrl+S,
  // Enter in the title and Cancel do nothing while it's open.
  it("Create stays disabled until answered, and nothing goes from behind the question", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose, save, prompt } = await inviteAndSave();
    const p = within(prompt);
    const create = p.getByRole<HTMLButtonElement>("button", { name: "Create" });
    expect(create.disabled).toBe(true);
    expect(p.getAllByRole<HTMLInputElement>("radio").some((r) => r.checked)).toBe(false);
    fireEvent.click(create);
    fireEvent.click(save);
    await ctrlS();
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await act(async () => {});
    expect(
      screen.getAllByRole("dialog", { name: "Send invitation emails to guests?" }),
    ).toHaveLength(1);
    expect(screen.queryByRole("dialog", { name: "Discard this new event?" })).toBeNull();
    expect(createEvent).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(p.getByRole("radio", { name: /^Don't send/ }));
    expect(create.disabled).toBe(false);
    fireEvent.click(create);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, withGuest(), null, { notify: "none", asked: ONE_INVITED }],
    ]);
  });

  it("Escape closes only the email question", async () => {
    const { onClose, prompt } = await inviteAndSave();
    fireEvent.keyDown(prompt, { key: "Escape" });
    expect(question()).toBeNull();
    expect(screen.queryByRole("dialog", { name: "Discard this new event?" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("a new event without guests never asks", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sentFor("Dentist").draft, null, null]]);
  });

  // The organiser of a new event is its calendar (yours, on your own calendar), and a room is no
  // one Google emails.
  it("inviting only a room never asks, and your own address isn't added", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    addGuest("me@x.com");
    expect(screen.getByText("That's the organiser, who is on it already.")).toBeTruthy();
    addGuest("room1@resource.calendar.google.com");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    const room = { email: "room1@resource.calendar.google.com", optional: false };
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Dentist").draft, guests: [room] }, null, null],
    ]);
  });

  // C13 review round 1: on a calendar that isn't your primary one, your own address is a guest,
  // counted as the backend counts it: as the account's own, never one Google invites.
  it("on another of your calendars, your own address goes but is never counted", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ calendarId: FAMILY });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    addGuest("me@x.com");
    addGuest("b@example.org");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const p = within(
      await screen.findByRole("dialog", { name: "Send invitation emails to guests?" }),
    );
    expect(p.getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    fireEvent.click(p.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(p.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const guests = [
      { email: "me@x.com", optional: false },
      { email: "b@example.org", optional: false },
    ];
    expect(createEvent.mock.calls).toEqual([
      [
        FAMILY,
        { ...sentFor("Dentist", FAMILY).draft, guests },
        null,
        { notify: "all", asked: ONE_INVITED },
      ],
    ]);
    cleanup();
    createEvent.mockClear();
    // Only you: nobody to invite, so nothing is asked.
    const alone = open({ calendarId: FAMILY });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    addGuest("me@x.com");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(alone.onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    expect(createEvent.mock.calls).toEqual([
      [
        FAMILY,
        { ...sentFor("Dentist", FAMILY).draft, guests: [{ email: "me@x.com", optional: false }] },
        null,
        null,
      ],
    ]);
  });

  // C13 review round 1: the calendar picked organises the event, so its address typed as a guest
  // on another one comes off the list, saying so: never a row stuck as "Organiser", sent, and
  // counted where the backend doesn't count it.
  it("picking the calendar a guest's address names takes that guest off, and says so", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    addGuest("work@x.com");
    addGuest("b@example.org");
    pickCalendar("Personal", "Work");
    expect(
      screen.getByText("work@x.com organises this event now, so it came off the guest list."),
    ).toBeTruthy();
    const rows = within(screen.getByRole("list", { name: "Guests" })).getAllByRole("listitem");
    expect(rows).toHaveLength(1);
    expect(screen.queryByText("Organiser")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const p = within(
      await screen.findByRole("dialog", { name: "Send invitation emails to guests?" }),
    );
    expect(p.getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    fireEvent.click(p.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(p.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent).toHaveBeenCalledTimes(1);
    const [calendarId, draft, , answer] = createEvent.mock.calls[0];
    expect(calendarId).toBe(WORK_CAL);
    expect(draft.guests).toEqual([{ email: "b@example.org", optional: false }]);
    expect(answer).toEqual({ notify: "all", asked: ONE_INVITED });
  });

  it("the backend's numbers ask again, and the answer goes for them", async () => {
    const two: GuestNotice = { ...ONE_INVITED, invited: 2, people: 2, emails: 2 };
    createEvent
      .mockResolvedValueOnce({ outcome: "needs_choice", scope: false, guests: two, again: true })
      .mockResolvedValueOnce(saved);
    const { onClose, prompt } = await inviteAndSave();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));
    const line = await screen.findByText("2 guests new to it will get an invitation.");
    const again = within(line.closest('[role="dialog"]') as HTMLElement);
    expect(
      again.getByText("PM checked with Google before saving, and these are the numbers it found."),
    ).toBeTruthy();
    // Asked afresh: nothing picked, nothing sent meanwhile, the editor still open.
    expect(again.getAllByRole<HTMLInputElement>("radio").some((r) => r.checked)).toBe(false);
    expect(createEvent).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    // A save from behind it (Ctrl+S, the editor's Save) never swaps Google's numbers for the
    // editor's own count.
    await ctrlS();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(again.getByText("2 guests new to it will get an invitation.")).toBeTruthy();
    expect(screen.queryByText("1 guest new to it will get an invitation.")).toBeNull();
    fireEvent.click(again.getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(again.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, withGuest(), null, { notify: "all", asked: ONE_INVITED }],
      [PERSONAL, withGuest(), null, { notify: "none", asked: two }],
    ]);
  });

  it("a second ask in one Save is said rather than asked, and nothing is held", async () => {
    createEvent.mockResolvedValue({
      outcome: "needs_choice",
      scope: false,
      guests: ONE_INVITED,
      again: true,
    });
    const { onClose, save, prompt } = await inviteAndSave();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));
    const reasked = await waitFor(() => {
      const open = question();
      expect(open).not.toBeNull();
      return within(open as HTMLElement);
    });
    fireEvent.click(reasked.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(reasked.getByRole("button", { name: "Create" }));
    expect(
      await screen.findByText(
        "“Dentist” wasn't created: PM's count of who Google would invite differs from the one you answered. Save it again to choose whether Google sends the invitations.",
      ),
    ).toBeTruthy();
    expect(question()).toBeNull();
    expect(createEvent).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("button", { name: "Check again" })).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(save));
    expect(onClose).not.toHaveBeenCalled();
    // A new Save starts its count afresh: the backend's ask opens the question again.
    fireEvent.click(save);
    const own = within(await screen.findByRole("dialog", { name: /^Send invitation/ }));
    fireEvent.click(own.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(own.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createEvent).toHaveBeenCalledTimes(3));
    expect(await screen.findByText(/PM checked with Google before saving/)).toBeTruthy();
  });

  // Check again sends the answer first sent; were the backend ever to ask, a fresh answer would go
  // without the token, as a second new event, so it's said instead and the token kept.
  it("Check again never asks the email question", async () => {
    createEvent.mockResolvedValue({
      outcome: "needs_choice",
      scope: false,
      guests: ONE_INVITED,
    });
    const draft: EditorFields = {
      ...seed,
      summary: "Dentist",
      guests: [
        {
          email: "b@example.org",
          name: null,
          response: null,
          optional: false,
          organizer: false,
          self: false,
          room: false,
        },
      ],
    };
    const sent = {
      calendarId: PERSONAL,
      draft: toNewEvent(draft),
      answer: { notify: "all" as const, asked: ONE_INVITED },
    };
    open({ handover: handed(draft, { held: { token: "ref-1", sent } }) });
    await act(async () => {});
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(createEvent).toHaveBeenCalledTimes(1));
    await act(async () => {});
    expect(question()).toBeNull();
    expect(createEvent.mock.calls).toEqual([[PERSONAL, withGuest(), "ref-1", sent.answer]]);
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
  });

  // The held create keeps its answer (C12 §9.3): Check again sends it with the draft, under the
  // token, and asks nothing; closing leaves it with the notice's Check again.
  it("an answer that never arrived keeps the answer for Check again, here and as a notice", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(saved);
    const { onClose, prompt } = await inviteAndSave();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));
    const check = await screen.findByRole("button", { name: "Check again" });
    // Locked on what was sent, the guest box too.
    expect(screen.getByRole<HTMLInputElement>("combobox", { name: "Add guests" }).disabled).toBe(
      true,
    );
    fireEvent.click(check);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    const answer = { notify: "all", asked: ONE_INVITED };
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, withGuest(), null, answer],
      [PERSONAL, withGuest(), "ref-1", answer],
    ]);
    cleanup();
    // Closed while held: the notice keeps exactly what was sent, its answer included.
    createEvent.mockReset();
    createEvent.mockResolvedValue(ambiguous);
    const second = await inviteAndSave();
    fireEvent.click(within(second.prompt).getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(within(second.prompt).getByRole("button", { name: "Create" }));
    await screen.findByRole("button", { name: "Check again" });
    // The footer's Close or the title bar's: both close without asking while one is held.
    fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0]);
    expect(second.onClose).toHaveBeenCalled();
    await leave(second.unmount);
    expect(noteHeldCreate).toHaveBeenCalledWith(
      {
        calendarId: PERSONAL,
        draft: withGuest(),
        answer: { notify: "none", asked: ONE_INVITED },
      },
      "ref-1",
    );
  });

  it("text in the guest box that isn't an address stops the save, and closing asks first", async () => {
    // Nothing else typed: the draft is as it opened, so only the box's text stops either.
    const { onClose } = open();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "bob at example" } });
    fireEvent.blur(box);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(question()).toBeNull();
    expect(screen.getAllByText("“bob at example” isn't an email address.").length).toBeGreaterThan(
      0,
    );
    // It never reached the draft, so closing would lose it: asked first.
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("dialog", { name: "Discard this new event?" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });
});

// Notifications and colour (#884, C13): left as the calendar's they go as null; a list or a colour
// goes as chosen; Ctrl+S sends a number typed but not committed, and a row it can't commit holds
// every way to save; another calendar keeps a list the user made; the kind switch says so.
describe("NewEventEditor, notifications and colour", () => {
  const count = (words: string) =>
    screen.getByRole<HTMLInputElement>("textbox", { name: `How many: ${words}` });

  it("sends the list and colour chosen", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    expect(screen.getByText("Personal has no default notifications.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    // Focus goes to the new row's number (C13 critique C2-M6).
    await waitFor(() => expect(document.activeElement).toBe(count("30 minutes before")));
    fireEvent.click(screen.getByRole("button", { name: "Colour: Calendar colour" }));
    fireEvent.click(await screen.findByRole("option", { name: "Tomato" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [
        PERSONAL,
        {
          ...sentFor("Dentist").draft,
          reminders: { use_default: false, overrides: [{ method: "popup", minutes: 30 }] },
          colour: { kind: "colour", id: "11" },
        },
        null,
        null,
      ],
    ]);
  });

  it("Ctrl+S with an uncommitted row saves the typed value (C13 critique C2-M5)", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    const row = count("30 minutes before");
    row.focus();
    fireEvent.change(row, { target: { value: "45" } });
    await ctrlS();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].reminders).toEqual({
      use_default: false,
      overrides: [{ method: "popup", minutes: 45 }],
    });
  });

  it("a row it can't commit holds Save, Ctrl+S and Enter, and closing asks first", async () => {
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    const unit = screen.getByRole("combobox", { name: "Unit: 30 minutes before" });
    // 30 weeks: past Google's four.
    fireEvent.change(unit, { target: { value: "weeks" } });
    expect(screen.getByText("Google allows notifications up to 4 weeks before.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await ctrlS();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("dialog", { name: "Discard this new event?" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("text a default row can't take asks before closing, though the draft never changed", async () => {
    const { onClose } = open({ calendarId: FAMILY });
    const row = count("10 minutes before");
    row.focus();
    fireEvent.change(row, { target: { value: "ten" } });
    fireEvent.blur(row);
    expect(screen.getByText("Use a whole number.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByRole("dialog", { name: "Discard this new event?" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("another calendar keeps the list made, and shows its own defaults otherwise", async () => {
    open();
    pickCalendar("Personal", "Family");
    expect(screen.getByText("Family's default notifications:")).toBeTruthy();
    expect(count("10 minutes before")).toBeTruthy();
    pickCalendar("Family", "Personal");
    expect(screen.getByText("Personal has no default notifications.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    pickCalendar("Personal", "Family");
    expect(screen.queryByText("Family's default notifications:")).toBeNull();
    expect(count("30 minutes before")).toBeTruthy();
  });

  it("switching to all day puts a list made back to the calendar's defaults, and says so", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to Personal's defaults for all-day events."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].reminders).toBeNull();
  });

  // C13 review round 1: switching back over a list made on the other kind says so too, and the
  // line names the calendar picked since, whose defaults the event is on.
  it("switching back over a list made says so, naming the calendar picked", async () => {
    open();
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(count("1 day before at 09:00")).toBeTruthy();
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to Personal's defaults for timed events."),
    ).toBeTruthy();
    pickCalendar("Personal", "Family");
    expect(
      screen.getByText("Notifications went back to Family's defaults for timed events."),
    ).toBeTruthy();
    expect(screen.queryByText(/Personal's defaults/)).toBeNull();
  });
});

// --- a video call on new events (C18b) ---

/** Personal and Work offer Google Meet, Family doesn't; Personal's account adds Meet to the events
 *  it creates (`auto`), Work's doesn't. */
const meetOverview = (auto: Record<string, boolean> = { [ME]: true }): CalendarOverview => ({
  ...overview,
  calendars: overview.calendars.map((c) =>
    c.id === FAMILY ? c : { ...c, conference_types: ["hangoutsMeet"] },
  ),
  auto_meet: auto,
});

const videoCall = () => screen.getByRole("group", { name: "Video call" });
const addMeet = () =>
  within(videoCall()).getByRole("button", { name: "Add Google Meet video conferencing" });
const dontAdd = () => within(videoCall()).getByRole("button", { name: "Don't add the video call" });
const meetAsked = () =>
  within(videoCall()).queryByText(
    "Google Meet video conferencing. Google makes the link when you save.",
  ) !== null;

describe("NewEventEditor video call (C18b)", () => {
  // Critique C2-H2: Google adds Meet "when you add invitees", so the setting turns the row on with
  // the first guest, and off again when none is left, while it's untouched.
  it("the account's setting turns the row on with the first guest", () => {
    open({ overview: meetOverview() });
    expect(meetAsked()).toBe(false);
    expect(addMeet()).toBeTruthy();
    addGuest("b@example.org");
    expect(meetAsked()).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Remove b@example.org" }));
    expect(meetAsked()).toBe(false);
    // The user's own address invites nobody, so no call.
    addGuest("me@x.com");
    expect(meetAsked()).toBe(false);
    cleanup();
    // Off in Google: a guest brings no call.
    open({ overview: meetOverview({ [ME]: false }) });
    addGuest("b@example.org");
    expect(meetAsked()).toBe(false);
  });

  it("Save asks for the call", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ overview: meetOverview() });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(addMeet());
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Dentist").draft, meet: true }, null, null],
    ]);
  });

  it("the row is off while what was sent may be in Google", async () => {
    createEvent.mockResolvedValue(ambiguous);
    open({ overview: meetOverview() });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(addMeet());
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("button", { name: "Check again" });
    expect(dontAdd()).toHaveProperty("disabled", true);
  });

  it("a calendar change re-seeds an untouched row", () => {
    open({ overview: meetOverview({ [ME]: true, [WORK]: false }) });
    addGuest("b@example.org");
    expect(meetAsked()).toBe(true);
    pickCalendar("Personal", "Work");
    expect(meetAsked(), "Work's account adds none").toBe(false);
    pickCalendar("Work", "Personal");
    expect(meetAsked()).toBe(true);
  });

  it("a touched row stays, except on a calendar without Meet, which says so", () => {
    open({ overview: meetOverview({ [ME]: false, [WORK]: false }) });
    fireEvent.click(addMeet());
    pickCalendar("Personal", "Work");
    expect(meetAsked(), "touched: kept where Meet is offered").toBe(true);
    pickCalendar("Work", "Family");
    expect(meetAsked()).toBe(false);
    expect(
      within(videoCall()).getByText("Family doesn't offer Google Meet, so the video call is off."),
    ).toBeTruthy();
    expect(within(videoCall()).queryByRole("button")).toBeNull();
    pickCalendar("Family", "Personal");
    expect(screen.queryByText(/doesn't offer Google Meet/)).toBeNull();
    expect(meetAsked(), "the call isn't brought back").toBe(false);
    // Turned off by hand, a guest doesn't bring it back either.
    cleanup();
    open({ overview: meetOverview() });
    addGuest("b@example.org");
    fireEvent.click(dontAdd());
    addGuest("c@example.org");
    expect(meetAsked()).toBe(false);
  });

  it("the hand-over carries the call, and whether it was pressed", () => {
    // Pressed in quick create: no guest needed, and a guest added here doesn't change it.
    open({
      overview: meetOverview({ [ME]: false }),
      handover: handed({ ...seed, summary: "Dentist", meet: true }, { meetTouched: true }),
    });
    expect(meetAsked()).toBe(true);
    addGuest("b@example.org");
    expect(meetAsked()).toBe(true);
    cleanup();
    // Untouched there: it follows the guests here.
    open({
      overview: meetOverview(),
      handover: handed({ ...seed, summary: "Dentist" }),
    });
    expect(meetAsked()).toBe(false);
    addGuest("b@example.org");
    expect(meetAsked()).toBe(true);
  });
});

// --- a copy of an event (Duplicate, C18c) ---

/** A copy of "Retro" with `fields` over the seed's, and what it says. */
const copyOf = (
  fields: Partial<EditorFields> = {},
  of: Partial<CopyOf> = {},
): { fields: EditorFields; of: CopyOf } => ({
  fields: { ...seed, summary: "Retro", location: "Room 2", ...fields },
  of: { title: "Retro", notes: [], callNote: null, ...of },
});

const guestOf = (email: string): EditorFields["guests"][number] => ({
  email,
  name: null,
  response: null,
  optional: false,
  organizer: false,
  self: false,
  room: false,
});

describe("NewEventEditor opened on a copy (C18c)", () => {
  it("a copy says what it copies and what it doesn't", () => {
    open({
      copy: copyOf(
        {},
        {
          notes: [
            "The original repeats (Weekly on Monday); the copy doesn't. Choose Repeat to set it.",
          ],
        },
      ),
    });
    const dialog = screen.getByRole("dialog", { name: "New event" });
    expect(within(dialog).getByText("A copy of “Retro”.")).toBeTruthy();
    expect(
      within(dialog).getByText(
        "The original repeats (Weekly on Monday); the copy doesn't. Choose Repeat to set it.",
      ),
    ).toBeTruthy();
    expect(title().value).toBe("Retro");
  });

  // Google's Duplicate makes no video call whatever the setting (Workspace admin help 9898950).
  it("a copy's call starts off even when the setting is on, and says why", () => {
    open({
      overview: meetOverview(),
      copy: copyOf({ guests: [guestOf("b@example.org")] }, { callNote: COPY_CALL_NOTE }),
    });
    expect(meetAsked()).toBe(false);
    expect(within(videoCall()).getByText(COPY_CALL_NOTE)).toBeTruthy();
    addGuest("c@example.org");
    pickCalendar("Personal", "Work");
    pickCalendar("Work", "Personal");
    expect(meetAsked(), "neither a guest nor a calendar turns it on").toBe(false);
    fireEvent.click(addMeet());
    expect(meetAsked()).toBe(true);
    expect(within(videoCall()).queryByText(COPY_CALL_NOTE)).toBeNull();
  });

  it("closing an untouched copy asks nothing and sends nothing", () => {
    const { onClose } = open({ copy: copyOf({ guests: [guestOf("b@example.org")] }) });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "Discard this new event?" })).toBeNull();
    expect(onClose).toHaveBeenCalled();
    expect(createEvent).not.toHaveBeenCalled();
  });

  // C12d's question: Google invites the copy's guests only once the user says whether it emails.
  it("a copy with guests asks Send / Don't send before creating", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ copy: copyOf({ guests: [guestOf("b@example.org")] }) });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send invitation emails to guests?" });
    expect(createEvent).not.toHaveBeenCalled();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1]).toMatchObject({
      summary: "Retro",
      location: "Room 2",
      guests: [{ email: "b@example.org", optional: false }],
      meet: false,
    });
    expect(createEvent.mock.calls[0][3]).toEqual({ notify: "none", asked: ONE_INVITED });
  });

  // Q13: Duplicate is the way to another account's calendar.
  it("a copy can go to another account's calendar", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ copy: copyOf() });
    pickCalendar("Personal", "Work");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][0]).toBe(WORK_CAL);
    expect(createEvent.mock.calls[0][1].summary).toBe("Retro");
  });

  // Critique C2-L12: a formatted description goes as Google holds it. Formatting PM writes back is
  // edited in place (C19d), and an untouched copy sends it byte for byte, Google's wrapper and all.
  it("a copy's formatted description is edited in place and sent unchanged when untouched", async () => {
    createEvent.mockResolvedValue(saved);
    const html = "<html-blob><strong>Bring</strong> the form</html-blob>";
    const { onClose } = open({ copy: copyOf({ description: html }) });
    const box = description();
    expect(box.querySelector("b")?.textContent).toBe("Bring");
    expect(screen.getByRole("toolbar", { name: "Description formatting" })).toBeTruthy();
    fireEvent.blur(box);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].description).toBe(html);
  });

  // C19d: one PM can't write back shows as Google shows it, with Replace, which is the copy's own.
  it("a copy's description PM can't edit shows as Google does, with Replace", async () => {
    createEvent.mockResolvedValue(saved);
    const html = "<table><tr><td>Room</td><td>4.12</td></tr></table>";
    const { onClose } = open({ copy: copyOf({ description: html }) });
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    const view = screen.getByRole("group", { name: "Description" });
    expect(view.textContent).toBe("Room 4.12");
    expect(view.getAttribute("aria-describedby")).toBe(
      screen.getByText(/This description has formatting PM can't edit/).id,
    );
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    const ask = screen.getByRole("dialog", { name: "Replace the description with plain text?" });
    fireEvent.click(within(ask).getByRole("button", { name: "Replace" }));
    const box = description();
    expect(document.activeElement).toBe(box);
    expect(screen.queryByText(/This description has formatting PM can't edit/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].description).toBe("Room 4.12");
  });

  it("Replace isn't offered while the copy is being created", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    open({ copy: copyOf({ description: "<h2>Agenda</h2>" }) });
    expect(screen.getByRole("button", { name: "Replace with plain text…" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(createEvent).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Replace with plain text…" })).toBeNull();
    expect(createEvent.mock.calls[0][1].description).toBe("<h2>Agenda</h2>");
    await act(async () => answer.answer(saved));
  });

  it("an untouched copy with a description PM can't edit sends it as Google holds it", async () => {
    createEvent.mockResolvedValue(saved);
    const html = '<p style="color: red">Red</p>';
    const { onClose } = open({ copy: copyOf({ description: html }) });
    // "Keep the formatting" closes only the question.
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    const ask = screen.getByRole("dialog", { name: "Replace the description with plain text?" });
    fireEvent.click(within(ask).getByRole("button", { name: "Keep the formatting" }));
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
    expect(screen.getByRole("group", { name: "Description" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].description).toBe(html);
  });
});

// --- the description (C19d) ---

/** The description's box. */
const description = () => screen.getByRole("textbox", { name: "Description" });

/** What the user typed into the box, as the engine leaves it, then the input event it fires. */
function typeInto(box: HTMLElement, ...nodes: (Node | string)[]) {
  box.replaceChildren(...nodes);
  fireEvent.input(box);
}

/** An element made by hand, never by parsing. */
function h(tag: string, ...children: (Node | string)[]) {
  const made = document.createElement(tag);
  made.append(...children);
  return made;
}

describe("NewEventEditor's description (C19d)", () => {
  it("a_new_event_sends_formatting_and_plain_lines_as_written", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Plan" } });
    typeInto(description(), h("b", "Agenda"), h("ul", h("li", "one & two")));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].description).toBe(
      "<b>Agenda</b><ul><li>one &amp; two</li></ul>",
    );

    cleanup();
    createEvent.mockResolvedValue(saved);
    const second = open();
    fireEvent.change(title(), { target: { value: "Plan" } });
    // WebKit's lines: a div per Enter.
    typeInto(description(), "first", h("div", "second"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(second.onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[1][1].description).toBe("first\nsecond");
  });

  it("an_emptied_description_is_sent_as_nothing", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ copy: copyOf({ description: "<b>Bring</b> the form" }) });
    // What an engine leaves when everything is deleted.
    typeInto(description(), h("br"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(createEvent.mock.calls[0][1].description).toBe("");
  });

  it("clicking the Description text puts the caret in the box", () => {
    open();
    fireEvent.click(screen.getByText("Description", { selector: "span" }));
    expect(document.activeElement).toBe(description());
  });

  it("the box stands still while a save is out", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    open();
    fireEvent.change(title(), { target: { value: "Plan" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(createEvent).toHaveBeenCalled());
    expect(description().getAttribute("contenteditable")).toBe("false");
    expect(description().getAttribute("aria-disabled")).toBe("true");
    for (const button of within(
      screen.getByRole("toolbar", { name: "Description formatting" }),
    ).getAllByRole("button")) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
    await act(async () => answer.answer(saved));
  });
});
