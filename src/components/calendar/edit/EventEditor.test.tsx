// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The event editor's promises (#884): only what changed is sent, a typed date counts even if Save is
// pressed before the field loses focus, a date it can't read is said naming its field (marked, and
// where a save it stops puts focus), a conflict never loses the draft silently, a dirty draft asks
// before it goes, and what can't change is greyed out with a reason (no device zone included).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  EditLoad,
  EventForEdit,
  EventPatchDraft,
  FieldPermissions,
  Milestone,
  ScopeRow,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import type { Picked } from "../../../lib/calendarEdit/saveChoices";

type SaveAnswer = WriteOutcome & { newPartial?: boolean };
const openForEdit = vi.fn<(id: string) => Promise<EditLoad>>();
const saveEdit =
  vi.fn<
    (
      session: string,
      changes: EventPatchDraft,
      picked: Picked | null,
      title: string,
    ) => Promise<SaveAnswer>
  >();
const choicesForEdit = vi.fn<(session: string, draft: EventPatchDraft) => Promise<SeriesChoices>>();
const settleSplit =
  vi.fn<(token: string, action: SplitAction, title: string) => Promise<WriteOutcome | null>>();
const noteSaved = vi.fn();
const noteSeriesNotice = vi.fn();
let deviceZone: string | null = "Europe/London";

vi.mock("./useEventWrites", () => ({
  openForEdit: (id: string) => openForEdit(id),
  // Every argument passes through, so a scope sent without its token couldn't pass for free.
  saveEdit: (s: string, c: EventPatchDraft, p: Picked | null, t: string) => saveEdit(s, c, p, t),
  choicesForEdit: (s: string, d: EventPatchDraft) => choicesForEdit(s, d),
  settleSplit: (token: string, action: SplitAction, title: string) =>
    settleSplit(token, action, title),
  noteSaved: (...a: unknown[]) => noteSaved(...a),
  noteSeriesNotice: (text: string) => noteSeriesNotice(text),
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

import { EventEditor } from "./EventEditor";

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
      milestone={null}
      onClose={onClose}
      onDelete={onDelete}
    />,
  );
  await screen.findByLabelText("Title");
  return onClose;
}

/** A one-off's save: no scope, and the draft's title for its notices. */
const UNSCOPED = [null, "Dentist"] as const;

beforeEach(() => {
  deviceZone = "Europe/London";
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
    );
    expect(noteSaved).toHaveBeenCalledWith(
      "Dentist (moved)",
      { outcome: "saved", warnings: [] },
      null,
      "2026-10-12",
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
      <EventEditor row={row} calendar={null} account={null} milestone={null} onClose={() => {}} />,
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
      { scope: "all", token: "t-all" },
      "Gym",
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
    expect(saveEdit.mock.calls[0][2]).toEqual({ scope: "this", token: "t-this" });
  });

  it("opens the choices when a save without one is answered that it repeats (R9)", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice" });
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
    saveEdit.mockResolvedValue({ outcome: "needs_choice" });
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
    saveEdit.mockResolvedValueOnce({ outcome: "needs_choice" });
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
      result: { outcome: "read_only", reason: "has_guests" },
    });
    await open(vi.fn(), undefined, occurrence);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Gym" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/This event has guests/)).toBeTruthy();
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
    expect(saveEdit).toHaveBeenCalledWith("s1", { summary: "Gym" }, null, "Gym");
    expect(saveEdit.mock.calls[0][1]).toBe(choicesForEdit.mock.calls[0][1]);
  });

  it("stops when the save wants a choice and the choices say it doesn't repeat", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    // Twice each, then answers that end a loop, so one would fail here rather than hang.
    saveEdit
      .mockResolvedValueOnce({ outcome: "needs_choice" })
      .mockResolvedValueOnce({ outcome: "needs_choice" })
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
