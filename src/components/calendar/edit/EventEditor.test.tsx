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
  CalendarEvent,
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
const noteSeriesNotice = vi.fn();
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
  ) => saveEdit(s, c, p, t, n, a),
  choicesForEdit: (s: string, d: EventPatchDraft) => choicesForEdit(s, d),
  settleSplit: (token: string, action: SplitAction, title: string) =>
    settleSplit(token, action, title),
  noteSaved: (...a: unknown[]) => noteSaved(...a),
  noteSeriesNotice: (text: string) => noteSeriesNotice(text),
}));

// The repeat menu's two reads, answered as each test says.
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

vi.mock("../../../lib/markdown", () => ({
  Markdown: ({ children }: { children: string }) => <div>{children}</div>,
}));

import { EventEditor, UNREADABLE_REPEAT } from "./EventEditor";

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
  location: true,
  description: true,
  show_as: true,
  visibility: true,
  guests: true,
  reminders: true,
  colour: true,
  delete: true,
  reasons: [],
};

function event(over: Partial<EventForEdit> = {}): EventForEdit {
  return {
    summary: "Dentist",
    location: "High St",
    description: "",
    description_html: false,
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
  },
  series: null,
});

async function open(
  onClose = vi.fn(),
  onDelete?: (r: CalendarEvent) => void,
  clicked: CalendarEvent = row,
) {
  render(
    <EventEditor
      row={clicked}
      calendar={null}
      account="me@x.com"
      suggestions={NO_SUGGESTIONS}
      milestone={null}
      onClose={onClose}
      onDelete={onDelete}
    />,
  );
  await screen.findByLabelText("Title");
  return onClose;
}

/** No one to suggest as a guest. */
const NO_SUGGESTIONS: readonly GuestSuggestion[] = [];

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
  recurrencePresets.mockImplementation(async (day) => presetsFor(day));
  describeRecurrence.mockImplementation(async (_rule, start) => readyAt(start));
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("EventEditor", () => {
  // Google's formatted description shows read-only, as its text: lines where Google breaks them,
  // escapes decoded once, and nothing of the markup (a script's text included) on screen.
  it("shows a formatted description as its text", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          description:
            "<b>Agenda</b><br><ul><li>One &amp; two</li></ul><script>alert(1)</script>&lt;i&gt;",
          description_html: true,
        }),
      ),
    );
    await open();
    const text = screen.getByText(
      (_, el) => el?.tagName === "P" && /Agenda/.test(el.textContent ?? ""),
    );
    expect(text.textContent).toBe("Agenda\nOne & two\n<i>");
    expect(text.querySelector("b, i, script")).toBeNull();
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
    );
    expect(choicesForEdit).not.toHaveBeenCalled();
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
  });

  it("says no reason for the notifications or colour, which have no row yet (C13 critique C1-M10)", async () => {
    openForEdit.mockResolvedValue(
      ready(event({ label_id: "label-1", color_id: "5" }), {
        ...ALL,
        reminders: false,
        colour: false,
        reasons: ["label_read_only", "reminders_unreadable", "html_description"],
      }),
    );
    await open(vi.fn(), vi.fn());
    expect(screen.getByText(/This description has formatting/)).toBeTruthy();
    expect(screen.queryByText(/Google Calendar label/)).toBeNull();
    expect(screen.queryByText(/notifications/)).toBeNull();
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
    const desc = screen.getByLabelText("Description");
    // Nothing between the field and the window swallows Enter (fireEvent is false if anything
    // called preventDefault), so the textarea's own line break happens.
    expect(fireEvent.keyDown(desc, { key: "Enter" })).toBe(true);
    expect(fireEvent.keyDown(desc, { key: "Enter", shiftKey: true })).toBe(true);
    fireEvent.change(desc, { target: { value: "line one\nline two" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { description: "line one\nline two" }, ...UNSCOPED);
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
    expect(await screen.findByText(/can't be changed here/)).toBeTruthy();
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
