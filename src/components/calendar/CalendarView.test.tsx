// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Calendar tab's wiring for a new event (#884, C10c), over a mocked backend: `c` opens the full
// editor and Shift+C quick create; while quick create is open a click on any event comes back to it
// (an overlay's jump would switch tabs, and an event's details would be a second box); More options
// and then Cancel gives focus back to "+ New event"; a saved event's ghost stays until a read that
// started after the save lands, and a click on it opens nothing; the ghost shows on a hidden calendar
// and only in the views that draw it; and a titled draft that went with the tab opens again when the
// calendar comes back. Each test loads the view afresh: its caches and that draft live in the module.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Calendar,
  CalendarAccount,
  CalendarEvent,
  CalendarOverview,
  Milestone,
  WriteOutcome,
} from "../../lib/types";
import { dayKey, startOfDay } from "../../lib/calendar-layout";

const ipc = vi.hoisted(() => ({
  calendarOverview: vi.fn(),
  listAllCalendarEvents: vi.fn(),
  listAllMilestones: vi.fn(),
  getPref: vi.fn(),
  setPref: vi.fn(),
  getSettings: vi.fn(),
  onCalendarSynced: vi.fn(),
  onCalendarWriteLanded: vi.fn(),
  syncCalendar: vi.fn(),
  createCalendarEvent: vi.fn(),
  deleteCalendarEvent: vi.fn(),
  cancelCalendarDelete: vi.fn(),
  getCalendarEventForEdit: vi.fn(),
  listHeldDeletes: vi.fn(),
  onCalendarDeleteSettled: vi.fn(),
  seriesChoices: vi.fn(),
  settleSeriesSplit: vi.fn(),
  updateCalendarEvent: vi.fn(),
  eventFlags: vi.fn(),
  openUrl: vi.fn(),
}));
// What the views and the header are shown: the System, the hidden calendars, and what the Year view
// was handed (it lays nothing out without a measured pane, so it's a stand-in that keeps its events).
const env = vi.hoisted(() => ({
  system: "slate",
  hidden: new Set<string>(),
  yearEvents: null as CalendarEvent[] | null,
}));

vi.mock("../../lib/ipc", () => ipc);

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: env.system, depth: "standard", accent: "mono", colorblind: false }),
}));

vi.mock("../../theme/UserTimeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useUserTime: () => ({ timeZone: "Europe/London", coords: null }),
}));

vi.mock("../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => "Europe/London",
}));

vi.mock("../../lib/calendarPrefs", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  readHidden: () => new Set(env.hidden),
}));

vi.mock("./views/YearView", () => ({
  YearView: ({ events }: { events: CalendarEvent[] }) => {
    env.yearEvents = events;
    return null;
  },
}));

const ME = "gcal:me@x.com";
const PERSONAL = `${ME}:me@x.com`;
const WORK_CAL = `${ME}:work`;

const account: CalendarAccount = {
  id: ME,
  provider: "google",
  email: "me@x.com",
  label: "me@x.com",
  state: "ok",
  last_synced_at: null,
};

function calendar(remote: string, over: Partial<Calendar>): Calendar {
  return {
    id: `${ME}:${remote}`,
    source_id: ME,
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
  accounts: [account],
  calendars: [
    calendar("me@x.com", { name: "Personal", is_primary: true }),
    calendar("work", { name: "Work" }),
  ],
  last_sync: null,
  window_days: 7,
  mirror_start: "2020-01-01T00:00:00Z",
  mirror_end: "2040-01-01T00:00:00Z",
  editing: { [ME]: "on" },
  default_length: {},
};

const today = dayKey(startOfDay(new Date()));

const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: PERSONAL,
  summary: "Standup",
  description: null,
  location: null,
  start: `${today}T07:00:00`,
  end: `${today}T07:30:00`,
  all_day: false,
  html_link: null,
  uid: "u1",
  ...over,
});
const standup = row({});

const milestone: Milestone = {
  id: 1,
  project_name: "Garden",
  label: "Plant bulbs",
  due_date: today,
  event_uid: null,
  calendar_linked: false,
  event_missing: false,
  state: null,
  status: null,
  source_type: null,
  external_id: null,
  sort_order: 0,
};

const saved: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };

/** The mirror's reads, each answered when the test says (`reads[n]`), or all at once with `rows`. */
let reads: ((rows: CalendarEvent[]) => void)[] = [];
let held = false;
let rows: CalendarEvent[] = [standup];
// What the backend's "a write landed" event calls.
let writeLanded: () => void = () => {};

// jsdom ships no ResizeObserver; the box and the pickers' popovers build one.
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
  vi.resetModules();
  localStorage.clear();
  localStorage.setItem("pm.calendar.view", "agenda");
  env.system = "slate";
  env.hidden = new Set();
  env.yearEvents = null;
  reads = [];
  held = false;
  rows = [standup];
  ipc.calendarOverview.mockResolvedValue(overview);
  ipc.listAllCalendarEvents.mockImplementation(() =>
    held ? new Promise<CalendarEvent[]>((r) => reads.push(r)) : Promise.resolve(rows),
  );
  ipc.listAllMilestones.mockResolvedValue([milestone]);
  ipc.getPref.mockResolvedValue(null);
  ipc.onCalendarSynced.mockResolvedValue(() => {});
  ipc.onCalendarWriteLanded.mockImplementation((cb: () => void) => {
    writeLanded = cb;
    return Promise.resolve(() => {});
  });
  ipc.listHeldDeletes.mockResolvedValue([]);
  ipc.onCalendarDeleteSettled.mockResolvedValue(() => {});
  ipc.eventFlags.mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

/** The Calendar tab, fresh, once its first reads have landed. `Calendar` mounts it again (the same
 *  module, as switching back to the tab does). The events and the calendars are read side by side:
 *  the rest of those reads, and the effects that rebind the keys to the calendars, are let finish
 *  too, since a key pressed before them would reach a handler with no calendars yet and open
 *  nothing (the source of an occasional failure here). */
async function mount(Calendar?: typeof import("./CalendarView").CalendarView) {
  const View = Calendar ?? (await import("./CalendarView")).CalendarView;
  const onOpenProject = vi.fn();
  const onOpenPinboard = vi.fn();
  const view = render(<View onOpenProject={onOpenProject} onOpenPinboard={onOpenPinboard} />);
  await screen.findByRole("button", { name: /Standup/ });
  await act(async () => {});
  return { onOpenProject, onOpenPinboard, unmount: view.unmount, View };
}

const newEvent = () => screen.getByRole("button", { name: "+ New event" });
const quick = () => screen.getByRole("dialog", { name: "New event" });
const title = () => within(quick()).getByLabelText<HTMLInputElement>("Title");
const ghost = () => document.querySelector<HTMLElement>("[data-draft-event]");
const press = (key: string, shiftKey = false) =>
  act(() => void fireEvent.keyDown(document.body, { key, shiftKey }));

/** Shift+C, and a title typed. */
function quickCreate(summary: string) {
  press("C", true);
  fireEvent.change(title(), { target: { value: summary } });
}

describe("CalendarView: a new event", () => {
  it("Shift+C opens quick create, non-modal, under + New event; c the full editor", async () => {
    await mount();
    press("C", true);
    expect(quick().getAttribute("aria-modal")).toBeNull();
    await waitFor(() => expect(document.activeElement).toBe(title()));
    fireEvent.click(within(quick()).getByRole("button", { name: "Discard" }));
    expect(screen.queryByRole("dialog", { name: "New event" })).toBeNull();
    expect(document.activeElement).toBe(newEvent());
    press("c");
    const editor = screen.getByRole("dialog", { name: "New event" });
    expect(editor.getAttribute("aria-modal")).toBe("true");
    expect(within(editor).getByRole("button", { name: "Save" })).toBeTruthy();
  });

  // Quick create's box is non-modal: the grid stays live. A milestone's jump would switch tabs and
  // take the draft with it, and an event's details would be a second box with a second primary
  // button, so a click on any event (the ghost too) goes back to the box instead.
  it("with quick create open, a click on any event comes back to it", async () => {
    const { onOpenProject } = await mount();
    quickCreate("Dentist");
    const drawn = ghost()!;
    expect(drawn.getAttribute("aria-label")).toMatch(/^New event, not saved yet: .*Dentist$/);
    for (const name of [/Plant bulbs · Garden/, /Standup/]) {
      const target = screen.getByRole("button", { name });
      target.focus();
      fireEvent.click(target);
      expect(document.activeElement).toBe(title());
    }
    drawn.focus();
    fireEvent.click(drawn);
    expect(document.activeElement).toBe(title());
    expect(onOpenProject).not.toHaveBeenCalled();
    // No details opened: the one dialog is the box.
    expect(screen.getAllByRole("dialog")).toEqual([quick()]);
    // With the box gone, an overlay's jump is back.
    fireEvent.click(within(quick()).getByRole("button", { name: "Discard" }));
    fireEvent.click(screen.getByRole("button", { name: /Plant bulbs · Garden/ }));
    expect(onOpenProject).toHaveBeenCalledWith("Garden");
  });

  // More options blurs what it reads, so the full editor's focus trap remembers the page, not the
  // box: closing it puts focus back on "+ New event", as the box's own close would have.
  it("More options and then Cancel gives focus back to + New event", async () => {
    await mount();
    quickCreate("Dentist");
    fireEvent.click(within(quick()).getByRole("button", { name: "More options" }));
    const editor = screen.getByRole("dialog", { name: "New event" });
    expect(editor.getAttribute("aria-modal")).toBe("true");
    expect(within(editor).getByLabelText<HTMLInputElement>("Title").value).toBe("Dentist");
    fireEvent.click(within(editor).getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(newEvent());
    expect(ipc.createCalendarEvent).not.toHaveBeenCalled();
  });

  // The save's own read of the mirror (the backend's "a write landed") can begin before its answer,
  // so it may not hold the new row: the ghost stays for a read begun after the save, which closing
  // the box starts. A click on the ghost meanwhile opens nothing: it isn't an event in the calendar.
  it("a saved event's ghost stays until a read begun after the save lands", async () => {
    const { onOpenProject } = await mount();
    quickCreate("Dentist");
    let answer: (o: WriteOutcome) => void = () => {};
    ipc.createCalendarEvent.mockReturnValue(new Promise<WriteOutcome>((r) => (answer = r)));
    held = true;
    fireEvent.keyDown(title(), { key: "Enter" });
    // While it's out, it may be made: never "not saved".
    expect(ghost()!.getAttribute("aria-label")).toMatch(/^New event, saving: .*Dentist$/);
    // The backend's read for the write starts before the answer.
    act(() => writeLanded());
    await act(async () => answer(saved));
    expect(screen.queryByRole("dialog", { name: "New event" })).toBeNull();
    const drawn = ghost()!;
    expect(drawn.getAttribute("aria-label")).toMatch(/^New event, saved: .*Dentist$/);
    // A click on it opens no details, and jumps nowhere.
    fireEvent.click(drawn);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(onOpenProject).not.toHaveBeenCalled();
    // Two reads out: the write's, and the one closing started. The first can't clear the ghost.
    expect(reads).toHaveLength(2);
    const made = row({ id: "e2", uid: "u2", summary: "Dentist", start: `${today}T23:00:00` });
    await act(async () => reads[0]([standup, made]));
    expect(ghost()).not.toBeNull();
    // The newer read lands: the row is the calendar's own now, and the ghost has gone.
    await act(async () => reads[1]([standup, made]));
    expect(ghost()).toBeNull();
    expect(screen.getByRole("button", { name: /Dentist/ }).hasAttribute("data-draft-event")).toBe(
      false,
    );
  });

  // The ghost joins after the hide filter (the box says the calendar is hidden), and only the views
  // that draw it get it: the Year overview and the Terminal tables show what's in the calendar.
  it("the ghost shows on a hidden calendar, and only in the views that draw it", async () => {
    env.hidden = new Set([WORK_CAL]);
    await mount();
    quickCreate("Dentist");
    fireEvent.click(within(quick()).getByRole("button", { name: "Calendar: Personal" }));
    fireEvent.click(screen.getByRole("radio", { name: /^Work/ }), { detail: 1 });
    expect(ghost()?.getAttribute("aria-label")).toMatch(/Dentist$/);
    fireEvent.click(screen.getByRole("button", { name: "Year" }));
    expect(env.yearEvents?.map((e) => e.id)).toContain("e1");
    expect(env.yearEvents?.some((e) => e.summary === "Dentist")).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Agenda" }));
    expect(ghost()).not.toBeNull();
  });

  it("the Terminal look draws no ghost", async () => {
    env.system = "terminal";
    await mount();
    quickCreate("Dentist");
    expect(ghost()).toBeNull();
    expect(screen.queryByText("Dentist")).toBeNull();
  });

  // A click in the sidebar switches tabs (a titled box stays through a click), which takes the box
  // with it: back on the calendar, it opens again on the same draft, nothing having been sent.
  it("a titled quick create that went with the tab opens again when the calendar comes back", async () => {
    const first = await mount();
    quickCreate("Dentist");
    first.unmount();
    await act(async () => {});
    await mount(first.View);
    await waitFor(() => expect(title().value).toBe("Dentist"));
    expect(ghost()?.getAttribute("aria-label")).toMatch(/^New event, not saved yet: .*Dentist$/);
    expect(ipc.createCalendarEvent).not.toHaveBeenCalled();
    // Once only: closed, it stays closed the next time.
    fireEvent.click(within(quick()).getByRole("button", { name: "Discard" }));
    cleanup();
    await act(async () => {});
    await mount(first.View);
    expect(screen.queryByRole("dialog", { name: "New event" })).toBeNull();
  });
});
