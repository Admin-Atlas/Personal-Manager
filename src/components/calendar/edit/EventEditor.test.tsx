// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The event editor's promises (#884): only what changed is sent, a typed date counts even if Save is
// pressed before the field loses focus, a date it can't read is said naming its field (marked, and
// where a save it stops puts focus), a conflict never loses the draft silently, a dirty draft asks
// before it goes, and what can't change is greyed out with a reason (no device zone included).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Calendar,
  CalendarAccount,
  CalendarEvent,
  CalendarOverview,
  EditLoad,
  EventForEdit,
  EventPatchDraft,
  FieldPermissions,
  GuestForEdit,
  GuestNotice,
  Milestone,
  Notify,
  RepeatPreset,
  RepeatRule,
  RepeatStart,
  RepeatView,
  Rsvp,
  RsvpScope,
  ScopeRow,
  SeriesChoices,
  SeriesInfo,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import type { Picked } from "../../../lib/calendarEdit/saveChoices";
import type { GuestSuggestion } from "../../../lib/calendarEdit/guests";

type SaveAnswer = WriteOutcome & { newPartial?: boolean };
const openForEdit = vi.fn<(id: string) => Promise<EditLoad>>();
const saveEdit =
  vi.fn<
    (
      session: string,
      changes: EventPatchDraft,
      picked: Picked | null,
      title: string,
      notify: Notify | null,
      asked: GuestNotice | null,
    ) => Promise<SaveAnswer>
  >();
const choicesForEdit = vi.fn<(session: string, draft: EventPatchDraft) => Promise<SeriesChoices>>();
const settleSplit =
  vi.fn<(token: string, action: SplitAction, title: string) => Promise<WriteOutcome | null>>();
const noteSaved = vi.fn();
/** The row a save names, so an earlier Undo of it goes (C16d). */
const savedRow = vi.fn<(rowId: string | null | undefined) => void>();
const noteSeriesNotice = vi.fn();
const respond =
  vi.fn<
    (id: string, reply: Rsvp, note: string | null, scope: RsvpScope | null) => Promise<WriteOutcome>
  >();
const noteReply = vi.fn();
const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();
const describeRecurrence = vi.fn<(rule: RepeatRule, start: RepeatStart) => Promise<RepeatView>>();
let deviceZone: string | null = "Europe/London";

vi.mock("./useEventWrites", () => ({
  openForEdit: (id: string) => openForEdit(id),
  // Every argument passes through, so a scope sent without its token, or an email answer sent
  // without being asked, couldn't pass for free.
  saveEdit: (
    s: string,
    c: EventPatchDraft,
    p: Picked | null,
    t: string,
    n: Notify | null,
    a: GuestNotice | null,
    r?: string | null,
  ) => {
    savedRow(r);
    return saveEdit(s, c, p, t, n, a);
  },
  choicesForEdit: (s: string, d: EventPatchDraft) => choicesForEdit(s, d),
  settleSplit: (token: string, action: SplitAction, title: string) =>
    settleSplit(token, action, title),
  noteSaved: (...a: unknown[]) => noteSaved(...a),
  noteSeriesNotice: (text: string) => noteSeriesNotice(text),
  // A reply from the editor (C14c), and the notice one leaves when it doesn't land.
  respond: (id: string, reply: Rsvp, note: string | null, scope: RsvpScope | null) =>
    respond(id, reply, note, scope),
  noteReply: (...a: unknown[]) => noteReply(...a),
}));

// The repeat menu's two reads, answered as each test says, and the mail app a description's mail
// link opens.
const openUrl = vi.fn<(url: string) => Promise<void>>();
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  recurrencePresets: (day: string) => recurrencePresets(day),
  describeRecurrence: (rule: RepeatRule, start: RepeatStart) => describeRecurrence(rule, start),
  openUrl: (url: string) => openUrl(url),
}));

vi.mock("../../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => deviceZone,
}));

// Standard depth unless a test says Power.
let depth = "standard";
vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth }),
}));

// The rest of the module is real: the description codec's link rule asks its `safeUrl`.
vi.mock("../../../lib/markdown", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  Markdown: ({ children }: { children: string }) => <div>{children}</div>,
}));

import { EventEditor, UNREADABLE_REPEAT } from "./EventEditor";
import {
  A11_NOTE,
  CALENDAR_ALONE,
  CALENDAR_WHOLE,
  NOT_MOVED_UNREAD,
  reasonText,
  savedNotice,
} from "../../../lib/calendarEdit/editReasons";
import { createTargets } from "../../../lib/calendarEdit/createTargets";

/** Why a description shows read-only (C19). */
const FORMATTING = reasonText("unsupported_formatting");

/** The description's box (C19d). */
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

/** What the user may change on someone else's event whose copy can reply (C14c). */
const YOURS_WITH_REPLY = "Your reply, notifications, colour and busy or free are yours to change.";

const row: CalendarEvent = {
  id: "gcal:me@x.com:me@x.com:abc",
  calendar_id: "gcal:me@x.com:me@x.com",
  summary: "Dentist",
  description: null,
  location: "High St",
  start: "2026-10-12T08:00:00Z",
  end: "2026-10-12T09:00:00Z",
  all_day: false,
  html_link: null,
  uid: "abc@google.com",
};

const ALL: FieldPermissions = {
  summary: true,
  time: true,
  repeat: true,
  location: true,
  description: true,
  show_as: true,
  visibility: true,
  guests: true,
  reminders: true,
  colour: true,
  meet: true,
  calendar: true,
  delete: true,
  respond: false,
  one_day: false,
  reasons: [],
};

/** No calendars to choose from (C18d): the editor keeps its "Saving to …" line unless a test gives
 *  it some. */
const NO_CALENDARS = {
  targets: null,
  overview: null,
  hidden: new Set<string>(),
  shapeOf: () => undefined,
} as const;

function event(over: Partial<EventForEdit> = {}): EventForEdit {
  return {
    summary: "Dentist",
    location: "High St",
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
    start_at: "2026-10-12T08:00:00Z",
    end_at: "2026-10-12T09:00:00Z",
    show_as: "busy",
    visibility: "default",
    attachments: [],
    html_link: null,
    guests: [],
    guests_can_modify: false,
    guests_can_invite_others: true,
    guests_can_see_other_guests: true,
    guests_omitted: false,
    organizer_email: "me@x.com",
    reminders: { use_default: true, overrides: [] },
    color_id: null,
    label_id: null,
    reply: null,
    organizer_self: true,
    // No call, on a calendar without Meet: no video-call row unless a test asks for one (C18).
    call: null,
    meet_offered: false,
    ...over,
  };
}

const ready = (ev: EventForEdit, permissions = ALL, session = "s1"): EditLoad => ({
  outcome: "ready",
  session,
  event: ev,
  permissions,
  seen: {
    summary: ev.summary,
    start: "2026-10-12T08:00:00Z",
    end: "2026-10-12T09:00:00Z",
    all_day: false,
    location: ev.location,
    removal: false,
  },
  series: null,
});

async function open(
  onClose = vi.fn(),
  onDelete?: (r: CalendarEvent) => void,
  clicked: CalendarEvent = row,
  calendar: Calendar | null = null,
) {
  render(
    <EventEditor
      row={clicked}
      calendar={calendar}
      account="me@x.com"
      suggestions={NO_SUGGESTIONS}
      milestone={null}
      colorOf={colourOf}
      {...NO_CALENDARS}
      onClose={onClose}
      onDelete={onDelete}
    />,
  );
  await screen.findByLabelText("Title");
  return onClose;
}

/** No one to suggest as a guest. */
const NO_SUGGESTIONS: readonly GuestSuggestion[] = [];

/** Every calendar in one colour: the colour list's "Calendar colour" swatch. */
const colourOf = () => "var(--ink4)";

/** A one-off's save: no scope, the draft's title for its notices, and nobody to email (no answer,
 *  no numbers). */
const UNSCOPED = [null, "Dentist", null, null] as const;

/** A rule from the repeat menu's set: `pattern`, every week or day, never ending. */
const ruleOf = (pattern: RepeatRule["pattern"], interval = 1): RepeatRule => ({
  pattern,
  interval,
  ends: { kind: "never" },
});
const DAILY = ruleOf({ kind: "daily" });
const MONDAYS = ruleOf({ kind: "weekly", days: ["mon"] });
const TUESDAYS = ruleOf({ kind: "weekly", days: ["tue"] });
const MON_WED = ruleOf({ kind: "weekly", days: ["mon", "wed"] });

/** The ready-made rules for a start date, as the backend gives them (the first two are enough). */
function presetsFor(day: string): RepeatPreset[] {
  return [
    { label: "Daily", rule: DAILY },
    day === "2026-10-13"
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

// jsdom ships no ResizeObserver, and the Repeat list's popover (escapeClipping) builds one.
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
  depth = "standard";
  recurrencePresets.mockImplementation(async (day) => presetsFor(day));
  describeRecurrence.mockImplementation(async (_rule, start) => readyAt(start));
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("EventEditor", () => {
  // A description PM can't write back shows read-only, what PM can of it (C19b): bold as bold, a
  // list as a list, escapes decoded once, and nothing of a script on screen. Its mail link opens the
  // mail app, and a line under it says so with the address when nothing opens.
  it("shows what PM can of a formatted description", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          description:
            "<b>Agenda</b><br><ul><li>One &amp; two</li></ul><script>alert(1)</script>&lt;i&gt; " +
            '<a href="mailto:ann@example.com">mail</a>',
        }),
      ),
    );
    openUrl.mockRejectedValue("PM couldn't open your mail app.");
    await open();
    const view = document.querySelector(".pm-rich") as HTMLElement;
    expect(view.querySelector("b")?.textContent).toBe("Agenda");
    expect(view.querySelector("ul > li")?.textContent).toBe("One & two");
    expect(view.querySelector("script, i")).toBeNull();
    expect(view.textContent).toBe("AgendaOne & two<i> mail");
    // Still read-only: no box to type it into (C19d offers Replace instead).
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    expect(screen.getByRole("group", { name: "Description" })).toBeTruthy();

    await act(async () => {
      expect(fireEvent.click(view.querySelector("a")!)).toBe(false);
    });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com");
    expect(
      screen
        .getByText("PM couldn't open your mail app. The address is ann@example.com.")
        .getAttribute("role"),
    ).toBe("status");
  });

  // Help mode (C20b §11): the editor explains itself, its description's buttons their own.
  it("carries the editor's help", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    expect(screen.getByRole("dialog", { name: "Edit event" }).getAttribute("data-help")).toBe(
      "calendar-editor",
    );
  });

  it("sends only what changed, then closes and says so, asking no choices of a one-off", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { summary: "Dentist (moved)" },
      null,
      "Dentist (moved)",
      null,
      null,
    );
    expect(noteSaved).toHaveBeenCalledWith(
      "Dentist (moved)",
      { outcome: "saved", warnings: [] },
      null,
      "2026-10-12",
      false,
      false,
      { rowId: row.id, sent: null },
      null,
    );
    expect(choicesForEdit).not.toHaveBeenCalled();
  });

  // C16d: the save names its row, so an Undo still offered for an earlier write of it goes, and the
  // notice gets the row and the answer sent, for the Undo the backend offers and its words.
  it("a save hands its row and answer to the notice", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const offer = { undo_token: "u1", undo_seconds: 8 };
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [], undo: offer });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(savedRow).toHaveBeenCalledWith(row.id);
    expect(noteSaved).toHaveBeenCalledWith(
      "Dentist (moved)",
      { outcome: "saved", warnings: [], undo: offer },
      null,
      "2026-10-12",
      false,
      false,
      { rowId: row.id, sent: null },
      null,
    );
  });

  // C18: the video call is a field of the editor, after the guests.
  it("shows an event's Meet call, and Remove sends remove", async () => {
    const call = {
      kind: "meet" as const,
      name: "Google Meet",
      join: "https://meet.google.com/abc-defg-hij",
      state: "ready" as const,
    };
    openForEdit.mockResolvedValue(ready(event({ call, meet_offered: true })));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const row_ = screen.getByRole("group", { name: "Video call" });
    expect(within(row_).getByRole("button", { name: "Join with Google Meet" })).toBeTruthy();
    fireEvent.click(within(row_).getByRole("button", { name: "Remove the video call" }));
    expect(within(row_).getByText("The video call will be removed when you save.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { meet: "remove" }, ...UNSCOPED);
  });

  it("Add sends add, and the saved notice says when Google is still making it", async () => {
    openForEdit.mockResolvedValue(ready(event({ meet_offered: true })));
    const outcome: Extract<WriteOutcome, { outcome: "saved" }> = {
      outcome: "saved",
      warnings: ["call_pending"],
    };
    saveEdit.mockResolvedValue(outcome);
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Add Google Meet video conferencing" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { meet: "add" }, ...UNSCOPED);
    expect(noteSaved).toHaveBeenCalledWith(
      "Dentist",
      outcome,
      null,
      "2026-10-12",
      false,
      false,
      { rowId: row.id, sent: null },
      null,
    );
    expect(savedNotice("Dentist", outcome, null).text).toBe(
      "Saved “Dentist”. Google is still making the video call; its link shows after a later refresh.",
    );
  });

  it("keeps another service's call, with its reason", async () => {
    const zoom = {
      kind: "add_on" as const,
      name: "Zoom Meeting",
      join: "https://zoom.us/j/987",
      state: "ready" as const,
    };
    openForEdit.mockResolvedValue(
      ready(event({ call: zoom, meet_offered: true }), {
        ...ALL,
        meet: false,
        reasons: ["other_call"],
      }),
    );
    await open();
    const row_ = screen.getByRole("group", { name: "Video call" });
    expect(within(row_).getByText("Zoom Meeting")).toBeTruthy();
    expect(within(row_).getByRole("button", { name: "Join" })).toBeTruthy();
    expect(within(row_).queryByRole("button", { name: /^Add|^Remove/ })).toBeNull();
    expect(
      screen.getByText(
        "This event's video call is a kind PM can't change, so PM leaves it as it is. Change it in Google Calendar.",
      ),
    ).toBeTruthy();
    // A Meet row the gate closed (someone else's event) can still be joined, never changed.
    cleanup();
    const meetCall = { ...zoom, kind: "meet" as const, name: "Google Meet" };
    openForEdit.mockResolvedValue(
      ready(event({ call: meetCall, meet_offered: true }), { ...ALL, meet: false }),
    );
    await open();
    const closed = screen.getByRole("group", { name: "Video call" });
    expect(
      within(closed).getByRole<HTMLButtonElement>("button", { name: "Remove the video call" })
        .disabled,
    ).toBe(true);
    expect(
      within(closed).getByRole<HTMLButtonElement>("button", { name: "Join with Google Meet" })
        .disabled,
    ).toBe(false);
  });

  it("turns Close off with Cancel while a save is out, and back on after", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    let answer: (a: SaveAnswer) => void = () => {};
    saveEdit.mockReturnValue(new Promise<SaveAnswer>((resolve) => (answer = resolve)));
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const close = () => screen.getByRole<HTMLButtonElement>("button", { name: "Close" });
    expect(close().disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" }).disabled).toBe(true);
    await act(async () => answer({ outcome: "failed", message: "PM couldn't reach Google." }));
    expect(close().disabled).toBe(false);
  });

  it("sends a date typed but not yet committed when Save is pressed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "13-10-2026" } });
    // Still focused: the field hasn't committed it.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    // The end moved with it, keeping the hour.
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      {
        time: {
          kind: "timed",
          start_date: "2026-10-13",
          start_time: "09:00",
          start_zone: "Europe/London",
          end_date: "2026-10-13",
          end_time: "10:00",
          end_zone: "Europe/London",
        },
      },
      ...UNSCOPED,
    );
  });

  it("closes without sending anything when nothing changed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("keeps the draft through a conflict, and names what Google's version replaced", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockResolvedValueOnce(ready(event({ summary: "Dentist (Google)" }), ALL, "s2"));
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["summary"] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (PM)" } });
    fireEvent.change(screen.getByLabelText("Location"), { target: { value: "Room 2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Google's version of the title is shown now/);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Dentist (Google)");
    expect(screen.getByLabelText<HTMLInputElement>("Location").value).toBe("Room 2");
    // The next save goes out on the new session, with only the user's own change.
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s2",
      { location: "Room 2" },
      null,
      "Dentist (Google)",
      null,
      null,
    );
  });

  it("asks before throwing a dirty draft away", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("greys out what a locked event can't change, and says why", async () => {
    openForEdit.mockResolvedValue(
      ready(event(), {
        ...ALL,
        summary: false,
        time: false,
        location: false,
        description: false,
        delete: false,
        reasons: ["locked"],
      }),
    );
    await open(vi.fn(), vi.fn());
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByText(/Google has locked this event's title/)).toBeTruthy();
    // Delete is wired (onDelete given), and still not offered.
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    // Notifications and colour are the user's own, which Google leaves open on a locked event (C13).
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Add notification" }).disabled,
    ).toBe(false);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Colour: Calendar colour" }).disabled,
    ).toBe(false);
  });

  it("a labelled event's colour and unreadable notifications stay as they are, saying why (C13)", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ label_id: "label-1", color_id: "5" }), {
        ...ALL,
        reminders: false,
        colour: false,
        reasons: ["label_read_only", "reminders_unreadable", "unsupported_formatting"],
      }),
    );
    await open(vi.fn(), vi.fn());
    expect(screen.getByText(/This description has formatting/)).toBeTruthy();
    // Each reason shows now that its row does (C1-M10).
    expect(screen.getByText(/This event has a Google Calendar label/)).toBeTruthy();
    expect(screen.getByText(/PM can't read one of this event's notifications/)).toBeTruthy();
    // The colour names the label that sets it, never the colour hidden under it, and is off.
    const colour = screen.getByRole<HTMLButtonElement>("button", {
      name: "Colour: Set by its Google Calendar label",
    });
    expect(colour.disabled).toBe(true);
    // No list is shown as the event's when PM can't read it: no defaults, no rows, no Add.
    const list = screen.getByRole("group", { name: "Notifications" });
    expect(within(list).queryByRole("button", { name: "Add notification" })).toBeNull();
    expect(within(list).queryByText(/default notifications/)).toBeNull();
    expect(within(list).getByText("Google Calendar sends these, not PM.")).toBeTruthy();
  });

  it("deletes what the editor showed, asking first when there are unsaved changes", async () => {
    openForEdit.mockResolvedValue(ready(event({ summary: "Dentist (Google)" })));
    const onDelete = vi.fn();
    await open(vi.fn(), onDelete);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Discard your changes and delete?")).toBeTruthy();
    expect(onDelete).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard and delete" }));
    // The row handed on carries Google's fresh copy (what the editor showed), not the mirror's.
    expect(onDelete).toHaveBeenCalledWith(
      expect.objectContaining({ id: row.id, summary: "Dentist (Google)" }),
    );
  });

  // C18c: Duplicate in the footer's left group, after Delete; the copy is read afresh, so the row is
  // handed on as clicked. Unsaved changes are asked about first.
  it("Duplicate from a dirty editor asks first", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onDuplicate = vi.fn();
    const onDelete = vi.fn();
    render(
      <EventEditor
        row={row}
        calendar={null}
        account="me@x.com"
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        {...NO_CALENDARS}
        onClose={vi.fn()}
        onDelete={onDelete}
        onDuplicate={onDuplicate}
        canDuplicate
      />,
    );
    await screen.findByLabelText("Title");
    const duplicate = screen.getByRole("button", { name: "Duplicate" });
    // Delete keeps to the left of it; the last of the left group pushes the rest right.
    expect(duplicate.parentElement?.className).toBe("mr-auto");
    expect(screen.getByRole("button", { name: "Delete" }).parentElement?.className).toBe("");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(duplicate);
    expect(await screen.findByText("Discard your changes and duplicate?")).toBeTruthy();
    expect(onDuplicate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(onDuplicate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    fireEvent.click(await screen.findByRole("button", { name: "Discard and duplicate" }));
    expect(onDuplicate).toHaveBeenCalledWith(row);
    expect(onDelete).not.toHaveBeenCalled();
  });

  // Review round 1 of PR 1: text that never reached the draft is lost by Duplicate and Delete too,
  // so they ask as closing does.
  it("Duplicate and Delete ask first when only an address it couldn't read stands", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onDuplicate = vi.fn();
    const onDelete = vi.fn();
    render(
      <EventEditor
        row={row}
        calendar={null}
        account="me@x.com"
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        {...NO_CALENDARS}
        onClose={vi.fn()}
        onDelete={onDelete}
        onDuplicate={onDuplicate}
        canDuplicate
      />,
    );
    await screen.findByLabelText("Title");
    const box = screen.getByRole("combobox", { name: "Add guests" });
    box.focus();
    fireEvent.change(box, { target: { value: "bob@" } });
    fireEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    expect(await screen.findByText("Discard your changes and duplicate?")).toBeTruthy();
    expect(onDuplicate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Discard your changes and delete?")).toBeTruthy();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it("Duplicate goes at once from a clean editor, and only where offered", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onDuplicate = vi.fn();
    const editor = (canDuplicate: boolean) => (
      <EventEditor
        row={row}
        calendar={null}
        account="me@x.com"
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        {...NO_CALENDARS}
        onClose={vi.fn()}
        onDuplicate={onDuplicate}
        canDuplicate={canDuplicate}
      />
    );
    const { rerender } = render(editor(true));
    await screen.findByLabelText("Title");
    // No Delete: Duplicate alone takes the left.
    expect(screen.getByRole("button", { name: "Duplicate" }).parentElement?.className).toBe(
      "mr-auto",
    );
    fireEvent.click(screen.getByRole("button", { name: "Duplicate" }));
    expect(onDuplicate).toHaveBeenCalledWith(row);
    rerender(editor(false));
    expect(screen.queryByRole("button", { name: "Duplicate" })).toBeNull();
  });

  it("refuses to save a date it couldn't read, rather than dropping it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "31-13-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/“31-13-2026” isn't a date PM can read/)).toBeTruthy();
    expect(saveEdit).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    // The field it was typed in keeps focus, rather than the page behind the dialog.
    expect(document.activeElement).toBe(start);
    expect(start.value).toBe("12-10-2026");
    // Saving again leaves that field on the date it put back, which nobody gave: still refused,
    // pressed with the field focused and with the press taking focus to Save first.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const save = screen.getByRole("button", { name: "Save" });
    act(() => save.focus());
    fireEvent.click(save);
    await act(async () => {});
    expect(saveEdit).not.toHaveBeenCalled();
    expect(screen.getByText(/“31-13-2026” isn't a date PM can read/)).toBeTruthy();
  });

  // Nothing else changed, but what was typed would be lost.
  it("asks before closing when only a date it couldn't read stands", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "31-13-2026" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(start.value).toBe("12-10-2026");
    expect(onClose).not.toHaveBeenCalled();
  });

  // The line names the field, since only a date typed or picked there ends the refusal; the field
  // is marked and described by the line, and a save the refusal stops puts focus there, whatever
  // had it. An All day switch hands the refusal to that half's field, and its name to the line.
  it("names the field a date was refused in, marks it, and a save it stops puts focus there", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const title = screen.getByLabelText<HTMLInputElement>("Title");
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    const typeOver = (input: HTMLInputElement, text: string) => {
      input.focus();
      fireEvent.change(input, { target: { value: "" } });
      fireEvent.change(input, { target: { value: text } });
      fireEvent.blur(input);
    };
    const saveFromTitle = async () => {
      act(() => title.focus());
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      await act(async () => {});
    };
    typeOver(end, "31-11-2026");
    // A start that moves the end to the day wanted: the end's refusal stands.
    typeOver(start, "30-11-2026");
    expect(end.value).toBe("30-11-2026");
    const line = screen.getByText(
      "End date: “31-11-2026” isn't a date PM can read. Type the end date again as dd-mm-yyyy, or pick it from the calendar.",
    );
    expect(line.getAttribute("role")).toBe("alert");
    expect(end.getAttribute("aria-invalid")).toBe("true");
    expect(end.getAttribute("aria-describedby")).toBe(line.id);
    expect(start.getAttribute("aria-invalid")).toBeNull();
    expect(start.getAttribute("aria-describedby")).toBeNull();
    await saveFromTitle();
    expect(saveEdit).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(end);
    // All day: the last day's refusal now, named so. The same text in the first day is said too,
    // and a save goes to the first day's field.
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    const first = screen.getByLabelText<HTMLInputElement>("First day");
    const last = screen.getByLabelText<HTMLInputElement>("Last day");
    const lastLine = screen.getByText(
      "Last day: “31-11-2026” isn't a date PM can read. Type the last day again as dd-mm-yyyy, or pick it from the calendar.",
    );
    expect(last.getAttribute("aria-invalid")).toBe("true");
    expect(last.getAttribute("aria-describedby")).toBe(lastLine.id);
    typeOver(first, "31-11-2026");
    const firstLine = screen.getByText(
      "First day: “31-11-2026” isn't a date PM can read. Type the first day again as dd-mm-yyyy, or pick it from the calendar.",
    );
    expect(first.getAttribute("aria-describedby")).toBe(firstLine.id);
    expect(lastLine.isConnected).toBe(true);
    // Each line has its own id, so each field is described by the line that names it: one id for
    // both would read the first day's line in Last day too.
    expect(firstLine.id).not.toBe(lastLine.id);
    const said = (field: HTMLElement) =>
      document.getElementById(field.getAttribute("aria-describedby") ?? "")?.textContent;
    expect(said(first)).toMatch(/^First day:/);
    expect(said(last)).toMatch(/^Last day:/);
    await saveFromTitle();
    expect(saveEdit).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(first);
    // Each given its date: the lines and the marks go, and the save goes.
    typeOver(first, "30-11-2026");
    typeOver(last, "30-11-2026");
    expect(screen.queryByText(/isn't a date PM can read/)).toBeNull();
    expect(first.getAttribute("aria-invalid")).toBeNull();
    expect(last.getAttribute("aria-describedby")).toBeNull();
    await saveFromTitle();
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: { kind: "all_day", first_day: "2026-11-30", last_day: "2026-11-30" } },
      ...UNSCOPED,
    );
  });

  it("asks before closing over a date typed but not yet committed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    end.focus();
    fireEvent.change(end, { target: { value: "13-10-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("moves a whole all-day event when its first day moves", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: { kind: "all_day", first_day: "2026-10-15", last_day: "2026-10-16" },
          start_at: null,
          end_at: null,
        }),
      ),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const first = screen.getByLabelText<HTMLInputElement>("First day");
    first.focus();
    fireEvent.change(first, { target: { value: "12-10-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: { kind: "all_day", first_day: "2026-10-12", last_day: "2026-10-13" } },
      ...UNSCOPED,
    );
  });

  it("turning All day on and off again leaves the time as Google holds it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const allDay = screen.getByRole("switch", { name: "All day" });
    fireEvent.click(allDay);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("turns the time controls off when this computer's zone is unknown", async () => {
    deviceZone = null;
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByText(/couldn't tell which time zone this computer is in/)).toBeTruthy();
    // Everything else still works.
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(false);
  });

  it("offers only ends after the start, each with the event's length", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    const end = screen.getByLabelText<HTMLSelectElement>("End time");
    const labels = [...end.options].map((o) => o.textContent);
    expect(labels[0]).toBe("09:15 (15 mins)");
    expect(labels).toContain("10:00 (1 hr)");
    expect(labels).not.toContain("08:00");
    expect(end.value).toBe("10:00");
  });

  it("keeps the event's length when the start moves", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "11:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: expect.objectContaining({ start_time: "11:00", end_time: "12:00" }) },
      ...UNSCOPED,
    );
  });

  it("gives a moved start an end after it, even from a 0-minute event", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: { ...event().time, end_time: "09:00" } as EventForEdit["time"],
          end_at: "2026-10-12T08:00:00Z",
        }),
      ),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "11:00" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    // An hour, as All day switched off gives.
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: expect.objectContaining({ start_time: "11:00", end_time: "12:00" }) },
      ...UNSCOPED,
    );
  });

  it("moves an end date set before the start up to the start's day", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: {
            ...event().time,
            end_date: "2026-10-13",
            end_time: "08:00",
          } as EventForEdit["time"],
          end_at: "2026-10-13T07:00:00Z",
        }),
      ),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const endDate = screen.getByLabelText<HTMLInputElement>("End date");
    endDate.focus();
    fireEvent.change(endDate, { target: { value: "05-10-2026" } });
    fireEvent.blur(endDate);
    // The date can't go before the start's day, and on that day 08:00 is before 09:00, so the end
    // takes the first time after the start.
    expect(endDate.value).toBe("12-10-2026");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: expect.objectContaining({ end_date: "2026-10-12", end_time: "09:15" }) },
      ...UNSCOPED,
    );
  });

  it("keeps the last day of an all-day event on or after its first", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: { kind: "all_day", first_day: "2026-10-15", last_day: "2026-10-16" },
          start_at: null,
          end_at: null,
        }),
      ),
    );
    await open();
    const last = screen.getByLabelText<HTMLInputElement>("Last day");
    last.focus();
    fireEvent.change(last, { target: { value: "10-10-2026" } });
    fireEvent.blur(last);
    expect(last.value).toBe("15-10-2026");
  });

  it("moves the end after the start when a new zone would put it first", async () => {
    // 09:00 London to 05:00 New York (09:00Z): an hour long. With the start in Los Angeles, 09:00
    // there is 16:00Z, after that end, so the end follows the start by the hour instead.
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: {
            ...event().time,
            end_time: "05:00",
            end_zone: "America/New_York",
          } as EventForEdit["time"],
          end_at: "2026-10-12T09:00:00Z",
        }),
      ),
    );
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Start zone" }));
    fireEvent.change(screen.getByLabelText("Filter timezones"), {
      target: { value: "los angeles" },
    });
    fireEvent.click(screen.getByTitle("America/Los_Angeles"));
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("13:00");
    expect(screen.queryByText("The event has to end after it starts.")).toBeNull();
  });

  it("says straight away what would stop the save", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "29-03-2026" } });
    fireEvent.blur(start);
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    // Before any Save: the clocks skip 01:30 in London that night.
    expect(screen.getByText(/01:30 doesn't happen in Europe\/London that night/)).toBeTruthy();
    // A save it stops leaves focus where it was.
    const title = screen.getByLabelText("Title");
    act(() => title.focus());
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(saveEdit).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title);
  });

  it("says the same problem once, for a start and an end in the same skipped hour", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "29-03-2026" } });
    fireEvent.blur(start);
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    fireEvent.change(screen.getByLabelText("End time"), { target: { value: "01:30" } });
    expect(screen.getAllByText(/01:30 doesn't happen in Europe\/London that night/)).toHaveLength(
      1,
    );
  });

  it("starts the End date the day after a start too late for any end that day", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: {
            ...event().time,
            start_time: "23:45",
            end_date: "2026-10-13",
            end_time: "00:45",
          } as EventForEdit["time"],
          start_at: "2026-10-12T22:45:00Z",
          end_at: "2026-10-12T23:45:00Z",
        }),
      ),
    );
    await open();
    const endDate = screen.getByLabelText<HTMLInputElement>("End date");
    endDate.focus();
    fireEvent.change(endDate, { target: { value: "12-10-2026" } });
    fireEvent.blur(endDate);
    // The box and the draft agree: no end on the 12th comes after 23:45.
    expect(endDate.value).toBe("13-10-2026");
    expect(screen.getByLabelText<HTMLSelectElement>("End time").value).toBe("00:45");
  });

  // A refusal is the field's that refused: only a date typed or picked there ends it, or Save would
  // send the very date the user was changing.
  it("says at once when a typed date can't be read, until that field is given a date", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    const refused = () => screen.queryByText(/“31-13-2026” isn't a date PM can read/);
    start.focus();
    fireEvent.change(start, { target: { value: "31-13-2026" } });
    fireEvent.blur(start);
    expect(refused()).not.toBeNull();
    // Back in the field and out again, on the date it put back: nobody gave that date.
    start.focus();
    fireEvent.blur(start);
    expect(refused()).not.toBeNull();
    // Nor does a time, or a date given to the end.
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "09:30" } });
    expect(refused()).not.toBeNull();
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    end.focus();
    fireEvent.change(end, { target: { value: "13-10-2026" } });
    fireEvent.blur(end);
    expect(end.value).toBe("13-10-2026");
    expect(refused()).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(saveEdit).not.toHaveBeenCalled();
    // The date it shows, typed over in that field: given, so it stands and the save goes.
    start.focus();
    fireEvent.change(start, { target: { value: "" } });
    fireEvent.change(start, { target: { value: "12-10-2026" } });
    fireEvent.blur(start);
    expect(refused()).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledTimes(1);
  });

  it("lets a description have several lines", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const desc = description();
    // Nothing between the box and the window swallows Enter (fireEvent is false if anything called
    // preventDefault), so the engine's own line break happens.
    expect(fireEvent.keyDown(desc, { key: "Enter" })).toBe(true);
    expect(fireEvent.keyDown(desc, { key: "Enter", shiftKey: true })).toBe(true);
    typeInto(desc, "line one", h("div", "line two"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { description: "line one\nline two" }, ...UNSCOPED);
  });

  // --- the description, formatted in place (C19d) ---

  it("a_formatted_description_is_edited_in_place", async () => {
    openForEdit.mockResolvedValue(ready(event({ description: "<b>Agenda</b><br>one" })));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const box = description();
    expect(box.querySelector("b")?.textContent).toBe("Agenda");
    expect(box.classList.contains("pm-desc-box")).toBe(true);
    expect(screen.getByRole("toolbar", { name: "Description formatting" })).toBeTruthy();
    typeInto(box, h("b", "Agenda"), h("br"), "one ", h("i", "two"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: "<b>Agenda</b><br>one <i>two</i>" },
      ...UNSCOPED,
    );
  });

  // C19 §7.8: what Google holds in its own wrapper, opened and left, is never sent.
  it("only_the_title_changed_sends_no_description", async () => {
    const held = "<html-blob><u></u><strong>Plan</strong><br>Room 2</html-blob>";
    openForEdit.mockResolvedValue(ready(event({ description: held })));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const box = description();
    box.focus();
    fireEvent.blur(box);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { summary: "Dentist (moved)" },
      null,
      "Dentist (moved)",
      null,
      null,
    );
  });

  // The backend refuses a table (C19c); PM shows what it can of it and offers to replace it. The
  // save then says it's the user's choice (`replace_formatting`), which alone reopens it.
  it("a_description_pm_cant_edit_shows_with_replace", async () => {
    const table = "<table><tr><td>Room</td><td>4.12</td></tr></table>";
    openForEdit.mockResolvedValue(
      ready(event({ description: table }), {
        ...ALL,
        description: false,
        reasons: ["unsupported_formatting"],
      }),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const view = screen.getByRole("group", { name: "Description" });
    expect(view.textContent).toBe("Room 4.12");
    const reason = screen.getByText(FORMATTING);
    expect(view.getAttribute("aria-describedby")).toBe(reason.id);
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    const ask = screen.getByRole("dialog", { name: "Replace the description with plain text?" });
    // Nothing changed until the user says so.
    expect(screen.getByRole("group", { name: "Description" })).toBeTruthy();
    fireEvent.click(within(ask).getByRole("button", { name: "Replace" }));
    const box = description();
    expect(document.activeElement).toBe(box);
    expect(box.textContent).toBe("Room 4.12");
    // The reason goes with the formatting.
    expect(screen.queryByText(FORMATTING)).toBeNull();
    typeInto(box, "Room 4.12 today");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: "Room 4.12 today", replace_formatting: true },
      ...UNSCOPED,
    );
  });

  it("a replaced description sent unchanged still says it was replaced", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ description: '<span style="color: red">Red</span> words' }), {
        ...ALL,
        description: false,
        reasons: ["unsupported_formatting"],
      }),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: "Red words", replace_formatting: true },
      ...UNSCOPED,
    );
  });

  // C19 review 1: words Google showed as words ("&lt;b&gt;") stay words in the replacement, in the
  // box and in what the save sends, never live markup that would bold a word or hide one.
  it.each([
    [
      "<h2>Use &lt;b&gt;bold&lt;/b&gt; tags</h2>",
      "Use <b>bold</b> tags",
      "Use &lt;b&gt;bold&lt;/b&gt; tags",
    ],
    [
      "<h2>Notes</h2>&lt;img src=x onerror=alert(1)&gt; after",
      "Notes<img src=x onerror=alert(1)> after",
      "Notes<br>&lt;img src=x onerror=alert(1)&gt; after",
    ],
    ['<font color="red">Tom &amp;amp; Jerry</font>', "Tom &amp; Jerry", "Tom &amp;amp; Jerry"],
  ])("replace keeps words Google showed as words: %j", async (held, shows, sent) => {
    openForEdit.mockResolvedValue(
      ready(event({ description: held }), {
        ...ALL,
        description: false,
        reasons: ["unsupported_formatting"],
      }),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    expect(description().textContent).toBe(shows);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: sent, replace_formatting: true },
      ...UNSCOPED,
    );
  });

  it("replace isn't offered while a save is out", async () => {
    let answer: (o: SaveAnswer) => void = () => {};
    saveEdit.mockReturnValue(new Promise<SaveAnswer>((resolve) => (answer = resolve)));
    openForEdit.mockResolvedValue(
      ready(event({ description: "<h2>Agenda</h2>" }), {
        ...ALL,
        description: false,
        reasons: ["unsupported_formatting"],
      }),
    );
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(saveEdit).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: "Replace with plain text…" })).toBeNull();
    await act(async () => answer({ outcome: "busy" }));
    expect(screen.getByRole("button", { name: "Replace with plain text…" })).toBeTruthy();
  });

  it("replace_is_not_offered_on_a_locked_description", async () => {
    const locked: FieldPermissions = {
      ...ALL,
      summary: false,
      time: false,
      repeat: false,
      location: false,
      description: false,
      guests: false,
      meet: false,
      calendar: false,
      reasons: ["locked"],
    };
    openForEdit.mockResolvedValue(
      ready(event({ description: "<table><tr><td>Room</td></tr></table>" }), locked),
    );
    await open();
    expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Room");
    expect(screen.queryByRole("button", { name: "Replace with plain text…" })).toBeNull();
    // A plain one locked shows the same way, with no box to type in.
    cleanup();
    openForEdit.mockResolvedValue(ready(event({ description: "Bring the form" }), locked));
    await open();
    expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Bring the form");
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Replace with plain text…" })).toBeNull();
  });

  // The backend lets through a link PM's own reading refuses (it reads tags only): the view says
  // why itself, and offers Replace. A link PM keeps keeps its address in the plain text; one to an
  // address PM never opens (ftp:) was never a link to it, and leaves its words.
  it.each([
    [
      '<a href="https://example.com/file" onclick="steal()">the file</a>',
      "the file (https://example.com/file)",
    ],
    ['<a href="ftp://example.com/file">the file</a>', "the file"],
  ])("a_link_the_backend_allows_but_pm_cant_open_still_offers_replace: %j", async (held, plain) => {
    openForEdit.mockResolvedValue(ready(event({ description: held })));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const view = screen.getByRole("group", { name: "Description" });
    expect(view.textContent).toBe("the file");
    expect(view.getAttribute("aria-describedby")).toBe(screen.getByText(FORMATTING).id);
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    expect(description().textContent).toBe(plain);
    expect(screen.queryByText(FORMATTING)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: plain, replace_formatting: true },
      ...UNSCOPED,
    );
  });

  it("a_conflict_on_the_description_reseeds_the_box", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event({ description: "<b>Plan</b>" })))
      .mockResolvedValueOnce(ready(event({ description: "<b>Plan</b> from Google" }), ALL, "s2"));
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["description"] });
    const onClose = await open();
    const box = description();
    typeInto(box, h("b", "Plan"), " from PM");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Google's version of the description is shown now/);
    // Google's copy is in the box now, made afresh.
    expect(description().textContent).toBe("Plan from Google");
    expect(description().querySelector("b")?.textContent).toBe("Plan");
    expect(onClose).not.toHaveBeenCalled();
    // Typing after it is the user's again.
    typeInto(description(), h("b", "Plan"), " from Google, and PM");
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s2",
      { description: "<b>Plan</b> from Google, and PM" },
      null,
      "Dentist",
      null,
      null,
    );
  });

  // After a Replace, a conflict that brings Google's description back puts the view back: the
  // replacement is gone with the user's edit, and so is the mark.
  it("a conflict on a replaced description shows Google's again, with Replace", async () => {
    const table = "<table><tr><td>Room</td></tr></table>";
    const held: FieldPermissions = {
      ...ALL,
      description: false,
      reasons: ["unsupported_formatting"],
    };
    openForEdit
      .mockResolvedValueOnce(ready(event({ description: table }), held))
      .mockResolvedValueOnce(
        ready(event({ description: "<table><tr><td>Hall</td></tr></table>" }), held, "s2"),
      );
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["description"] });
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Google's version of the description is shown now/);
    expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Hall");
    expect(screen.getByRole("button", { name: "Replace with plain text…" })).toBeTruthy();
    expect(screen.getByText(FORMATTING)).toBeTruthy();
  });

  it("the_dialog_closes_only_the_replace_question_on_escape", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ description: "<h2>Agenda</h2>" }), {
        ...ALL,
        description: false,
        reasons: ["unsupported_formatting"],
      }),
    );
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    expect(
      screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
    ).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(
      screen.queryByRole("dialog", { name: "Replace the description with plain text?" }),
    ).toBeNull();
    expect(screen.getByRole("dialog", { name: "Edit event" })).toBeTruthy();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    // Nothing was replaced.
    expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Agenda");
  });

  it("a mail link in the box opens only on Ctrl+click", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ description: '<a href="mailto:ann@example.com">Ann</a> and more' })),
    );
    openUrl.mockResolvedValue();
    await open();
    const link = description().querySelector("a") as HTMLAnchorElement;
    expect(fireEvent.click(link)).toBe(false);
    expect(openUrl).not.toHaveBeenCalled();
    await act(async () => {
      expect(fireEvent.click(link, { ctrlKey: true })).toBe(false);
    });
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com");
  });

  it("says why it couldn't open", async () => {
    openForEdit.mockResolvedValue({ outcome: "reauth" });
    render(
      <EventEditor
        row={row}
        calendar={null}
        account={null}
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        {...NO_CALENDARS}
        onClose={() => {}}
      />,
    );
    expect(await screen.findByText(/Google needs you to sign in again/)).toBeTruthy();
  });
});

// A repeating event's save (#884, C9b-1b): the choices first, the save for the occurrences picked
// with their token and the very changes the choices were asked for, and every way back to the
// editor keeping the draft, the words said once, and focus on Save.
describe("EventEditor, a repeating event", () => {
  const occurrence: CalendarEvent = { ...row, series_id: "abc", recurring: true };
  const offered = (token: string): ScopeRow => ({ row: "offered", warnings: [], token });
  const choices = (over: Partial<Extract<SeriesChoices, { outcome: "ready" }>> = {}) =>
    ({
      outcome: "ready",
      this: offered("t-this"),
      following: offered("t-following"),
      all: offered("t-all"),
      ...over,
    }) as SeriesChoices;
  const NOTICE = "PM closed before it finished an earlier change to this repeating event.";
  /** A one-event save of the title "Gym": no scope, and no email answer yet. */
  const UNSCOPED_GYM = [null, "Gym", null, null] as const;

  /** Open the occurrence, retitle it, press Save: the choices dialog, and the editor's Save. */
  async function retitle(title = "Gym") {
    const onClose = await open(vi.fn(), undefined, occurrence);
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: title } });
    fireEvent.click(editorSave);
    const dialog = await screen.findByRole("dialog", { name: "Edit repeating event" });
    return { onClose, editorSave, dialog };
  }
  const inDialog = (dialog: HTMLElement) => within(dialog);
  const choicesOpen = () => screen.queryByRole("dialog", { name: "Edit repeating event" });

  it("asks, then saves the occurrences picked with their token and the same changes", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices());
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { onClose, dialog } = await retitle();
    const d = inDialog(dialog);
    await d.findByRole("radio", { name: /^This event/ });
    expect(d.getByRole<HTMLInputElement>("radio", { name: /^This event/ }).checked).toBe(true);
    expect(saveEdit).not.toHaveBeenCalled();
    fireEvent.click(d.getByRole("radio", { name: /^All events/ }));
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(choicesForEdit).toHaveBeenCalledWith("s1", { summary: "Gym" });
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { summary: "Gym" },
      { scope: "all", token: "t-all", asAll: false },
      "Gym",
      null,
      null,
    );
    // The very object the choices were asked for: the token binds it.
    expect(saveEdit.mock.calls[0][1]).toBe(choicesForEdit.mock.calls[0][1]);
    // No Undo after a repeating save: the notice says how to change it back.
    expect(noteSaved).toHaveBeenCalledWith(
      "Gym",
      { outcome: "saved", warnings: [] },
      "all",
      "2026-10-12",
      false,
      false,
      { rowId: occurrence.id, sent: null },
      null,
    );
  });

  // C11c review round 2: "This and following" from the first occurrence goes as that row, with its
  // token, but saves every occurrence, so the notice is told to say so.
  it("tells the notice a 'this and following' pick from the first occurrence saved every one", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(
      choices({ following: { row: "as_all", warnings: [], token: "t-first" } }),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { onClose, dialog } = await retitle();
    const d = inDialog(dialog);
    fireEvent.click(await d.findByRole("radio", { name: /^This and following events/ }));
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    const picked = saveEdit.mock.calls[0][2];
    expect([picked?.scope, picked?.token]).toEqual(["following", "t-first"]);
    expect(noteSaved).toHaveBeenCalledWith(
      "Gym",
      { outcome: "saved", warnings: [] },
      "following",
      "2026-10-12",
      false,
      true,
      { rowId: occurrence.id, sent: null },
      null,
    );
  });

  it("asks when the editor knows its repeating event, whatever the row says", async () => {
    openForEdit.mockResolvedValue({
      ...ready(event()),
      series: {
        first: false,
        repeats: "Weekly on Monday",
        rule: {
          pattern: { kind: "weekly", days: ["mon"] },
          interval: 1,
          ends: { kind: "never" },
        },
        starts: "2026-10-05",
        zone: "Europe/London",
      },
    } as EditLoad);
    choicesForEdit.mockResolvedValue(choices());
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("dialog", { name: "Edit repeating event" });
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("pre-selects nothing for a visibility change, and Save waits for a pick", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(
      choices({
        this: {
          row: "refused",
          message: "Visibility applies to every occurrence, so it can't change for this one only.",
        },
      }),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByRole("combobox", { name: /Visibility/ }), {
      target: { value: "private" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const d = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await d.findByRole("radio", { name: /^All events/ });
    expect(d.getAllByRole<HTMLInputElement>("radio").some((r) => r.checked)).toBe(false);
    const send = d.getByRole<HTMLButtonElement>("button", { name: "Save" });
    expect(send.disabled).toBe(true);
    fireEvent.click(d.getByRole("radio", { name: /^All events/ }));
    expect(send.disabled).toBe(false);
  });

  it("puts focus on Back, so Enter on open sends nothing, and Back closes", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices());
    const { dialog } = await retitle();
    const d = inDialog(dialog);
    await d.findByRole("radio", { name: /^This event/ });
    const back = d.getByRole("button", { name: "Back" });
    await waitFor(() => expect(document.activeElement).toBe(back));
    expect(d.getAllByRole("radio")).not.toContain(document.activeElement);
    expect(document.activeElement).not.toBe(d.getByRole("button", { name: "Save" }));
    fireEvent.click(back);
    expect(choicesOpen()).toBeNull();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("goes Back to the draft, keeping the words said once and putting focus on Save", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices({ notice: NOTICE }));
    const { onClose, editorSave, dialog } = await retitle();
    const d = inDialog(dialog);
    expect(await d.findByText(NOTICE)).toBeTruthy();
    fireEvent.click(d.getByRole("button", { name: "Back" }));
    expect(choicesOpen()).toBeNull();
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym");
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  it("closes only the choices on Escape: no discard prompt", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices());
    const { onClose, editorSave } = await retitle();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(choicesOpen()).toBeNull();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  // C9b-1 critique 1 H1: an answer to an earlier ask never replaces a later one, so the save
  // never sends a draft older than the one on screen.
  it("saves the changes on screen when an earlier ask's answer arrives late", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    let first: (c: SeriesChoices) => void = () => {};
    let second: (c: SeriesChoices) => void = () => {};
    choicesForEdit
      .mockReturnValueOnce(new Promise((r) => (first = r)))
      .mockReturnValueOnce(new Promise((r) => (second = r)));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { onClose, editorSave, dialog } = await retitle("Gym");
    fireEvent.click(inDialog(dialog).getByRole("button", { name: "Back" }));
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym (moved)" } });
    fireEvent.click(editorSave);
    await screen.findByRole("dialog", { name: "Edit repeating event" });
    await act(async () => first(choices({ notice: NOTICE, this: offered("old") })));
    // Dropped, though its words aren't.
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    await act(async () => second(choices()));
    const d = inDialog(screen.getByRole("dialog", { name: "Edit repeating event" }));
    expect(d.queryByText(NOTICE)).toBeNull();
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledTimes(1);
    expect(saveEdit.mock.calls[0][1]).toBe(choicesForEdit.mock.calls[1][1]);
    expect(saveEdit.mock.calls[0][1]).toEqual({ summary: "Gym (moved)" });
    expect(saveEdit.mock.calls[0][2]).toEqual({ scope: "this", token: "t-this", asAll: false });
  });

  it("opens the choices when a save without one is answered that it repeats (R9)", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice", scope: true });
    choicesForEdit.mockResolvedValue(choices());
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const d = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await d.findByRole("radio", { name: /^This event/ });
    // The dialog is the answer: no "changed since you chose" line, no banner.
    expect(screen.queryByText(/since you chose/)).toBeNull();
    expect(choicesForEdit.mock.calls[0][1]).toBe(saveEdit.mock.calls[0][1]);
    // Focus is the dialog's, on Back: the save that opened it never pulls it to the editor's Save
    // behind (C9b-1 critique 1 M2).
    const back = d.getByRole("button", { name: "Back" });
    await new Promise((r) => setTimeout(r, 10));
    expect(document.activeElement).toBe(back);
  });

  it("asks again once when the plan changed since the choice, then says so", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices());
    saveEdit.mockResolvedValue({ outcome: "needs_choice", scope: true });
    const { onClose, dialog } = await retitle();
    await inDialog(dialog).findByRole("radio", { name: /^This event/ });
    fireEvent.click(inDialog(dialog).getByRole("button", { name: "Save" }));
    const again = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    expect(
      await again.findByText(/changed since you chose which occurrences to change/),
    ).toBeTruthy();
    await again.findByRole("radio", { name: /^This event/ });
    fireEvent.click(again.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(choicesOpen()).toBeNull());
    expect(
      await screen.findByText(/changed since you chose which occurrences to change/),
    ).toBeTruthy();
    expect(choicesForEdit).toHaveBeenCalledTimes(2);
    expect(saveEdit).toHaveBeenCalledTimes(2);
    expect(onClose).not.toHaveBeenCalled();
    // A new Save starts its count afresh: the plan changing once more is asked again, not refused.
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice", scope: true });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const next = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await next.findByRole("radio", { name: /^This event/ });
    fireEvent.click(next.getByRole("button", { name: "Save" }));
    const reasked = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    expect(
      await reasked.findByText(/changed since you chose which occurrences to change/),
    ).toBeTruthy();
    expect(choicesForEdit).toHaveBeenCalledTimes(4);
  });

  it("closes on a partial save it started, and stays open when an earlier change blocked it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(choices());
    const partial = {
      outcome: "partial_split",
      token: "p",
      offers: ["finish"],
      from: "2026-11-02",
    } as const;
    saveEdit.mockResolvedValueOnce({ ...partial, offers: ["finish"], newPartial: false });
    const { onClose, dialog } = await retitle();
    await inDialog(dialog).findByRole("radio", { name: /^This event/ });
    fireEvent.click(inDialog(dialog).getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(/^PM is still finishing an earlier change to it from 02-11/),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym");
    // New: Google holds part of it, and its notice outlives the editor.
    saveEdit.mockResolvedValueOnce({ ...partial, offers: ["finish"], newPartial: true });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const d = inDialog(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await d.findByRole("radio", { name: /^This event/ });
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("rebases instead of asking when Google changed the event, keeping the words said once", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockResolvedValueOnce(ready(event({ summary: "Gym (Google)" }), ALL, "s2"));
    choicesForEdit.mockResolvedValue(
      choices({ notice: NOTICE, following: { row: "conflict", fields: ["summary"] } }),
    );
    await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Location"), { target: { value: "Room 2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(/This event changed in Google while you were editing/),
    ).toBeTruthy();
    expect(choicesOpen()).toBeNull();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym (Google)");
    expect(screen.getByLabelText<HTMLInputElement>("Location").value).toBe("Room 2");
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("says why when the choices are refused", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "read_only", reason: "special_type" },
    });
    await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    // C20: the special kinds PM can't change, in their narrowed words.
    expect(await screen.findByText(/PM can't change focus time/)).toBeTruthy();
    expect(choicesOpen()).toBeNull();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // C9b-1b review F3 / F9: what the choices answer besides "ready", each pinned.
  it("saves the one event when Google's copy no longer repeats", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue({ outcome: "not_repeating" });
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledTimes(1);
    expect(saveEdit).toHaveBeenCalledWith("s1", { summary: "Gym" }, null, "Gym", null, null);
    expect(saveEdit.mock.calls[0][1]).toBe(choicesForEdit.mock.calls[0][1]);
  });

  it("stops when the save wants a choice and the choices say it doesn't repeat", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    // Twice each, then answers that end a loop, so one would fail here rather than hang.
    saveEdit
      .mockResolvedValueOnce({ outcome: "needs_choice", scope: true })
      .mockResolvedValueOnce({ outcome: "needs_choice", scope: true })
      .mockResolvedValue({ outcome: "failed", message: "Sent a third time." });
    choicesForEdit
      .mockResolvedValueOnce({ outcome: "not_repeating" })
      .mockResolvedValueOnce({ outcome: "not_repeating" })
      .mockResolvedValue(choices());
    const onClose = await open();
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    expect(
      await screen.findByText(/changed since you chose which occurrences to change/),
    ).toBeTruthy();
    await new Promise((r) => setTimeout(r, 20));
    expect(choicesForEdit).toHaveBeenCalledTimes(2);
    expect(saveEdit).toHaveBeenCalledTimes(2);
    expect(choicesOpen()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  // C12 §14.1, §14.2: whether Google emails the guests is no choice of occurrences. Asking those
  // would say "doesn't repeat" and send it again unanswered, round and round; the email question
  // asks it instead, with the backend's numbers (no "again": nobody had answered).
  it("asks the backend's guests question itself, and never which occurrences", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const guests = {
      updated: 0,
      invited: 1,
      removed: 0,
      told_cancelled: 0,
      cancelled: false,
      people: 1,
      emails: 1,
      complete: true,
    };
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice", scope: false, guests });
    const onClose = await open();
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    const prompt = await screen.findByRole("dialog", { name: "Send invitation emails to guests?" });
    expect(within(prompt).getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    expect(within(prompt).queryByText(/PM checked with Google/)).toBeNull();
    expect(saveEdit).toHaveBeenCalledTimes(1);
    expect(choicesForEdit).not.toHaveBeenCalled();
    expect(choicesOpen()).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    saveEdit.mockResolvedValueOnce({ outcome: "saved", warnings: [] });
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" }, null, "Gym", "all", guests);
  });

  // C12c review: the row still says it repeats, Google's copy no longer does, and it has guests. The
  // one-event save that follows "doesn't repeat" is asked whether Google emails them: that question
  // opens with the backend's numbers, rather than becoming a banner no Save could get past.
  it("asks the guests question after the choices say it doesn't repeat", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue({ outcome: "not_repeating" });
    const guests: GuestNotice = {
      updated: 1,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: false,
      people: 1,
      emails: 1,
      complete: true,
    };
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice", scope: false, guests });
    const onClose = await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    expect(within(prompt).getByText("1 guest already invited will get an update.")).toBeTruthy();
    expect(screen.queryByText(/Choose whether Google emails the guests/)).toBeNull();
    expect(saveEdit).toHaveBeenCalledWith("s1", { summary: "Gym" }, ...UNSCOPED_GYM);
    saveEdit.mockResolvedValueOnce({ outcome: "saved", warnings: [] });
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledTimes(2);
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" }, null, "Gym", "all", guests);
  });

  it("says why the choices couldn't be read, with focus back on Save", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockRejectedValue(new Error("PM couldn't reach the calendar."));
    await open(vi.fn(), undefined, occurrence);
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    expect(await screen.findByText("PM couldn't reach the calendar.")).toBeTruthy();
    expect(choicesOpen()).toBeNull();
    expect(saveEdit).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  it("rebases when the choices are refused because Google changed the event", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockResolvedValueOnce(ready(event({ summary: "Gym (Google)" }), ALL, "s2"));
    choicesForEdit.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "conflict", fields: ["summary"] },
    });
    await open(vi.fn(), undefined, occurrence);
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Location"), { target: { value: "Room 2" } });
    fireEvent.click(editorSave);
    expect(
      await screen.findByText(/This event changed in Google while you were editing/),
    ).toBeTruthy();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym (Google)");
    expect(screen.getByLabelText<HTMLInputElement>("Location").value).toBe("Room 2");
    expect(saveEdit).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  // C9b-1b review F2: the rebase's read failing is said, with Save held while it runs and focus
  // back on it after.
  it("says why a rebase after the choices couldn't read the event, holding Save meanwhile", async () => {
    let reread: (e: unknown) => void = () => {};
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockReturnValueOnce(new Promise((_, reject) => (reread = reject)));
    choicesForEdit.mockResolvedValue(choices({ following: { row: "conflict", fields: [] } }));
    await open(vi.fn(), undefined, occurrence);
    const editorSave = screen.getByRole<HTMLButtonElement>("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    await waitFor(() => expect(openForEdit).toHaveBeenCalledTimes(2));
    expect(editorSave.disabled).toBe(true);
    await act(async () => reread(new Error("PM couldn't open the event.")));
    expect(await screen.findByText("PM couldn't open the event.")).toBeTruthy();
    expect(choicesOpen()).toBeNull();
    expect(editorSave.disabled).toBe(false);
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym");
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  // A conflict means nothing was written: a rebase that can't read the event says why, and
  // doesn't mark the save as possibly in Google.
  it("keeps a save Google refused as a conflict out of doubt when the rebase can't read", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockRejectedValueOnce(new Error("PM couldn't open the event."));
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["summary"] });
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("PM couldn't open the event.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Nothing has been saved to Google yet.")).toBeTruthy();
  });

  it("settles an earlier change from the dialog, then closes it, keeping the words said once", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    choicesForEdit.mockResolvedValue(
      choices({
        notice: NOTICE,
        following: { row: "pending", token: "p", offers: ["finish", "remove"], from: "2026-11-02" },
        all: { row: "pending", token: "p", offers: ["finish", "remove"], from: "2026-11-02" },
      }),
    );
    settleSplit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const { onClose, editorSave, dialog } = await retitle();
    fireEvent.click(await inDialog(dialog).findByRole("button", { name: "Finish" }));
    await waitFor(() => expect(choicesOpen()).toBeNull());
    expect(settleSplit).toHaveBeenCalledWith("p", "finish", "Gym");
    expect(noteSeriesNotice).toHaveBeenCalledWith(NOTICE);
    expect(saveEdit).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });
});

// Changing how it repeats (C11d): a one-off given a rule saves alone once the backend has said where
// it starts; an occurrence's rule is its repeating event's, so the choices ask, with "This event"
// refused for a new one, and the event's own rule, in its own words, never stops a save or follows a
// moved date.
describe("EventEditor, how it repeats", () => {
  const occurrence: CalendarEvent = { ...row, series_id: "abc", recurring: true };
  const offered = (token: string): ScopeRow => ({ row: "offered", warnings: [], token });
  const RULE_IS_SERIES_WIDE = "Changing how it repeats can't apply to just this occurrence.";
  const choices = (over: Partial<Extract<SeriesChoices, { outcome: "ready" }>> = {}) =>
    ({
      outcome: "ready",
      this: { row: "refused", message: RULE_IS_SERIES_WIDE },
      following: offered("t-following"),
      all: offered("t-all"),
      ...over,
    }) as SeriesChoices;
  const series = (over: Partial<SeriesInfo> = {}): SeriesInfo => ({
    first: false,
    repeats: "Weekly on Monday",
    rule: MONDAYS,
    starts: "2026-10-05",
    zone: "Europe/London",
    ...over,
  });
  const repeatingLoad = (over: Partial<SeriesInfo> = {}, ev = event()) =>
    ({ ...ready(ev), series: series(over) }) as EditLoad;
  /** A timed event on `date`, 09:00 to 10:00 in London. */
  const timedOn = (date: string): EventForEdit["time"] => ({
    kind: "timed",
    start_date: date,
    start_time: "09:00",
    start_zone: "Europe/London",
    end_date: date,
    end_time: "10:00",
    end_zone: "Europe/London",
  });

  const repeat = () => screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / });
  const editorSave = () => screen.getByRole<HTMLButtonElement>("button", { name: "Save" });
  /** The rule the Repeat button names. */
  const shownRepeat = () =>
    repeat()
      .getAttribute("aria-label")
      ?.replace(/^Repeat: /, "");

  /** Click the Repeat row whose words are `label`, once the menu has its ready-made rules, and let
   *  the backend's words for it land. */
  async function pickRepeat(label: string) {
    await act(async () => {});
    fireEvent.click(repeat());
    const list = screen.getByRole("group", { name: "Repeat" });
    fireEvent.click(within(list).getByRole("radio", { name: label }), { detail: 1 });
    await act(async () => {});
  }

  it("a rule change on an occurrence asks with This event refused and nothing pre-selected", async () => {
    openForEdit.mockResolvedValue(repeatingLoad());
    choicesForEdit.mockResolvedValue(choices());
    // The words for the new rule never come: an occurrence's save doesn't wait for them.
    describeRecurrence.mockReturnValue(new Promise<RepeatView>(() => {}));
    await open(vi.fn(), undefined, occurrence);
    expect(shownRepeat()).toBe("Weekly on Monday");
    await pickRepeat("Daily");
    // Read from where the repeating event starts, at this occurrence's time.
    expect(describeRecurrence).toHaveBeenLastCalledWith(DAILY, {
      kind: "timed",
      date: "2026-10-05",
      time: "09:00",
      zone: "Europe/London",
    });
    expect(editorSave().disabled).toBe(false);
    fireEvent.click(editorSave());
    const d = within(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await d.findByRole("radio", { name: /^All events/ });
    expect(choicesForEdit).toHaveBeenCalledWith("s1", {
      recurrence: { kind: "rule", rule: DAILY },
    });
    expect(d.getByRole<HTMLInputElement>("radio", { name: /^This event/ }).disabled).toBe(true);
    expect(d.getAllByRole<HTMLInputElement>("radio").some((r) => r.checked)).toBe(false);
    expect(
      d.getAllByText((_, el) => !!el?.textContent?.endsWith(RULE_IS_SERIES_WIDE)).length,
    ).toBeGreaterThan(0);
    expect(d.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
  });

  // Where a new rule starts, or why it can't, depends on which occurrences it's for: each choice says
  // it, never the Repeat row.
  it("a repeating event's new rule says nothing of where it starts under the row", async () => {
    const GAP =
      "The first occurrence would be on 29-03-2027, when the clocks skip that time. Pick a time outside that hour.";
    for (const view of [
      { outcome: "refused", message: GAP },
      { outcome: "ready", summary: "Daily", starts: "2026-10-06", moved: true },
    ] as RepeatView[]) {
      openForEdit.mockResolvedValue(repeatingLoad());
      describeRecurrence.mockResolvedValue(view);
      await open(vi.fn(), undefined, occurrence);
      await pickRepeat("Daily");
      expect(screen.queryByText(GAP)).toBeNull();
      expect(screen.queryByText(/^Starts on /)).toBeNull();
      expect(editorSave().disabled).toBe(false);
      cleanup();
    }
  });

  it("Does not repeat on an occurrence sends the once draft with the picked scope", async () => {
    openForEdit.mockResolvedValue(repeatingLoad());
    choicesForEdit.mockResolvedValue(choices());
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open(vi.fn(), undefined, occurrence);
    await pickRepeat("Does not repeat");
    fireEvent.click(editorSave());
    const d = within(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    fireEvent.click(await d.findByRole("radio", { name: /^All events/ }));
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { recurrence: { kind: "once" } },
      { scope: "all", token: "t-all", asAll: false },
      "Dentist",
      null,
      null,
    );
    // Its notice says one event is left (C11d review 1).
    expect(noteSaved).toHaveBeenCalledWith(
      "Dentist",
      { outcome: "saved", warnings: [] },
      "all",
      "2026-10-12",
      true,
      false,
      expect.objectContaining({ sent: null }),
      null,
    );
  });

  // Google lets every occurrence end before the one opened: Custom… checks the end date against the
  // repeating event's first day, never this occurrence's, and the choices say what each would do
  // (C11d review 5).
  it("Custom… on a later occurrence takes an end date between the first and it", async () => {
    openForEdit.mockResolvedValue(repeatingLoad());
    choicesForEdit.mockResolvedValue(choices());
    await open(vi.fn(), undefined, occurrence);
    await act(async () => {});
    fireEvent.click(repeat());
    fireEvent.click(
      within(screen.getByRole("group", { name: "Repeat" })).getByRole("button", {
        name: "Custom…",
      }),
      { detail: 1 },
    );
    const custom = within(await screen.findByRole("dialog", { name: "Custom recurrence" }));
    fireEvent.click(custom.getByLabelText("On"));
    const end = custom.getByLabelText<HTMLInputElement>("End date");
    // Before the first occurrence, Monday 05-10: said, and Done waits.
    for (const [text, refused] of [
      ["04-10-2026", true],
      ["08-10-2026", false],
    ] as const) {
      end.focus();
      fireEvent.change(end, { target: { value: text } });
      fireEvent.blur(end);
      expect(!!custom.queryByText("The end date is before the first occurrence.")).toBe(refused);
      expect(custom.getByRole<HTMLButtonElement>("button", { name: "Done" }).disabled).toBe(
        refused,
      );
    }
    // After the first, before this one (Monday 12-10): it goes.
    fireEvent.click(custom.getByRole("button", { name: "Done" }));
    await act(async () => {});
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    fireEvent.click(editorSave());
    await screen.findByRole("dialog", { name: "Edit repeating event" });
    expect(choicesForEdit).toHaveBeenCalledWith("s1", {
      recurrence: {
        kind: "rule",
        rule: { ...MONDAYS, ends: { kind: "on_date", date: "2026-10-08" } },
      },
    });
  });

  it("a one-off given a preset saves without the dialog", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    expect(shownRepeat()).toBe("Does not repeat");
    await pickRepeat("Weekly on Monday");
    // Read at the event's own start, with where it ends.
    expect(describeRecurrence).toHaveBeenLastCalledWith(MONDAYS, {
      kind: "timed",
      date: "2026-10-12",
      time: "09:00",
      zone: "Europe/London",
      end: { date: "2026-10-12", time: "10:00", zone: "Europe/London" },
    });
    fireEvent.click(editorSave());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { recurrence: { kind: "rule", rule: MONDAYS } },
      ...UNSCOPED,
    );
    expect(choicesForEdit).not.toHaveBeenCalled();
  });

  it("a one-off given Mon, Wed on a Tuesday says it starts Wednesday", async () => {
    openForEdit.mockResolvedValue(ready(event({ time: timedOn("2026-10-13") })));
    describeRecurrence.mockResolvedValue({
      outcome: "ready",
      summary: "Weekly on Monday and Wednesday",
      starts: "2026-10-14",
      moved: true,
    });
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    await act(async () => {});
    fireEvent.click(repeat());
    fireEvent.click(
      within(screen.getByRole("group", { name: "Repeat" })).getByRole("button", {
        name: "Custom…",
      }),
      { detail: 1 },
    );
    const custom = within(await screen.findByRole("dialog", { name: "Custom recurrence" }));
    // Weekly on the start's own Tuesday to begin with: Monday and Wednesday instead.
    for (const day of ["Tuesday", "Monday", "Wednesday"]) {
      fireEvent.click(custom.getByRole("button", { name: day }));
    }
    fireEvent.click(custom.getByRole("button", { name: "Done" }));
    await act(async () => {});
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    expect(shownRepeat()).toBe("Weekly on Monday and Wednesday");
    expect(
      screen.getByText(/^Starts on Wed 14-10(-2026)?, the first day this repeat allows\.$/),
    ).toBeTruthy();
    // Said, never written into the date: the save moves it.
    expect(screen.getByLabelText<HTMLInputElement>("Start date").value).toBe("13-10-2026");
    fireEvent.click(editorSave());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { recurrence: { kind: "rule", rule: MON_WED } },
      ...UNSCOPED,
    );
  });

  it("a one-off's repeat the backend refuses holds Save, and one it hasn't said yet too", async () => {
    const GAP =
      "The first occurrence would be on 29-03-2027, when the clocks skip that time. Pick a time outside that hour.";
    openForEdit.mockResolvedValue(ready(event()));
    describeRecurrence.mockResolvedValue({ outcome: "refused", message: GAP });
    await open();
    await pickRepeat("Weekly on Monday");
    expect(screen.getByText(GAP)).toBeTruthy();
    expect(editorSave().disabled).toBe(true);
    // A start typed and saved at once was never asked about: the save waits for its words.
    let answer: (v: RepeatView) => void = () => {};
    describeRecurrence.mockImplementation((_rule, start) =>
      start.date === "2026-10-19"
        ? new Promise<RepeatView>((r) => (answer = r))
        : Promise.resolve(readyAt(start)),
    );
    await pickRepeat("Daily");
    expect(editorSave().disabled).toBe(false);
    const date = screen.getByLabelText<HTMLInputElement>("Start date");
    date.focus();
    fireEvent.change(date, { target: { value: "19-10-2026" } });
    fireEvent.click(editorSave());
    await act(async () => {});
    expect(saveEdit).not.toHaveBeenCalled();
    await act(async () => answer({ outcome: "refused", message: GAP }));
    expect(saveEdit).not.toHaveBeenCalled();
    // Not repeating, it saves as any change does.
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    await pickRepeat("Does not repeat");
    expect(editorSave().disabled).toBe(false);
  });

  it("holds every save while the Custom dialog is open", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    await act(async () => {});
    fireEvent.click(repeat());
    fireEvent.click(
      within(screen.getByRole("group", { name: "Repeat" })).getByRole("button", {
        name: "Custom…",
      }),
      { detail: 1 },
    );
    await screen.findByRole("dialog", { name: "Custom recurrence" });
    // Behind the dialog, so out of reach but for a click that gets there anyway.
    fireEvent.click(screen.getByRole("button", { name: "Save", hidden: true }));
    await act(async () => {});
    expect(saveEdit).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "Custom recurrence" })).toBeTruthy();
    // Escape closes only it: the editor stays, asking nothing, its repeat as it was.
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Edit event" })).toBeTruthy();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
    expect(shownRepeat()).toBe("Does not repeat");
    await waitFor(() => expect(document.activeElement).toBe(repeat()));
  });

  it("an unreadable repeating event disables Repeat with the reason", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    await open(vi.fn(), undefined, occurrence);
    expect(shownRepeat()).toBe("Repeats");
    expect(repeat().disabled).toBe(true);
    expect(screen.getByText(UNREADABLE_REPEAT)).toBeTruthy();
    // Everything else still changes.
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(false);
  });

  it("a custom rule shows its words and stays unless replaced", async () => {
    // One from the menu's set that no ready-made rule is, and one PM can only show.
    for (const over of [
      { repeats: "Every 2 weeks on Monday", rule: ruleOf({ kind: "weekly", days: ["mon"] }, 2) },
      { repeats: "Repeats (custom rule)", rule: null },
    ]) {
      openForEdit.mockResolvedValue(repeatingLoad(over));
      choicesForEdit.mockResolvedValue(choices({ this: offered("t-this") }));
      await open(vi.fn(), undefined, occurrence);
      await act(async () => {});
      expect(shownRepeat()).toBe(over.repeats);
      // Its own words came with it: nothing to ask.
      expect(describeRecurrence).not.toHaveBeenCalled();
      fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
      fireEvent.click(editorSave());
      await screen.findByRole("dialog", { name: "Edit repeating event" });
      expect(choicesForEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" });
      cleanup();
      vi.clearAllMocks();
    }
    // Replaced, the new rule is sent.
    openForEdit.mockResolvedValue(repeatingLoad({ repeats: "Repeats (custom rule)", rule: null }));
    choicesForEdit.mockResolvedValue(choices());
    await open(vi.fn(), undefined, occurrence);
    await pickRepeat("Weekly on Monday");
    fireEvent.click(editorSave());
    await screen.findByRole("dialog", { name: "Edit repeating event" });
    expect(choicesForEdit).toHaveBeenLastCalledWith("s1", {
      recurrence: { kind: "rule", rule: MONDAYS },
    });
  });

  it("the zone note uses the repeating event's zone", async () => {
    // Shown in London here, it repeats in New York's.
    openForEdit.mockResolvedValue(repeatingLoad({ zone: "America/New_York" }));
    await open(vi.fn(), undefined, occurrence);
    expect(screen.getByText("Repeats in New York time.")).toBeTruthy();
    cleanup();
    deviceZone = "America/New_York";
    await open(vi.fn(), undefined, occurrence);
    expect(screen.queryByText(/^Repeats in /)).toBeNull();
    cleanup();
    // One that repeats by days has no zone to say.
    deviceZone = "Europe/London";
    openForEdit.mockResolvedValue(repeatingLoad({ zone: null }));
    await open(vi.fn(), undefined, occurrence);
    expect(screen.queryByText(/^Repeats in /)).toBeNull();
    cleanup();
    // A rule PM can only show repeats in its event's zone too (C11d review 1).
    openForEdit.mockResolvedValue(
      repeatingLoad({ rule: null, repeats: "Repeats (custom rule)", zone: "America/New_York" }),
    );
    await open(vi.fn(), undefined, occurrence);
    expect(shownRepeat()).toBe("Repeats (custom rule)");
    expect(screen.getByText("Repeats in New York time.")).toBeTruthy();
  });

  // C11d review 1: the event's own rule keeps its row once the list has closed on another, as in
  // Google's list, so picking it again changes nothing.
  it("the event's own rule can be picked again after the list closed on another", async () => {
    for (const over of [
      { repeats: "Every 2 weeks on Monday", rule: ruleOf({ kind: "weekly", days: ["mon"] }, 2) },
      { repeats: "Repeats (custom rule)", rule: null },
    ]) {
      openForEdit.mockResolvedValue(repeatingLoad(over));
      choicesForEdit.mockResolvedValue(choices({ this: offered("t-this") }));
      await open(vi.fn(), undefined, occurrence);
      fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
      await act(async () => {});
      fireEvent.click(repeat());
      // An arrow key moves the choice to "Does not repeat" (a click no pointer made); Escape closes.
      const once = within(screen.getByRole("group", { name: "Repeat" })).getByRole("radio", {
        name: "Does not repeat",
      });
      once.focus();
      fireEvent.click(once, { detail: 0 });
      fireEvent.keyDown(once, { key: "Escape" });
      expect(screen.queryByRole("group", { name: "Repeat" })).toBeNull();
      expect(shownRepeat()).toBe("Does not repeat");
      await pickRepeat(over.repeats);
      expect(shownRepeat()).toBe(over.repeats);
      fireEvent.click(editorSave());
      await screen.findByRole("dialog", { name: "Edit repeating event" });
      expect(choicesForEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" });
      cleanup();
      vi.clearAllMocks();
    }
  });

  // C11d review 1: switched back to the kind Google holds, the time is Google's again, and a rule
  // picked here follows the date it lands on, as it follows any other time change.
  it("a picked rule follows the date switching All day back restores", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    await pickRepeat("Weekly on Monday");
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    const first = screen.getByLabelText<HTMLInputElement>("First day");
    first.focus();
    fireEvent.change(first, { target: { value: "13-10-2026" } });
    fireEvent.blur(first);
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Tuesday");
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    await act(async () => {});
    expect(screen.getByLabelText<HTMLInputElement>("Start date").value).toBe("12-10-2026");
    expect(shownRepeat()).toBe("Weekly on Monday");
    // Google's time again, so only the rule goes, on the day it shows.
    fireEvent.click(editorSave());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { recurrence: { kind: "rule", rule: MONDAYS } },
      ...UNSCOPED,
    );
  });

  it("moving the date doesn't rewrite an untouched series rule", async () => {
    openForEdit.mockResolvedValue(repeatingLoad());
    choicesForEdit.mockResolvedValue(choices({ this: offered("t-this") }));
    await open(vi.fn(), undefined, occurrence);
    const date = screen.getByLabelText<HTMLInputElement>("Start date");
    date.focus();
    fireEvent.change(date, { target: { value: "13-10-2026" } });
    fireEvent.blur(date);
    await act(async () => {});
    // Its own rule, in its own words, whatever the day.
    expect(shownRepeat()).toBe("Weekly on Monday");
    fireEvent.click(editorSave());
    await screen.findByRole("dialog", { name: "Edit repeating event" });
    expect(choicesForEdit.mock.calls[0][1]).not.toHaveProperty("recurrence");
    cleanup();
    // A rule picked here follows the date, as a new event's does.
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    await pickRepeat("Weekly on Monday");
    const moved = screen.getByLabelText<HTMLInputElement>("Start date");
    moved.focus();
    fireEvent.change(moved, { target: { value: "13-10-2026" } });
    fireEvent.blur(moved);
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Tuesday");
  });
});

// C9b-2: a milestone linked to a repeating event takes its date from the next occurrence, so moving
// one occurrence moves it only when it's that one; a one-off's words would overclaim.
describe("EventEditor, a linked milestone", () => {
  const launch: Milestone = {
    id: 7,
    project_name: "Atlas",
    label: "Launch",
    due_date: "2026-10-19",
    event_uid: "abc@google.com",
    calendar_linked: true,
    event_missing: false,
    state: null,
    status: null,
    source_type: null,
    external_id: null,
    sort_order: 0,
  };
  const editor = (clicked: CalendarEvent) => (
    <EventEditor
      row={clicked}
      calendar={null}
      account="me@x.com"
      suggestions={NO_SUGGESTIONS}
      milestone={launch}
      colorOf={colourOf}
      {...NO_CALENDARS}
      onClose={() => {}}
    />
  );

  it("a repeating event's milestone takes its date from the next occurrence", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    render(editor({ ...row, series_id: "abc", recurring: true }));
    expect(
      await screen.findByText(
        "Linked to the “Launch” milestone in Atlas, which takes its date from the next occurrence.",
      ),
    ).toBeTruthy();
    cleanup();
    render(editor(row));
    expect(
      await screen.findByText(
        "Linked to the “Launch” milestone in Atlas: moving this event moves the milestone too.",
      ),
    ).toBeTruthy();
  });
});

// Guests (#884, C12c): the list in the editor, and Send / Don't send with no default. A one-off asks
// from the editor's own count before anything is sent; the backend's numbers ask again; Busy/free
// alone never asks; a repeating event's rows ask in the choices dialog.
describe("EventEditor, guests", () => {
  const me: GuestForEdit = {
    email: "me@x.com",
    name: null,
    response: "accepted",
    optional: false,
    organizer: true,
    self: true,
    room: false,
  };
  const ann: GuestForEdit = {
    email: "ann@x.org",
    name: "Ann",
    response: "accepted",
    optional: false,
    organizer: false,
    self: false,
    room: false,
  };
  const withAnn = () => event({ guests: [me, ann] });
  /** Ann, told of a change. */
  const ONE: GuestNotice = {
    updated: 1,
    invited: 0,
    removed: 0,
    told_cancelled: 0,
    cancelled: false,
    people: 1,
    emails: 1,
    complete: true,
  };
  const question = () => screen.queryByRole("dialog", { name: "Send update emails to guests?" });

  /** Open the one-off with Ann, retitle it, press Save. */
  async function retitle() {
    openForEdit.mockResolvedValue(ready(withAnn()));
    const onClose = await open();
    const editorSave = screen.getByRole("button", { name: "Save" });
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    const prompt = await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    return { onClose, editorSave, prompt };
  }

  it("a one-off with guests asks before saving, and Back returns to Save", async () => {
    const { onClose, editorSave, prompt } = await retitle();
    const p = within(prompt);
    expect(p.getByText("1 guest already invited will get an update.")).toBeTruthy();
    expect(p.getByText("Google sends the emails from me@x.com.")).toBeTruthy();
    // No default: Save waits for a pick, and focus starts on Back.
    expect(p.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(p.getByRole<HTMLInputElement>("radio", { name: /^Send/ }).checked).toBe(false);
    expect(document.activeElement).toBe(p.getByRole("button", { name: "Back" }));
    expect(saveEdit).not.toHaveBeenCalled();
    fireEvent.click(p.getByRole("button", { name: "Back" }));
    expect(question()).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Gym");
    expect(saveEdit).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("Send saves with all, Don't send with none, each for the numbers it showed", async () => {
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    for (const [label, notify] of [
      [/^Send/, "all"],
      [/^Don't send/, "none"],
    ] as const) {
      const { onClose, prompt } = await retitle();
      fireEvent.click(within(prompt).getByRole("radio", { name: label }));
      fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
      await waitFor(() => expect(onClose).toHaveBeenCalled());
      expect(saveEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" }, null, "Gym", notify, ONE);
      cleanup();
    }
  });

  // Nothing goes from behind the question (C12 critique C2-L3): the editor's own Save, Cancel and
  // Delete do nothing while it's open, whatever reaches them.
  it("holds the editor's own buttons while the email question is open", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    const onDelete = vi.fn();
    const onClose = await open(vi.fn(), onDelete);
    const [editorSave, cancel, del] = ["Save", "Cancel", "Delete"].map((name) =>
      screen.getByRole("button", { name }),
    );
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(editorSave);
    await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    for (const button of [editorSave, cancel, del]) fireEvent.click(button);
    expect(screen.getAllByRole("dialog", { name: "Send update emails to guests?" })).toHaveLength(
      1,
    );
    expect(screen.queryByRole("dialog", { name: /^Discard your changes/ })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("Escape closes only the email question", async () => {
    const { onClose, prompt } = await retitle();
    fireEvent.keyDown(prompt, { key: "Escape" });
    expect(question()).toBeNull();
    expect(screen.queryByRole("dialog", { name: "Discard your changes?" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "Edit event" })).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("a Busy/free change with guests saves without asking", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Free" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    expect(saveEdit).toHaveBeenCalledWith("s1", { show_as: "free" }, ...UNSCOPED);
  });

  // Notifications and colour are the user's own (C13): Google emails no guest about them, so PM
  // never asks, as the backend's `PERSONAL` has it.
  it("a notification and colour change with guests saves without asking", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Add notification" }));
    fireEvent.click(screen.getByRole("button", { name: "Colour: Calendar colour" }));
    fireEvent.click(await screen.findByRole("option", { name: "Basil" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      {
        reminders: { use_default: false, overrides: [{ method: "popup", minutes: 30 }] },
        colour: { kind: "colour", id: "10" },
      },
      ...UNSCOPED,
    );
  });

  it("the backend's guests question opens the prompt with its numbers and the again line", async () => {
    const two: GuestNotice = { ...ONE, updated: 2, people: 2, emails: 2 };
    saveEdit.mockResolvedValueOnce({
      outcome: "needs_choice",
      scope: false,
      guests: two,
      again: true,
    });
    const { onClose, editorSave, prompt } = await retitle();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    const again = await screen.findByText("2 guests already invited will get an update.");
    const reasked = within(again.closest('[role="dialog"]') as HTMLElement);
    expect(
      reasked.getByText(
        "PM checked with Google before saving, and these are the numbers it found.",
      ),
    ).toBeTruthy();
    // Asked afresh: nothing picked.
    expect(reasked.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    // A Save from behind it never swaps the backend's numbers for the editor's own count.
    fireEvent.click(editorSave);
    expect(screen.getByText("2 guests already invited will get an update.")).toBeTruthy();
    expect(screen.queryByText("1 guest already invited will get an update.")).toBeNull();
    saveEdit.mockResolvedValueOnce({ outcome: "saved", warnings: [] });
    fireEvent.click(reasked.getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(reasked.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" }, null, "Gym", "none", two);
  });

  it("a second one in one Save is a banner", async () => {
    const two: GuestNotice = { ...ONE, updated: 2, people: 2, emails: 2 };
    saveEdit.mockResolvedValue({ outcome: "needs_choice", scope: false, guests: two, again: true });
    const { onClose, editorSave, prompt } = await retitle();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    const again = await screen.findByText("2 guests already invited will get an update.");
    const reasked = within(again.closest('[role="dialog"]') as HTMLElement);
    fireEvent.click(reasked.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(reasked.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText("Choose whether Google emails the guests. Nothing was saved."),
    ).toBeTruthy();
    expect(screen.queryByText("2 guests already invited will get an update.")).toBeNull();
    expect(saveEdit).toHaveBeenCalledTimes(2);
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => expect(document.activeElement).toBe(editorSave));
  });

  it("a repeating save passes the dialog's answer", async () => {
    const occurrence: CalendarEvent = { ...row, series_id: "abc", recurring: true };
    openForEdit.mockResolvedValue(ready(withAnn()));
    choicesForEdit.mockResolvedValue({
      outcome: "ready",
      this: { row: "offered", warnings: [], token: "t-this", guests: ONE },
      following: { row: "offered", warnings: [], token: "t-following", guests: ONE },
      all: { row: "offered", warnings: [], token: "t-all", guests: ONE },
    });
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const d = within(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    await d.findByRole("radio", { name: /^This event/ });
    // Asked inside the dialog, never in a second one.
    expect(question()).toBeNull();
    fireEvent.click(d.getByRole("radio", { name: /^Send/ }));
    fireEvent.click(d.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { summary: "Gym" },
      { scope: "this", token: "t-this", asAll: false },
      "Gym",
      "all",
      null,
    );
  });

  it("the guest field is disabled with the reason when guests can't change", async () => {
    const locked: FieldPermissions = {
      ...ALL,
      summary: false,
      time: false,
      location: false,
      description: false,
      guests: false,
      reasons: ["locked"],
    };
    openForEdit.mockResolvedValue(ready(withAnn(), locked));
    await open();
    expect(screen.getByRole<HTMLInputElement>("combobox", { name: "Add guests" }).disabled).toBe(
      true,
    );
    expect(screen.getByText(/Google has locked this event's/)).toBeTruthy();
    // The list still shows.
    expect(screen.getByText("Ann")).toBeTruthy();
  });

  // A guest added is invited, and the guest already there hears of the change, as Google counts it.
  it("adding a guest asks with an invitation, and sends the change to Google's list", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "bo@x.org" } });
    fireEvent.keyDown(box, { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    expect(within(prompt).getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    expect(within(prompt).getByText("1 guest already invited will get an update.")).toBeTruthy();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { guests: [{ email: "bo@x.org", to: "required" }] },
      null,
      "Dentist",
      "all",
      { ...ONE, invited: 1, people: 2, emails: 2 },
    );
  });

  it("a rejected address stops the save", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    await open();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "bob at x" } });
    fireEvent.blur(box);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getAllByText("“bob at x” isn't an email address.").length).toBeGreaterThan(0);
    expect(question()).toBeNull();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // C12c review: like a date it couldn't read, an address it couldn't read never reached the draft,
  // so closing would lose it without a word: the editor asks first.
  it("asks before closing when only an address it couldn't read stands", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    const onClose = await open();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    box.focus();
    fireEvent.change(box, { target: { value: "bob at x" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // The field takes Google's list from the editor: a guest taken off and added back is Google's
  // entry again, reply and all, so there's nothing to save and nobody to ask about.
  it("a guest taken off and added back is Google's entry, so nothing is sent", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Remove Ann" }));
    expect(screen.queryByText("Ann")).toBeNull();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "ann@x.org" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(screen.getByText("Ann")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(question()).toBeNull();
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // And the organiser's address from Google's copy, on the list or not: Google refuses any change
  // to it, so the field never adds it.
  it("never adds the organiser, who isn't on Google's list", async () => {
    openForEdit.mockResolvedValue(ready(event({ guests: [ann], organizer_email: "boss@x.org" })));
    const onClose = await open();
    const box = screen.getByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "Boss@x.org" } });
    fireEvent.keyDown(box, { key: "Enter" });
    expect(screen.getByText("That's the organiser, who is on it already.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Remove Boss/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // C12 critique C1-H2: a delete from the editor asks about the guests Google's copy has, not the
  // mirror's (possibly older) list.
  it("deletes with the guest list Google's copy has", async () => {
    openForEdit.mockResolvedValue(ready(withAnn()));
    const onDelete = vi.fn();
    await open(vi.fn(), onDelete);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onDelete.mock.calls[0][0].attendees).toEqual([
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
    ]);
  });
});

// Notifications and colour (#884, C13): the whole list goes when it changed, never an untouched one;
// a colour is one choice; a row that can't be committed stops the save, and one typed but not
// committed is sent; the kind switch puts an own list back to the defaults and says so.
describe("EventEditor, notifications and colour", () => {
  const work: Calendar = {
    id: row.calendar_id,
    source_id: "gcal:me@x.com",
    provider: "google",
    remote_id: "me@x.com",
    name: "Work",
    color: null,
    selected: true,
    is_primary: true,
    quiet: false,
    kind: null,
    access_role: "owner",
    time_zone: "Europe/London",
    default_reminders: [{ method: "popup", minutes: 30 }],
    conference_types: [],
    data_owner: null,
  };
  const own = (...list: [string, number][]) => ({
    use_default: false,
    overrides: list.map(([method, minutes]) => ({ method, minutes })),
  });
  const saved: SaveAnswer = { outcome: "saved", warnings: [] };
  const count = (words: string) =>
    screen.getByRole<HTMLInputElement>("textbox", { name: `How many: ${words}` });

  it("saves a notification change as the whole list", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10], ["email", 60]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    const ten = count("10 minutes before");
    fireEvent.change(ten, { target: { value: "15" } });
    fireEvent.blur(ten);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { reminders: own(["popup", 15], ["email", 60]) },
      ...UNSCOPED,
    );
  });

  it("sends a number typed but not yet committed when Save is pressed (C13 critique C2-M5)", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    const ten = count("10 minutes before");
    ten.focus();
    fireEvent.change(ten, { target: { value: "45" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { reminders: own(["popup", 45]) }, ...UNSCOPED);
  });

  it("shows a timed event's defaults as rows that send nothing until one changes", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    expect(screen.getByText("Work's default notifications:")).toBeTruthy();
    // Retitled only: nothing about notifications goes.
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { summary: "Gym" }, null, "Gym", null, null);
    cleanup();
    // The first change to a default row makes the list the event's own, the rest as they were.
    const again = await open(vi.fn(), undefined, row, work);
    fireEvent.change(screen.getByRole("combobox", { name: "How: 30 minutes before" }), {
      target: { value: "email" },
    });
    expect(screen.queryByText("Work's default notifications:")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(again).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { reminders: own(["email", 30]) }, ...UNSCOPED);
  });

  it("a row it can't commit stops the save and asks before closing", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    const ten = count("10 minutes before");
    fireEvent.change(ten, { target: { value: "ten" } });
    fireEvent.blur(ten);
    expect(screen.getByText("Use a whole number.")).toBeTruthy();
    expect(ten.getAttribute("aria-invalid")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(saveEdit).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    // The draft never took the text, so only the refusal says something would be lost.
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    // Fixed, it saves.
    fireEvent.change(ten, { target: { value: "20" } });
    fireEvent.blur(ten);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { reminders: own(["popup", 20]) }, ...UNSCOPED);
  });

  it("saves a colour as one choice, and the calendar's colour as the calendar's", async () => {
    openForEdit.mockResolvedValue(ready(event({ color_id: "5" })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Colour: Banana" }));
    fireEvent.click(await screen.findByRole("option", { name: "Calendar colour" }));
    expect(screen.getByRole("button", { name: "Colour: Calendar colour" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { colour: { kind: "calendar" } }, ...UNSCOPED);
  });

  it("a conflict on the colour, or a label Google added, names the colour", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event({ color_id: "5" })))
      .mockResolvedValueOnce(ready(event({ color_id: "3" }), ALL, "s2"));
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["colorId", "eventLabelId"] });
    await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Colour: Banana" }));
    fireEvent.click(await screen.findByRole("option", { name: "Sage" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Google's version of the colour is shown now/);
    expect(screen.getByRole("button", { name: "Colour: Grape" })).toBeTruthy();
  });

  it("the colour list sits inside the editor, and Escape closes only it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open(vi.fn(), undefined, row, work);
    const trigger = screen.getByRole("button", { name: "Colour: Calendar colour" });
    fireEvent.click(trigger);
    const list = screen.getByRole("listbox", { name: "Colour" });
    // In the editor's dialog, so its focus trap and screen readers reach it.
    expect(screen.getByRole("dialog", { name: "Edit event" }).contains(list)).toBe(true);
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(list).getByRole("option", { name: "Calendar colour" }),
      ),
    );
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(screen.getByRole("dialog", { name: "Edit event" })).toBeTruthy();
    expect(screen.queryByText("Discard your changes?")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
  });

  it("switching to all day puts the event's own list back to the defaults and says so", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to Work's defaults for all-day events."),
    ).toBeTruthy();
    expect(screen.getByText("Work's default notifications for all-day events.")).toBeTruthy();
    // Back to timed, as Google holds it: the event's own list returns, and nothing says otherwise
    // (C13 critique C2-L5).
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    expect(count("10 minutes before")).toBeTruthy();
    // All day again; said until the list next changes.
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    // Back to timed over the list just made: the event's own replaces it, and that is said (C13
    // review round 1).
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to the ones this event has in Google."),
    ).toBeTruthy();
    expect(count("10 minutes before")).toBeTruthy();
    // All day again, and saved: the defaults go with the new time.
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit.mock.calls[0][1].reminders).toEqual({ use_default: true, overrides: [] });
    expect(saveEdit.mock.calls[0][1].time?.kind).toBe("all_day");
  });

  // C13 review round 1: a conflict that puts Google's timed time back over the switch to all day
  // puts back the notifications that switch replaced, as switching back would, and the switch's
  // line goes; the next save sends none of them.
  it("a conflict that undoes the switch to all day puts the event's own notifications back", async () => {
    const list = own(["popup", 10], ["email", 60]);
    const moved = event({
      reminders: list,
      time: {
        kind: "timed",
        start_date: "2026-10-12",
        start_time: "15:00",
        start_zone: "Europe/London",
        end_date: "2026-10-12",
        end_time: "16:00",
        end_zone: "Europe/London",
      },
      start_at: "2026-10-12T14:00:00Z",
      end_at: "2026-10-12T15:00:00Z",
    });
    openForEdit
      .mockResolvedValueOnce(ready(event({ reminders: list })))
      .mockResolvedValueOnce(ready(moved, ALL, "s2"));
    saveEdit
      .mockResolvedValueOnce({ outcome: "conflict", fields: ["start", "end"] })
      .mockResolvedValueOnce(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to Work's defaults for all-day events."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/changed in Google while you were editing/);
    expect(saveEdit.mock.calls[0][1].reminders).toEqual({ use_default: true, overrides: [] });
    // Google's time, with the list that goes with it, as Google holds both.
    expect(count("10 minutes before")).toBeTruthy();
    expect(count("1 hour before, by email")).toBeTruthy();
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit.mock.calls[1][1]).toEqual({ summary: "Gym" });
  });

  // C13 review round 2: the defaults pressed for are the user's, on either kind; a switch there and
  // back put nothing there, so it takes nothing away.
  it("switching to all day and back keeps the defaults I picked", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Use Work's defaults" }));
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    expect(screen.getByText("Work's default notifications:")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { reminders: { use_default: true, overrides: [] } },
      ...UNSCOPED,
    );
  });

  // C13 review round 2: once the list has changed, the defaults on it are the user's, even if the
  // switch had put them there first.
  it("defaults picked again after the switch stay when switching back", async () => {
    openForEdit.mockResolvedValue(ready(event({ reminders: own(["popup", 10]) })));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    fireEvent.click(screen.getByRole("button", { name: "Use Work's defaults" }));
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.getByText("Work's default notifications:")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { reminders: { use_default: true, overrides: [] } },
      ...UNSCOPED,
    );
  });

  // C13 review round 2: a conflict that puts Google's timed time back over the switch keeps the
  // defaults pressed for before it, since the switch never put them there.
  it("a conflict that undoes the switch to all day keeps the defaults I picked", async () => {
    const moved = event({
      reminders: own(["popup", 10]),
      time: {
        kind: "timed",
        start_date: "2026-10-12",
        start_time: "15:00",
        start_zone: "Europe/London",
        end_date: "2026-10-12",
        end_time: "16:00",
        end_zone: "Europe/London",
      },
      start_at: "2026-10-12T14:00:00Z",
      end_at: "2026-10-12T15:00:00Z",
    });
    openForEdit
      .mockResolvedValueOnce(ready(event({ reminders: own(["popup", 10]) })))
      .mockResolvedValueOnce(ready(moved, ALL, "s2"));
    saveEdit
      .mockResolvedValueOnce({ outcome: "conflict", fields: ["start", "end"] })
      .mockResolvedValueOnce(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Use Work's defaults" }));
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/changed in Google while you were editing/);
    expect(screen.getByText("Work's default notifications:")).toBeTruthy();
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit.mock.calls[1][1]).toEqual({ reminders: { use_default: true, overrides: [] } });
  });

  // C13 review round 1: Google's list put in by a conflict is no longer the defaults the switch's
  // line names, so the line goes.
  it("a conflict that replaces the notifications drops the switch's line", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event({ reminders: own(["popup", 10]) })))
      .mockResolvedValueOnce(ready(event({ reminders: own(["popup", 30]) }), ALL, "s2"));
    saveEdit.mockResolvedValueOnce({ outcome: "conflict", fields: ["reminders"] });
    await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(
      screen.getByText("Notifications went back to Work's defaults for all-day events."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/changed in Google while you were editing/);
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
  });

  it("the kind switch leaves notifications it can't change as they are (C13 critique C1-L14)", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ reminders: own(["popup", 10]) }), { ...ALL, reminders: false }),
    );
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    // Off, as the permission says.
    expect(count("10 minutes before").disabled).toBe(true);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.queryByText(/Notifications went back/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(Object.keys(saveEdit.mock.calls[0][1])).toEqual(["time"]);
  });
});

// Invitations (#884, C14c): the reply on top and apart from the fields, the user's own settings
// live, the shared fields only where the organiser lets guests change them (with Google's warning),
// how it repeats never, and an email question for a shared change that counts the organiser.
describe("EventEditor, an invitation", () => {
  const work: Calendar = {
    id: row.calendar_id,
    source_id: "gcal:me@x.com",
    provider: "google",
    remote_id: "me@x.com",
    name: "Work",
    color: null,
    selected: true,
    is_primary: true,
    quiet: false,
    kind: null,
    access_role: "owner",
    time_zone: "Europe/London",
    default_reminders: [{ method: "popup", minutes: 30 }],
    conference_types: [],
    data_owner: null,
  };
  const guest = (email: string, over: Partial<GuestForEdit> = {}): GuestForEdit => ({
    email,
    name: null,
    response: "accepted",
    optional: false,
    organizer: false,
    self: false,
    room: false,
    ...over,
  });
  const boss = guest("boss@x.org", { organizer: true });
  const mine = guest("me@x.com", { self: true, response: "needsAction" });
  const ann = guest("ann@x.org");
  /** Someone else's event with the calendar, the organiser and Ann on its list. */
  const invitation = (over: Partial<EventForEdit> = {}) =>
    event({
      guests: [boss, mine, ann],
      organizer_email: "boss@x.org",
      organizer_self: false,
      reply: { response: "needsAction", note: "" },
      ...over,
    });
  /** The backend's allow-list without "modify event" (gate.rs row A2). */
  const THEIRS: FieldPermissions = {
    ...ALL,
    summary: false,
    time: false,
    repeat: false,
    location: false,
    description: false,
    visibility: false,
    guests: false,
    delete: true,
    respond: true,
    reasons: ["not_organizer"],
  };
  /** With "modify event" (row A3). */
  const MODIFY: FieldPermissions = {
    ...ALL,
    repeat: false,
    guests: false,
    delete: true,
    respond: true,
    reasons: ["organizer_only_guests"],
  };
  const saved: SaveAnswer = { outcome: "saved", warnings: [] };
  const theirsLine =
    "Someone else organises this event, so only they can change its title, time, place, description, visibility, guests, video call and calendar.";
  /** What the guest list says of the user's own entry. */
  const myEntry = () =>
    within(screen.getByRole("list", { name: "Guests" }))
      .getByText("me@x.com")
      .closest("li")!;

  it("opens with the reply on top and the shared fields greyed, saying why", async () => {
    openForEdit.mockResolvedValue(ready(invitation(), THEIRS));
    await open(vi.fn(), vi.fn(), row, work);
    const yes = screen.getByRole("button", { name: "Yes" });
    const title = screen.getByLabelText<HTMLInputElement>("Title");
    // Above the fields, as on Google's event page.
    expect(yes.compareDocumentPosition(title) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByText("Going?")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add a note" })).toBeTruthy();
    expect(title.disabled).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByLabelText<HTMLInputElement>("Location").disabled).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / }).disabled).toBe(
      true,
    );
    // The user's own settings stay live.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Free" }).disabled).toBe(false);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Add notification" }).disabled,
    ).toBe(false);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Colour: Calendar colour" }).disabled,
    ).toBe(false);
    // Why, with the reply among what's the user's, since this copy can send one.
    expect(screen.getByText(`${theirsLine} ${YOURS_WITH_REPLY}`)).toBeTruthy();
    expect(
      screen.getByText("Someone else organises this event, so only they can make it repeat."),
    ).toBeTruthy();
    // No Google warning without "modify event"; Delete is Remove here.
    expect(screen.queryByText(/lets guests change this event/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.getByRole("button", { name: "Remove" })).toBeTruthy();
  });

  // C14 §10.4: Remove from the editor hands on the row with who organises it as Google's copy says,
  // so the question and the send go by what the footer said; unsaved changes are asked about first,
  // in Remove's words.
  it("removes what the editor showed, asking first when there are unsaved changes", async () => {
    openForEdit.mockResolvedValue(ready(invitation(), { ...THEIRS, summary: true }));
    const onDelete = vi.fn();
    await open(vi.fn(), onDelete, { ...row, organizer_self: true }, work);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Changed" } });
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Discard your changes and remove?")).toBeTruthy();
    expect(onDelete).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard and remove" }));
    expect(onDelete).toHaveBeenCalledWith(expect.objectContaining({ organizer_self: false }));
  });

  it("a locked invitation or one not on its guest list offers no Remove", async () => {
    openForEdit.mockResolvedValue(
      ready(invitation(), { ...THEIRS, delete: false, reasons: ["not_organizer", "locked"] }),
    );
    await open(vi.fn(), vi.fn(), row, work);
    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("says the reply is the user's only where this copy can send one", async () => {
    openForEdit.mockResolvedValue(
      ready(invitation({ reply: null }), { ...THEIRS, respond: false }),
    );
    await open(vi.fn(), undefined, row, work);
    expect(screen.queryByRole("button", { name: "Yes" })).toBeNull();
    expect(
      screen.getByText(
        `${theirsLine} Your notifications, colour and busy or free are yours to change.`,
      ),
    ).toBeTruthy();
  });

  it("a reply doesn't make the draft dirty, and Save doesn't send it", async () => {
    openForEdit.mockResolvedValue(ready(invitation(), THEIRS));
    respond.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    await screen.findByText("You replied Yes.");
    expect(respond).toHaveBeenCalledWith(row.id, "yes", null, null);
    // Nothing to save, so Save closes, sending nothing, and asking nothing.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(onClose).toHaveBeenCalled();
    expect(saveEdit).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: /^Discard your changes/ })).toBeNull();
  });

  // A reply goes at once, apart from the draft: the discard question never says nothing went.
  describe("the discard question after a reply", () => {
    const NOTHING = "Nothing has been saved to Google yet.";
    const WITH_GOOGLE = "Your reply is already with Google. Only your other changes are discarded.";
    const UNKNOWN =
      "PM couldn't confirm whether your reply reached Google. Refresh the calendar to check.";
    const after = async (outcome: WriteOutcome | null) => {
      openForEdit.mockResolvedValue(ready(invitation(), { ...THEIRS, summary: true }));
      await open(vi.fn(), vi.fn(), row, work);
      if (outcome) {
        respond.mockResolvedValue(outcome);
        fireEvent.click(screen.getByRole("button", { name: "Yes" }));
        await waitFor(() => expect(respond).toHaveBeenCalled());
        await screen.findByRole("button", { name: "Yes" });
      }
      fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Changed" } });
      fireEvent.click(screen.getByRole("button", { name: "Remove" }));
      await screen.findByText("Discard your changes and remove?");
    };

    it("says nothing has gone when no reply did", async () => {
      await after(null);
      expect(screen.getByText(NOTHING)).toBeTruthy();
    });

    it("says the reply is with Google once it is", async () => {
      await after(saved);
      expect(await screen.findByText(WITH_GOOGLE)).toBeTruthy();
      expect(screen.queryByText(NOTHING)).toBeNull();
    });

    it("says PM can't tell when the reply came back unconfirmed", async () => {
      await after({ outcome: "unconfirmed" });
      expect(await screen.findByText(UNKNOWN)).toBeTruthy();
      expect(screen.queryByText(NOTHING)).toBeNull();
    });

    it("a refused reply sent nothing", async () => {
      await after({ outcome: "busy" });
      expect(await screen.findByText(NOTHING)).toBeTruthy();
    });
  });

  it("after a reply the guest list shows it, and Yes stays pressed", async () => {
    openForEdit.mockResolvedValue(ready(invitation(), THEIRS));
    respond.mockResolvedValue(saved);
    await open(vi.fn(), undefined, row, work);
    expect(within(myEntry()).getByText("no reply")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    await screen.findByText("You replied Yes.");
    expect(within(myEntry()).getByText("accepted")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Yes" }).getAttribute("aria-pressed")).toBe("true");
    // Nobody else's entry changes.
    const annRow = within(screen.getByRole("list", { name: "Guests" }))
      .getByText("ann@x.org")
      .closest("li")!;
    expect(within(annRow).getByText("accepted")).toBeTruthy();
  });

  it("sends the note with the reply shown", async () => {
    openForEdit.mockResolvedValue(
      ready(
        invitation({
          guests: [boss, { ...mine, response: "accepted" }, ann],
          reply: { response: "accepted", note: "late" },
        }),
        THEIRS,
      ),
    );
    respond.mockResolvedValue(saved);
    await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Change note" }));
    const note = screen.getByLabelText<HTMLTextAreaElement>("Note");
    expect(note.value).toBe("late");
    fireEvent.change(note, { target: { value: "running late " } });
    fireEvent.click(screen.getByRole("button", { name: "Send note" }));
    await screen.findByText("You replied Yes.");
    expect(respond).toHaveBeenCalledWith(row.id, "yes", "running late ", null);
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("an occurrence that kept its own answer for all events still shows its own", async () => {
    const occurrence: CalendarEvent = { ...row, series_id: "abc", recurring: true };
    const declined = () =>
      invitation({
        guests: [boss, { ...mine, response: "declined" }, ann],
        reply: { response: "declined", note: "" },
      });
    // "Going?" too (PR B review round 1): the occurrence that kept No has No pressed, never the
    // reply sent.
    for (const [left, shown, going] of [
      [["2026-10-12"], "declined", "No"],
      [["2026-10-19"], "accepted", "Yes"],
    ] as const) {
      openForEdit.mockResolvedValue(ready(declined(), THEIRS));
      respond.mockResolvedValue({
        outcome: "saved",
        warnings: ["kept_own_reply"],
        left_behind: [...left],
      });
      await open(vi.fn(), undefined, occurrence, work);
      fireEvent.click(screen.getByRole("button", { name: "Yes" }));
      fireEvent.click(screen.getByRole("button", { name: "All events" }));
      await screen.findByText(/You replied Yes for all events\./);
      expect(respond).toHaveBeenLastCalledWith(occurrence.id, "yes", null, "all");
      expect(within(myEntry()).getByText(shown)).toBeTruthy();
      expect(
        ["Yes", "No", "Maybe"].filter(
          (name) => screen.getByRole("button", { name }).getAttribute("aria-pressed") === "true",
        ),
      ).toEqual([going]);
      cleanup();
    }
  });

  // PR B review round 2: an all-events reply PM couldn't read back yet may not have reached this
  // occurrence, answered No on its own, so its own answer stays; to this occurrence alone, it shows.
  it("an all-events reply not read back yet leaves the occurrence's own answer", async () => {
    const occurrence: CalendarEvent = { ...row, series_id: "abc", recurring: true };
    const declined = () =>
      invitation({
        guests: [boss, { ...mine, response: "declined" }, ann],
        reply: { response: "declined", note: "" },
      });
    for (const [scope, which, shown, going] of [
      ["all", "All events", "declined", "No"],
      ["this", "This event", "accepted", "Yes"],
    ] as const) {
      openForEdit.mockResolvedValue(ready(declined(), THEIRS));
      respond.mockResolvedValue({
        outcome: "saved",
        warnings: ["mirror_refresh_pending"],
        left_behind: [],
      });
      await open(vi.fn(), undefined, occurrence, work);
      fireEvent.click(screen.getByRole("button", { name: "Yes" }));
      fireEvent.click(screen.getByRole("button", { name: which }));
      await screen.findByText(/^You replied Yes.*after the next refresh\.$/);
      expect(respond).toHaveBeenLastCalledWith(occurrence.id, "yes", null, scope);
      expect(within(myEntry()).getByText(shown)).toBeTruthy();
      expect(
        ["Yes", "No", "Maybe"].filter(
          (name) => screen.getByRole("button", { name }).getAttribute("aria-pressed") === "true",
        ),
      ).toEqual([going]);
      cleanup();
    }
  });

  it("where the organiser lets guests change it, says Google's warning and its fields are live", async () => {
    openForEdit.mockResolvedValue(ready(invitation({ guests_can_modify: true }), MODIFY));
    await open(vi.fn(), vi.fn(), row, work);
    expect(screen.getByText(A11_NOTE)).toBeTruthy();
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(false);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(false);
    // How it repeats, and who is invited, stay the organiser's (C14 critiques C1-H2, C2-H2; Q4).
    expect(screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / }).disabled).toBe(
      true,
    );
    expect(
      screen.getByText("Someone else organises this event, so only they can make it repeat."),
    ).toBeTruthy();
    expect(
      screen.getByText("In PM, only the organiser can change who's invited and the video call."),
    ).toBeTruthy();
    expect(screen.queryByText(theirsLine, { exact: false })).toBeNull();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("your own event that lets guests change it has no warning, and its Repeat stays yours", async () => {
    openForEdit.mockResolvedValue(ready(event({ guests_can_modify: true })));
    await open(vi.fn(), undefined, row, work);
    expect(screen.queryByText(A11_NOTE)).toBeNull();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / }).disabled).toBe(
      false,
    );
    expect(screen.queryByText(/so only they can make it repeat/)).toBeNull();
    cleanup();
    // The row reads its own right, never the time's (C14 critiques C1-H2, C2-H2).
    openForEdit.mockResolvedValue(ready(event(), { ...ALL, repeat: false }));
    await open(vi.fn(), undefined, row, work);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / }).disabled).toBe(
      true,
    );
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(false);
  });

  it("busy or free on an invitation saves with no email question", async () => {
    openForEdit.mockResolvedValue(ready(invitation(), THEIRS));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.click(screen.getByRole("button", { name: "Free" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(screen.queryByRole("dialog", { name: "Send update emails to guests?" })).toBeNull();
    expect(saveEdit).toHaveBeenCalledWith("s1", { show_as: "free" }, ...UNSCOPED);
  });

  it("a title change where guests may change it asks Send / Don't send, the organiser counted", async () => {
    openForEdit.mockResolvedValue(ready(invitation({ guests_can_modify: true }), MODIFY));
    saveEdit.mockResolvedValue(saved);
    const onClose = await open(vi.fn(), undefined, row, work);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    expect(within(prompt).getByText("2 guests already invited will get an update.")).toBeTruthy();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { summary: "Retro" }, null, "Retro", "none", {
      updated: 2,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: false,
      people: 2,
      emails: 2,
      complete: true,
    });
  });
});

// C18d (§10): the Calendar row, on an event you organise on a calendar you own: the other
// calendars you own on its account, sent with the rest in one Save.
describe("EventEditor, its calendar", () => {
  const ME = "gcal:me@x.com";
  const WORK = "gcal:work@x.com";
  const account = (id: string): CalendarAccount => {
    const email = id.slice(id.indexOf(":") + 1);
    return { id, provider: "google", email, label: email, state: "ok", last_synced_at: null };
  };
  const cal = (source: string, remote: string, over: Partial<Calendar> = {}): Calendar => ({
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
  });
  // The row's own calendar is Personal (`row.calendar_id`).
  const PERSONAL = cal(ME, "me@x.com", { name: "Personal", is_primary: true });
  const SIDE = cal(ME, "side", { name: "Side" });
  const QUIET = cal(ME, "quiet", { name: "Quiet" });
  const SHARED = cal(ME, "shared", { name: "Shared", access_role: "writer" });
  const WORK_CAL = cal(WORK, "work@x.com", { name: "Work", is_primary: true });
  const overviewOf = (accounts: string[], calendars: Calendar[]): CalendarOverview => ({
    google_client_configured: true,
    microsoft_client_configured: false,
    accounts: accounts.map(account),
    calendars,
    last_sync: null,
    window_days: 7,
    mirror_start: "2026-09-01T00:00:00Z",
    mirror_end: "2027-11-01T00:00:00Z",
    editing: Object.fromEntries(accounts.map((a) => [a, "on" as const])),
    default_length: {},
    auto_meet: {},
    ask_access: {},
  });
  const BOTH = overviewOf([ME, WORK], [PERSONAL, SIDE, QUIET, SHARED, WORK_CAL]);
  const saved: WriteOutcome = { outcome: "saved", warnings: [], calendar: SIDE.id };

  /** Open `ev` with `permissions` on `on` (Personal unless said), the calendars of `overview`, Quiet
   *  hidden in the calendar list. */
  async function openOn(
    ev = event(),
    permissions = ALL,
    overview = BOTH,
    on: Calendar = PERSONAL,
    onClose = vi.fn(),
    over: Partial<CalendarEvent> = {},
    footer: {
      onDelete?: (row: CalendarEvent) => void;
      onDuplicate?: (row: CalendarEvent) => void;
      canDuplicate?: boolean;
    } = {},
  ) {
    openForEdit.mockResolvedValue(ready(ev, permissions));
    render(
      <EventEditor
        row={{ ...row, calendar_id: on.id, ...over }}
        calendar={on}
        account="me@x.com"
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        targets={createTargets(overview)}
        overview={overview}
        hidden={new Set([QUIET.id])}
        shapeOf={() => undefined}
        onClose={onClose}
        {...footer}
      />,
    );
    await screen.findByLabelText("Title");
    return onClose;
  }

  const trigger = (name: string) =>
    screen.getByRole<HTMLButtonElement>("button", { name: `Calendar: ${name}` });
  /** Pick `to` from the list `from` opens: by its name, a hidden one's mark after it. */
  function pick(from: string, to: string) {
    fireEvent.click(trigger(from));
    const list = screen.getByRole("group", { name: "Calendars this event can go on" });
    const name = new RegExp(`^${to}(, hidden)?$`);
    fireEvent.click(within(list).getByRole("radio", { name }), { detail: 1 });
  }
  const DUPLICATE_WORDS =
    "Only calendars you own on me@x.com are listed. To put it on another account's calendar, duplicate it there, then delete this one.";

  it("an event you organise shows a Calendar row with the calendars it owns on its account, and the Duplicate words", async () => {
    await openOn();
    // The row says where it is: no "Saving to" line beside it.
    expect(screen.queryByText(/^Saving to /)).toBeNull();
    expect(screen.getByText(DUPLICATE_WORDS)).toBeTruthy();
    fireEvent.click(trigger("Personal"));
    const list = within(screen.getByRole("group", { name: "Calendars this event can go on" }));
    expect(list.getAllByRole("radio").map((r) => r.closest("label")?.textContent)).toEqual([
      "Personal",
      "Quiet, hidden",
      "Side",
    ]);
    expect(list.queryByRole("radio", { name: "Shared" })).toBeNull();
    expect(list.queryByRole("radio", { name: "Work" })).toBeNull();
    cleanup();
    // With no other account to take events, no Duplicate sentence.
    await openOn(event(), ALL, overviewOf([ME], [PERSONAL, SIDE]));
    expect(screen.getByText("Only calendars you own on me@x.com are listed.")).toBeTruthy();
    expect(screen.queryByText(/duplicate it there/)).toBeNull();
  });

  it("an invitation, a locked event, a calendar you can only edit, or nowhere else to go keeps the subtitle", async () => {
    const locked: FieldPermissions = { ...ALL, calendar: false, reasons: ["locked"] };
    const cases: [FieldPermissions, CalendarOverview, Calendar, string][] = [
      [locked, BOTH, PERSONAL, "Saving to Personal · me@x.com"],
      [ALL, BOTH, SHARED, "Saving to Shared · me@x.com"],
      [ALL, overviewOf([ME], [PERSONAL]), PERSONAL, "Saving to Personal · me@x.com"],
    ];
    for (const [permissions, overview, on, subtitle] of cases) {
      await openOn(event(), permissions, overview, on);
      expect(screen.queryByRole("button", { name: /^Calendar: / })).toBeNull();
      expect(screen.getByText(subtitle)).toBeTruthy();
      cleanup();
    }
  });

  it("choosing another calendar makes Save send it, alone or with a title", async () => {
    saveEdit.mockResolvedValue(saved);
    const onClose = await openOn();
    pick("Personal", "Side");
    expect(trigger("Side")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s1", { calendar: SIDE.id }, ...UNSCOPED);
    // The notice names where it went, from the backend's answer, and where it was.
    expect(noteSaved).toHaveBeenLastCalledWith(
      "Dentist",
      saved,
      null,
      "2026-10-12",
      false,
      false,
      { rowId: row.id, sent: null },
      { to: "Side", from: "Personal" },
    );
    cleanup();
    const again = await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(again).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s1",
      { summary: "Retro", calendar: SIDE.id },
      null,
      "Retro",
      null,
      null,
    );
    cleanup();
    // Chosen, then put back: nothing to send.
    const back = await openOn();
    pick("Personal", "Side");
    pick("Side", "Personal");
    saveEdit.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(back).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("a calendar chosen counts as a change: closing asks first", async () => {
    await openOn();
    pick("Personal", "Side");
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
  });

  it("a hidden calendar says so", async () => {
    await openOn();
    expect(screen.queryByText(/is hidden in your calendar list/)).toBeNull();
    pick("Personal", "Quiet");
    expect(
      screen.getByText(
        "Quiet is hidden in your calendar list, so the event won't show until you show that calendar again.",
      ),
    ).toBeTruthy();
  });

  it("a guest's event asks once, the calendar change counted", async () => {
    const me: GuestForEdit = {
      email: "me@x.com",
      name: null,
      response: "accepted",
      optional: false,
      organizer: true,
      self: true,
      room: false,
    };
    const ann: GuestForEdit = {
      ...me,
      email: "ann@x.org",
      name: "Ann",
      organizer: false,
      self: false,
    };
    saveEdit.mockResolvedValue(saved);
    const onClose = await openOn(event({ guests: [me, ann] }));
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send update emails to guests?" });
    expect(within(prompt).getByText("1 guest already invited will get an update.")).toBeTruthy();
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s1",
      { summary: "Retro", calendar: SIDE.id },
      null,
      "Retro",
      "all",
      {
        updated: 1,
        invited: 0,
        removed: 0,
        told_cancelled: 0,
        cancelled: false,
        people: 1,
        emails: 2,
        complete: true,
      },
    );
  });

  it("not moved reloads Google's copy, says why, and keeps the calendar chosen", async () => {
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    const onClose = await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    // Google's copy now: the title saved, under a new session.
    openForEdit.mockResolvedValue(ready(event({ summary: "Retro" }), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(
        "Your other changes are saved, but it's still on Personal. Google is busy right now.",
      ),
    ).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(trigger("Side")).toBeTruthy();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Retro");
    // Save tries the calendar again, alone, on the new session.
    saveEdit.mockResolvedValue(saved);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s2",
      { calendar: SIDE.id },
      null,
      "Retro",
      null,
      null,
    );
  });

  // Review round 1 of PR 1: a calendar change whose answer was lost had landed, and the title was
  // changed before Save was pressed again. The title is never dropped: the editor goes on from the
  // row the event has now, on Google's copy there, and the next Save sends the title alone.
  const MOVED = `${SIDE.id}:e1`;
  const movedEarlier: WriteOutcome = {
    outcome: "moved_earlier",
    row_id: MOVED,
    calendar: SIDE.id,
  };
  const STILL_TO_SAVE =
    "It's already on Side: the calendar change landed earlier. Your other changes aren't saved yet: press Save to send them.";

  it("a calendar change that had landed goes on from there, the later changes kept", async () => {
    saveEdit.mockResolvedValueOnce({ outcome: "unconfirmed" });
    const onClose = await openOn();
    pick("Personal", "Side");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/PM lost touch with Google/);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    saveEdit.mockResolvedValueOnce(movedEarlier);
    // Google's copy there, under a new session: the title isn't on it.
    openForEdit.mockResolvedValue(ready(event(), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(STILL_TO_SAVE)).toBeTruthy();
    expect(openForEdit).toHaveBeenLastCalledWith(MOVED);
    expect(onClose).not.toHaveBeenCalled();
    expect(trigger("Side")).toBeTruthy();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Retro");
    // Leaving now loses the title, and the calendar stays changed: said so, not "nothing saved".
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(
      await screen.findByText(
        "The event is on Side already. Only what you’ve changed since is discarded.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText("Nothing has been saved to Google yet.")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    // The title alone, on the new session; no calendar change is left to make.
    saveEdit.mockResolvedValue(saved);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s2",
      { summary: "Retro" },
      null,
      "Retro",
      null,
      null,
    );
    // What it's saved as is the row it has now, so an Undo names that one.
    expect(noteSaved.mock.lastCall?.[6]).toEqual({ rowId: MOVED, sent: null });
  });

  it("a calendar change that had landed says why when the copy there can't be read", async () => {
    saveEdit.mockResolvedValueOnce(movedEarlier);
    await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockRejectedValueOnce(new Error("That event is no longer in PM's calendar."));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText(
        "It's already on Side: the calendar change landed earlier. PM couldn't read it there, so your other changes aren't saved. Close this and open the event again to make them.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/no longer in PM's calendar/)).toBeNull();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Retro");
    // Those words say to open it again: its session ended, and nothing more goes from here.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
  });

  it("a calendar change that had landed says nothing's left when Google's copy holds it all", async () => {
    saveEdit.mockResolvedValueOnce(movedEarlier);
    const onClose = await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockResolvedValue(ready(event({ summary: "Retro" }), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(
      await screen.findByText("It's already on Side: the calendar change landed earlier."),
    ).toBeTruthy();
    expect(screen.queryByText(STILL_TO_SAVE)).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });

  // The choices of a repeating event find the same: there's nothing left to choose.
  it("the choices finding a calendar change that had landed go on from there too", async () => {
    choicesForEdit.mockResolvedValue({ outcome: "refused", result: movedEarlier });
    const repeats = { series_id: "abc", recurring: true };
    const onClose = await openOn(event(), ALL, BOTH, PERSONAL, vi.fn(), repeats);
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockResolvedValue(ready(event(), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(STILL_TO_SAVE)).toBeTruthy();
    expect(openForEdit).toHaveBeenLastCalledWith(MOVED);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Retro");
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("the choices finding all of a save landed close on it, saying where it is", async () => {
    const landed: WriteOutcome = { outcome: "saved", warnings: [], calendar: SIDE.id };
    choicesForEdit.mockResolvedValue({ outcome: "refused", result: landed });
    const repeats = { series_id: "abc", recurring: true };
    const onClose = await openOn(event(), ALL, BOTH, PERSONAL, vi.fn(), repeats);
    pick("Personal", "Side");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(noteSaved.mock.lastCall?.[1]).toEqual(landed);
    expect(noteSaved.mock.lastCall?.[7]).toEqual({ to: "Side", from: "Personal" });
    expect(saveEdit).not.toHaveBeenCalled();
  });

  // Review round 1 of PR 1: the fields landed, so a read after that fails never takes the place of
  // saying so (the read's own words can say nothing changed, or that the event is gone when the
  // save gave its occurrences new ids), and what the fields had to say beside "saved" is said too.
  it("not moved still says what saved when Google's copy can't be read again", async () => {
    const head =
      "Your other changes are saved, but it's still on Personal. Google is busy right now. Google didn't make the video call. Add it again from Edit, or in Google Calendar.";
    const reads: (() => void)[] = [
      () =>
        openForEdit.mockRejectedValueOnce(new Error("That event is no longer in PM's calendar.")),
      () =>
        openForEdit.mockResolvedValueOnce({
          outcome: "failed",
          message:
            "PM couldn't reach Google. Nothing was changed; try again when you're back online.",
        }),
      () => openForEdit.mockResolvedValueOnce({ outcome: "gone" }),
    ];
    for (const read of reads) {
      saveEdit.mockResolvedValueOnce({
        outcome: "not_moved",
        message: "Google is busy right now.",
        warnings: ["call_failed"],
      });
      const onClose = await openOn();
      pick("Personal", "Side");
      fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
      read();
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
      expect(await screen.findByText(`${head} ${NOT_MOVED_UNREAD}`)).toBeTruthy();
      expect(screen.queryByText(/Nothing was changed|no longer in/)).toBeNull();
      expect(onClose).not.toHaveBeenCalled();
      cleanup();
    }
  });

  // Review round 2 of PR 1: what landed gave a repeating event's occurrences new ids, so the row the
  // editor is on can be dead: nothing more goes from here, where a Save would read as the event
  // deleted. Its words stop offering Save, and Cancel still closes.
  // Review round 3 of PR 1: the likeliest way here, offline, offers no Try again above a held Save.
  it("not moved and unread offline says to open it again, never to try again", async () => {
    saveEdit.mockResolvedValueOnce({
      outcome: "not_moved",
      message:
        "PM couldn't reach Google to change its calendar. Try again when you're back online.",
    });
    await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockRejectedValueOnce(new Error("offline"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const said = await screen.findByText(new RegExp(NOT_MOVED_UNREAD));
    expect(said.textContent).toBe(
      `Your other changes are saved, but it's still on Personal. PM couldn't reach Google to change its calendar. ${NOT_MOVED_UNREAD}`,
    );
  });

  it("not moved and unread holds the editor until it's opened again", async () => {
    saveEdit.mockResolvedValueOnce({
      outcome: "not_moved",
      message:
        "Its guests changed in Google just now, so PM didn't change its calendar. Save again to answer for them.",
    });
    const onDelete = vi.fn();
    const onDuplicate = vi.fn();
    const onClose = await openOn(
      event(),
      { ...ALL, respond: true },
      BOTH,
      PERSONAL,
      vi.fn(),
      {},
      {
        onDelete,
        onDuplicate,
        canDuplicate: true,
      },
    );
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockRejectedValueOnce(new Error("That event is no longer in PM's calendar."));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const said = await screen.findByText(new RegExp(NOT_MOVED_UNREAD));
    expect(said.textContent).toBe(
      `Your other changes are saved, but it's still on Personal. Its guests changed in Google just now, so PM didn't change its calendar. ${NOT_MOVED_UNREAD}`,
    );
    const cancel = screen.getByRole<HTMLButtonElement>("button", { name: "Cancel" });
    await waitFor(() => expect(cancel.disabled).toBe(false));
    await waitFor(() => expect(document.activeElement).toBe(cancel));
    for (const name of ["Save", "Delete", "Duplicate"]) {
      const button = screen.getByRole<HTMLButtonElement>("button", { name });
      expect(button.disabled, name).toBe(true);
      fireEvent.click(button);
    }
    // A reply from here would go on a row that can be dead too: a repeating event's old occurrence.
    const reply = screen.getByRole<HTMLButtonElement>("button", { name: "Yes" });
    expect(reply.disabled).toBe(true);
    fireEvent.click(reply);
    await act(async () => {});
    expect(saveEdit).toHaveBeenCalledTimes(1);
    expect(respond).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(onDuplicate).not.toHaveBeenCalled();
    fireEvent.click(cancel);
    fireEvent.click(await screen.findByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("not moved holds nothing when Google's copy reads again", async () => {
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockResolvedValue(ready(event({ summary: "Retro" }), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Your other changes are saved/);
    const save = screen.getByRole<HTMLButtonElement>("button", { name: "Save" });
    await waitFor(() => expect(save.disabled).toBe(false));
  });

  // Review round 1 of PR 1: after not moved, the other changes are in Google: discarding loses only
  // the calendar change and what was changed since, never "nothing has been saved".
  it("discarding after not moved says the other changes are already saved", async () => {
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    const onClose = await openOn();
    pick("Personal", "Side");
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    openForEdit.mockResolvedValue(ready(event({ summary: "Retro" }), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Your other changes are saved/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(
      screen.getByText(
        "Your other changes are already saved to Google. Only the calendar change, and anything you’ve changed since, is discarded.",
      ),
    ).toBeTruthy();
    expect(screen.queryByText("Nothing has been saved to Google yet.")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
  });

  // Review round 1 of PR 1: at Power depth, another calendar chosen is a change the line names.
  it("the Power-depth line counts the calendar chosen", async () => {
    depth = "power";
    await openOn();
    expect(screen.getByText("No changes yet")).toBeTruthy();
    pick("Personal", "Side");
    expect(screen.getByText("Changed: calendar")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    expect(screen.getByText("Changed: title and calendar")).toBeTruthy();
  });

  // C19d: the replacement landed with the other changes, so the choice is spent. A description
  // Google's copy still holds with formatting PM can't edit shows read-only again, never in the box.
  it("a replaced description that landed without the calendar asks again before replacing", async () => {
    const held: FieldPermissions = {
      ...ALL,
      description: false,
      reasons: ["unsupported_formatting"],
    };
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    await openOn(event({ description: "<h2>Agenda</h2>" }), held);
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    pick("Personal", "Side");
    openForEdit.mockResolvedValue(ready(event({ description: "<h3>Agenda</h3>" }), held, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Your other changes are saved/);
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { description: "Agenda", replace_formatting: true, calendar: SIDE.id },
      null,
      "Dentist",
      null,
      null,
    );
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Agenda");
    expect(screen.getByRole("button", { name: "Replace with plain text…" })).toBeTruthy();
  });

  // C19 review 1: the replacement landed as Google's copy (the words), so the mark is spent: a table
  // someone puts there later shows read-only with Replace, never in a live box that would flatten it
  // on the next key.
  it("a replacement that landed as the words asks again for a table put there later", async () => {
    const held: FieldPermissions = {
      ...ALL,
      description: false,
      reasons: ["unsupported_formatting"],
    };
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    await openOn(event({ description: "<h2>Agenda</h2>" }), held);
    fireEvent.click(screen.getByRole("button", { name: "Replace with plain text…" }));
    fireEvent.click(
      within(
        screen.getByRole("dialog", { name: "Replace the description with plain text?" }),
      ).getByRole("button", { name: "Replace" }),
    );
    pick("Personal", "Side");
    // Google's copy now: the words the replacement sent.
    openForEdit.mockResolvedValue(ready(event({ description: "Agenda" }), ALL, "s2"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Your other changes are saved/);
    expect(description().textContent).toBe("Agenda");
    // Someone puts a table there in Google; the user changes the title and saves again.
    const table = "<table><tr><td>Room</td><td>4.12</td></tr></table>";
    saveEdit.mockResolvedValueOnce({ outcome: "not_moved", message: "Google is busy right now." });
    openForEdit.mockResolvedValue(
      ready(event({ summary: "Retro", description: table }), held, "s3"),
    );
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Retro" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(openForEdit).toHaveBeenCalledTimes(3));
    await waitFor(() =>
      expect(screen.getByRole("group", { name: "Description" }).textContent).toBe("Room 4.12"),
    );
    expect(screen.queryByRole("textbox", { name: "Description" })).toBeNull();
    expect(screen.getByRole("button", { name: "Replace with plain text…" })).toBeTruthy();
    expect(screen.getByText(FORMATTING)).toBeTruthy();
  });

  it("a repeating event says it goes whole and asks which occurrences, All its only row (C18e)", async () => {
    const onClose = vi.fn();
    const occurrence: CalendarEvent = {
      ...row,
      calendar_id: PERSONAL.id,
      series_id: "abc",
      recurring: true,
    };
    openForEdit.mockResolvedValue(ready(event(), ALL));
    render(
      <EventEditor
        row={occurrence}
        calendar={PERSONAL}
        account="me@x.com"
        suggestions={NO_SUGGESTIONS}
        milestone={null}
        colorOf={colourOf}
        targets={createTargets(BOTH)}
        overview={BOTH}
        hidden={new Set([QUIET.id])}
        shapeOf={() => undefined}
        onClose={onClose}
      />,
    );
    await screen.findByLabelText("Title");
    expect(screen.queryByText(CALENDAR_WHOLE)).toBeNull();
    pick("Personal", "Side");
    expect(screen.getByText(CALENDAR_WHOLE)).toBeTruthy();
    const refused = (message: string): ScopeRow => ({ row: "refused", message });
    const whole: SeriesChoices = {
      outcome: "ready",
      this: refused(
        "Changing the calendar can't apply to just this occurrence: the whole repeating event goes to the other calendar.",
      ),
      following: refused(
        "PM puts a whole repeating event on another calendar, not this one and the ones after it.",
      ),
      all: {
        row: "offered",
        warnings: [{ warning: "changes_calendar", to: "Side" }],
        token: "t-all",
      },
    };
    choicesForEdit.mockResolvedValue(whole);
    saveEdit.mockResolvedValue(saved);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const dialog = within(await screen.findByRole("dialog", { name: "Edit repeating event" }));
    expect(choicesForEdit).toHaveBeenCalledWith("s1", { calendar: SIDE.id });
    expect(dialog.getByRole<HTMLInputElement>("radio", { name: /^This event/ }).disabled).toBe(
      true,
    );
    fireEvent.click(dialog.getByRole("radio", { name: /^All events/ }));
    expect(dialog.getByText("Every occurrence goes to Side.")).toBeTruthy();
    fireEvent.click(dialog.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith(
      "s1",
      { calendar: SIDE.id },
      { scope: "all", token: "t-all", asAll: false },
      "Dentist",
      null,
      null,
    );
    // Where it went, from the backend's answer: every occurrence, said by the notice.
    expect(noteSaved).toHaveBeenLastCalledWith(
      "Dentist",
      saved,
      "all",
      "2026-10-12",
      false,
      false,
      { rowId: occurrence.id, sent: null },
      { to: "Side", from: "Personal" },
    );
  });

  it("a new repeat and another calendar go one at a time", async () => {
    await openOn();
    const repeat = () => screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / });
    pick("Personal", "Side");
    // Another calendar holds the repeat, saying why.
    expect(repeat().disabled).toBe(true);
    expect(screen.getByText(CALENDAR_ALONE)).toBeTruthy();
    pick("Side", "Personal");
    expect(repeat().disabled).toBe(false);
    expect(screen.queryByText(CALENDAR_ALONE)).toBeNull();
    // A new repeat holds the calendar, saying why under it.
    await act(async () => {});
    fireEvent.click(repeat());
    const list = screen.getByRole("group", { name: "Repeat" });
    fireEvent.click(within(list).getByRole("radio", { name: "Daily" }), { detail: 1 });
    await act(async () => {});
    expect(trigger("Personal").disabled).toBe(true);
    expect(screen.getByText(CALENDAR_ALONE)).toBeTruthy();
  });
});

// Birthdays and events Gmail added (#884, C20): what Google's list for each kind allows is live, the
// rest greyed with the kind's line; a birthday's date is one whole day, one Date field; neither asks
// whether to email anyone.
describe("EventEditor, a birthday or an event Gmail added", () => {
  const NONE: FieldPermissions = {
    summary: false,
    time: false,
    repeat: false,
    location: false,
    description: false,
    show_as: false,
    visibility: false,
    guests: false,
    reminders: false,
    colour: false,
    meet: false,
    calendar: false,
    delete: false,
    respond: false,
    one_day: false,
    reasons: [],
  };
  /** A birthday's list (gate.rs `special_rights`): its date open only where Google allows. */
  const DATED: FieldPermissions = {
    ...NONE,
    summary: true,
    reminders: true,
    colour: true,
    delete: true,
    one_day: true,
    reasons: ["special_date_fields"],
  };
  const GMAIL: FieldPermissions = {
    ...NONE,
    show_as: true,
    visibility: true,
    reminders: true,
    colour: true,
    delete: true,
    reasons: ["from_gmail_fields"],
  };
  const yearly: CalendarEvent = {
    ...row,
    summary: "Mum",
    location: null,
    event_type: "birthday",
    all_day: true,
    start: "2026-05-04",
    end: "2026-05-05",
  };
  const onTheDay = event({
    summary: "Mum",
    location: "",
    time: { kind: "all_day", first_day: "2026-05-04", last_day: "2026-05-04" },
    start_at: null,
    end_at: null,
  });
  const gmailRow: CalendarEvent = { ...row, event_type: "fromGmail" };
  const placeholder = "unknownorganizer@calendar.google.com";
  /** Gmail's event, organised by Google's placeholder, who is on its list with the calendar. */
  const fromGmail = event({
    organizer_self: false,
    organizer_email: placeholder,
    guests: [
      {
        email: placeholder,
        name: null,
        response: "accepted",
        optional: false,
        organizer: true,
        self: false,
        room: false,
      },
      {
        email: "me@x.com",
        name: null,
        response: "accepted",
        optional: false,
        organizer: false,
        self: true,
        room: false,
      },
    ],
  });
  const disabled = (el: HTMLElement) => (el as HTMLInputElement).disabled;

  it("a birthday from Contacts opens with only its name, notifications and colour live, and both lines, the date's first", async () => {
    openForEdit.mockResolvedValue(
      ready(onTheDay, { ...DATED, reasons: ["date_from_contacts", "special_date_fields"] }),
    );
    await open(vi.fn(), vi.fn(), yearly);
    expect(disabled(screen.getByLabelText("Title"))).toBe(false);
    expect(disabled(screen.getByLabelText("Date"))).toBe(true);
    expect(disabled(screen.getByLabelText("Location"))).toBe(true);
    expect(disabled(screen.getByRole("button", { name: "Free" }))).toBe(true);
    expect(disabled(screen.getByLabelText("Visibility"))).toBe(true);
    // All day on the calendar's defaults: its notifications open with Change.
    expect(disabled(screen.getByRole("button", { name: "Change" }))).toBe(false);
    expect(disabled(screen.getByRole("button", { name: "Colour: Calendar colour" }))).toBe(false);
    // Repeat is greyed: Google repeats a birthday every year itself.
    expect(disabled(screen.getByRole("button", { name: /^Repeat: / }))).toBe(true);
    const contacts = screen.getByText(reasonText("date_from_contacts"));
    const kind = screen.getByText(reasonText("special_date_fields"));
    expect(contacts.compareDocumentPosition(kind) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Google lets a birthday be deleted (C20 §0.4 item 1), and it's never Remove (§0.4 item 11).
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
  });

  it("one day shows a single Date field and no All day switch, and a new date moves the last day with it", async () => {
    openForEdit.mockResolvedValue(ready(onTheDay, { ...DATED, time: true }));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open(vi.fn(), vi.fn(), yearly);
    expect(screen.queryByRole("switch", { name: "All day" })).toBeNull();
    expect(screen.queryByLabelText("First day")).toBeNull();
    expect(screen.queryByLabelText("Last day")).toBeNull();
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    expect(date.disabled).toBe(false);
    date.focus();
    fireEvent.change(date, { target: { value: "05-06-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith(
      "s1",
      { time: { kind: "all_day", first_day: "2026-06-05", last_day: "2026-06-05" } },
      null,
      "Mum",
      null,
      null,
    );
  });

  it("an ordinary all-day event keeps its two days and its switch", async () => {
    openForEdit.mockResolvedValue(ready(onTheDay));
    await open(vi.fn(), vi.fn(), yearly);
    expect(screen.getByRole("switch", { name: "All day" })).toBeTruthy();
    expect(screen.getByLabelText("First day")).toBeTruthy();
    expect(screen.queryByLabelText("Date")).toBeNull();
  });

  it("a Gmail event opens with busy or free, visibility, notifications and colour live and the rest greyed", async () => {
    openForEdit.mockResolvedValue(ready(fromGmail, GMAIL));
    await open(vi.fn(), vi.fn(), gmailRow);
    expect(disabled(screen.getByLabelText("Title"))).toBe(true);
    expect(disabled(screen.getByLabelText("Start time"))).toBe(true);
    expect(disabled(screen.getByLabelText("Location"))).toBe(true);
    expect(disabled(screen.getByRole("button", { name: "Free" }))).toBe(false);
    expect(disabled(screen.getByLabelText("Visibility"))).toBe(false);
    expect(disabled(screen.getByRole("button", { name: "Add notification" }))).toBe(false);
    expect(disabled(screen.getByRole("button", { name: "Colour: Calendar colour" }))).toBe(false);
    expect(screen.getByText(reasonText("from_gmail_fields"))).toBeTruthy();
    // Google's placeholder organiser is no one whose event this is: nothing says "only they can",
    // and its delete is a delete, with no decline (C20 §0.4 item 11).
    expect(screen.queryByText(/Someone else organises/)).toBeNull();
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Remove" })).toBeNull();
  });

  it("a Gmail event's visibility saves with no email question, its organiser on the list", async () => {
    openForEdit.mockResolvedValue(ready(fromGmail, GMAIL));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open(vi.fn(), vi.fn(), gmailRow);
    fireEvent.change(screen.getByLabelText("Visibility"), { target: { value: "private" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(screen.queryByRole("dialog", { name: "Send update emails to guests?" })).toBeNull();
    expect(saveEdit).toHaveBeenCalledWith("s1", { visibility: "private" }, ...UNSCOPED);
  });
});
