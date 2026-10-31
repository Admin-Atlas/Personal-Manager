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
  EditLoad,
  EventForEdit,
  GuestForEdit,
  GuestNotice,
  Milestone,
  NewEventDraft,
  WriteOutcome,
} from "../../lib/types";
import { addDays, dayKey, startOfDay, startOfWeek } from "../../lib/calendar-layout";
import { A11_NOTE } from "../../lib/calendarEdit/editReasons";
import { COPY_CALL_OFF_NOTE } from "../../lib/calendarEdit/eventDraft";
import { formatDateLocal, pad2 } from "../../lib/format";

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
  respondToCalendarEvent: vi.fn(),
  getSeriesSummary: vi.fn(),
  eventFlags: vi.fn(),
  openUrl: vi.fn(),
  requestCalendarAccess: vi.fn(),
  recurrencePresets: vi.fn(),
  describeRecurrence: vi.fn(),
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
  auto_meet: {},
  ask_access: {},
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
  // What the list says PM may offer on it (C14): yours, so Edit and Delete, and nobody invited.
  edit_block: null,
  can_delete: true,
  can_respond: false,
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
  // The Repeat row in quick create and the full editor: no ready-made rules, so just its own rows.
  ipc.recurrencePresets.mockResolvedValue([]);
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

// C16a: the box the drag gesture will hold the pointer in (C16b), which the LD1 probe finds by its
// attribute: the swipe's own box, holding the event the view draws.
describe("CalendarView: the grid's box", () => {
  it("marks the swipe's box, around the events, for the drag and its probe", async () => {
    await mount();
    const grids = document.querySelectorAll("[data-calendar-grid]");
    expect(grids).toHaveLength(1);
    expect(grids[0].contains(screen.getByRole("button", { name: /Standup/ }))).toBe(true);
  });
});

// C16b: Day and Week drag once a Google account has editing on. A drop is drawn where it went at once
// (dashed, and not dragged again until Google has it), and another account's copy of the event hides
// until that account's own read catches up, so it never shows at the old time meanwhile.
describe("CalendarView: dragging in Week", () => {
  const cards = (id: string) =>
    document.querySelectorAll<HTMLElement>(`[data-time-body] [data-event-id="${id}"]`);
  // Yours, and the list says its time can change.
  const movable = row({ can_retime: true });

  it("marks the grid for the drag only where events can be dragged", async () => {
    localStorage.setItem("pm.calendar.view", "week");
    rows = [movable];
    const first = await mount();
    expect(document.querySelector("[data-time-body]")).not.toBeNull();
    // A half-hour card at the unmeasured row height has room for its bottom edge only.
    expect(cards("e1")[0].querySelector('[data-edge="end"]')).not.toBeNull();
    first.unmount();
    // Editing off: the same Week, drawn as it always was.
    ipc.calendarOverview.mockResolvedValue({ ...overview, editing: { [ME]: "off" } });
    await mount();
    expect(screen.getByRole("button", { name: /Standup/ })).toBeTruthy();
    expect(document.querySelector("[data-time-body], [data-event-id]")).toBeNull();
  });

  it("a drop shows where it went at once, and hides another account's copy meanwhile", async () => {
    localStorage.setItem("pm.calendar.view", "week");
    const copy = row({ id: "e2", calendar_id: WORK_CAL });
    rows = [movable, copy];
    await mount();
    // One occurrence, two copies: one drawn.
    expect(cards("e1")).toHaveLength(1);
    expect(cards("e2")).toHaveLength(0);
    expect(cards("e1")[0].querySelector('[data-edge="end"]')).not.toBeNull();
    const top = parseFloat(cards("e1")[0].style.top);
    const writes = await import("./edit/useEventWrites");
    const from = {
      kind: "timed" as const,
      start: { day: today, min: 420 },
      end: { day: today, min: 450 },
    };
    const to = {
      kind: "timed" as const,
      start: { day: today, min: 540 },
      end: { day: today, min: 570 },
    };
    act(() => void writes.beginMove(writes.dropOf(movable, from, to, false)));
    const moved = cards("e1")[0];
    expect(parseFloat(moved.style.top)).toBeGreaterThan(top);
    expect(moved.className).toContain("opacity-70");
    // "Saving…": no edge to take until it lands.
    expect(moved.querySelectorAll("[data-edge]")).toHaveLength(0);
    // Google has the move on this account; the other account's copy is still at the old time.
    rows = [{ ...movable, start: `${today}T09:00:00`, end: `${today}T09:30:00` }, copy];
    await act(async () => writeLanded());
    expect(cards("e1")).toHaveLength(1);
    expect(cards("e2")).toHaveLength(0);
  });

  // C16c: Month drags on the same terms.
  // jsdom lays out no weeks, so the stream is looked at, not the events in it.
  it("marks Month's weeks for the drag only where events can be dragged", async () => {
    localStorage.setItem("pm.calendar.view", "month");
    rows = [movable];
    const { CalendarView } = await import("./CalendarView");
    const first = render(<CalendarView />);
    await screen.findByRole("button", { name: "+ New event" });
    await act(async () => {});
    expect(document.querySelector('[data-surface="month"]')).not.toBeNull();
    first.unmount();
    ipc.calendarOverview.mockResolvedValue({ ...overview, editing: { [ME]: "off" } });
    render(<CalendarView />);
    await act(async () => {});
    await act(async () => {});
    expect(document.querySelector("[data-calendar-grid] .overflow-y-auto")).not.toBeNull();
    expect(document.querySelector("[data-surface]")).toBeNull();
  });
});

describe("CalendarView: a new event", () => {
  // C16 critique C2-M9: a date typed in quick create that the view doesn't show moves the view to
  // it, as Google's does; the box opening on a day already shown moves nothing.
  it("a date typed in quick create off screen moves the week to it", async () => {
    localStorage.setItem("pm.calendar.view", "week");
    await mount();
    const week = (d: Date) => `${formatDateLocal(d)} – ${formatDateLocal(addDays(d, 6))}`;
    const now = startOfWeek(new Date());
    quickCreate("Dentist");
    expect(screen.getByText(week(now))).toBeTruthy();
    const later = addDays(now, 21);
    const date = within(quick()).getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, {
      target: {
        value: `${pad2(later.getDate())}-${pad2(later.getMonth() + 1)}-${later.getFullYear()}`,
      },
    });
    fireEvent.blur(date);
    await act(async () => {});
    expect(screen.getByText(week(later))).toBeTruthy();
    expect(quick()).toBeTruthy();
  });

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

// The Calendar tab's guest wiring (#884, C12c review): the editor gets the people already in the
// user's calendars for its guest box, and a delete the backend asks about guests reopens its dialog
// with the backend's numbers and the account Google sends the emails from, not the row's own count.
describe("CalendarView: guests", () => {
  const ann = {
    name: "Ann",
    email: "ann@x.org",
    response: "accepted",
    optional: false,
    organizer: false,
    self: false,
  } as const;

  /** Click the Standup chip, then a button of its details. */
  async function fromPopover(button: "Edit" | "Delete") {
    fireEvent.click(screen.getByRole("button", { name: /Standup/ }));
    fireEvent.click(await screen.findByRole("button", { name: button }));
  }

  it("suggests people from the user's calendars in the editor's guest box", async () => {
    rows = [standup, row({ id: "e2", uid: "u2", summary: "Lunch", attendees: [ann] })];
    const editable: EventForEdit = {
      summary: "Standup",
      location: "",
      description: "",
      time: {
        kind: "timed",
        start_date: today,
        start_time: "07:00",
        start_zone: "Europe/London",
        end_date: today,
        end_time: "07:30",
        end_zone: "Europe/London",
      },
      start_at: `${today}T06:00:00Z`,
      end_at: `${today}T06:30:00Z`,
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
      call: null,
      meet_offered: false,
    };
    const load: EditLoad = {
      outcome: "ready",
      session: "s1",
      event: editable,
      permissions: {
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
        meet: false,
        calendar: true,
        delete: true,
        respond: false,
        one_day: false,
        reasons: [],
      },
      seen: null,
      series: null,
    };
    ipc.getCalendarEventForEdit.mockResolvedValue(load);
    await mount();
    await fromPopover("Edit");
    const editor = await screen.findByRole("dialog", { name: "Edit event" });
    const box = await within(editor).findByRole("combobox", { name: "Add guests" });
    fireEvent.change(box, { target: { value: "ann" } });
    const option = within(within(editor).getByRole("listbox", { name: "Suggestions" })).getByRole(
      "option",
    );
    expect(option.textContent).toContain("Ann");
    expect(option.textContent).toContain("ann@x.org");
  });

  // The list decides Delete, as the backend's gate does (C14): a row it marks undeletable (a locked
  // copy, an invitation) offers Edit and no Delete, whatever the webview could guess from the row.
  it("offers Delete only where the list says it can be deleted", async () => {
    rows = [row({ can_delete: false })];
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /Standup/ }));
    expect(await screen.findByRole("button", { name: "Edit" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("reopens a delete the backend asks about guests with its numbers and the sending account", async () => {
    const told: GuestNotice = {
      updated: 1,
      invited: 0,
      removed: 0,
      told_cancelled: 0,
      cancelled: true,
      people: 1,
      emails: 1,
      complete: true,
    };
    ipc.deleteCalendarEvent.mockResolvedValue({
      outcome: "refused",
      result: { outcome: "needs_choice", scope: false, guests: told },
    });
    await mount();
    await fromPopover("Delete");
    const first = screen.getByRole("dialog", { name: "Delete this event?" });
    // The row shows no guests: nothing to ask yet.
    expect(within(first).queryByRole("radio")).toBeNull();
    fireEvent.click(within(first).getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("1 guest will be told it's cancelled.")).toBeTruthy();
    const again = screen.getByRole("dialog", { name: "Delete this event?" });
    expect(within(again).getByText("Google sends the emails from me@x.com.")).toBeTruthy();
    expect(
      within(again).getByText(
        "It wasn't deleted. PM found guests who would be told. Choose whether Google emails them.",
      ),
    ).toBeTruthy();
    const del = within(again).getByRole<HTMLButtonElement>("button", { name: "Delete" });
    expect(del.disabled).toBe(true);
    fireEvent.click(within(again).getByRole("radio", { name: /^Send/ }));
    fireEvent.click(del);
    await waitFor(() => expect(ipc.deleteCalendarEvent).toHaveBeenCalledTimes(2));
    expect(ipc.deleteCalendarEvent).toHaveBeenLastCalledWith(
      standup.id,
      expect.anything(),
      null,
      null,
      "all",
      told,
    );
  });
});

// Replying to invitations (#884, C14b): "Going?" in the popover from each copy the user can reply
// from (with both accounts shown, the dedup keeps the organiser's copy, and the invitation's copy
// would otherwise have no surface); the line under someone else's event; the popover reading its
// row afresh; a declined event struck through only when every copy with the user's entry says so;
// and a reply's answer said as a notice once the popover has closed (C14 critique C2-M1).
describe("CalendarView: invitations", () => {
  const own = (email: string, response: string, organizer = false) => ({
    name: null,
    email,
    response,
    optional: false,
    organizer,
    self: true,
  });
  /** Bobby's copy of someone else's invitation, on his main calendar: editable for his own
   *  settings since C14c, never deleted. */
  const invite = (response = "needsAction") =>
    row({
      id: "i1",
      edit_block: null,
      can_delete: false,
      can_respond: true,
      organizer_self: false,
      attendees: [own("me@x.com", response)],
    });
  /** The organiser's copy of the same occurrence, on another calendar he can see. */
  const organiser = (response = "accepted") =>
    row({
      id: "o1",
      calendar_id: WORK_CAL,
      can_respond: true,
      organizer_self: true,
      attendees: [own("work", response, true)],
    });
  const chip = () => screen.getByRole("button", { name: /Standup/ });

  it("offers Going? from every copy of the occurrence, the one shown first, each named", async () => {
    rows = [invite(), organiser()];
    ipc.respondToCalendarEvent.mockResolvedValue(saved);
    await mount();
    fireEvent.click(chip());
    // The organiser's copy is the one shown (it can be deleted); the invitation's is offered too.
    const groups = screen.getAllByRole("group", { name: /^Going\?/ });
    expect(groups).toHaveLength(2);
    expect(groups[0]).toBe(screen.getByRole("group", { name: "Going? Replying for Work." }));
    const personal = screen.getByRole("group", { name: "Going? Replying for Personal." });
    expect(groups[1]).toBe(personal);
    await act(async () => fireEvent.click(within(personal).getByRole("button", { name: "No" })));
    expect(ipc.respondToCalendarEvent).toHaveBeenCalledWith("i1", "no", null, null);
  });

  it("says under an invitation what the user may still do, and how with editing off", async () => {
    rows = [invite()];
    await mount();
    fireEvent.click(chip());
    expect(
      screen.getByText(
        "Someone else organises this event, so only they can change its title, time, place, description, visibility, guests, video call and calendar. Your reply, notifications, colour and busy or free are yours to change.",
      ),
    ).toBeTruthy();
    expect(screen.getByRole("group", { name: "Going?" })).toBeTruthy();
    // C14c: Edit, for the user's own settings, and never Delete.
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
    cleanup();
    ipc.calendarOverview.mockResolvedValue({ ...overview, editing: { [ME]: "off" } });
    await mount();
    fireEvent.click(chip());
    expect(
      screen.getByText(
        "Someone else organises this event. Turn on editing for this account to reply and change your own settings from PM.",
      ),
    ).toBeTruthy();
    expect(screen.queryByRole("group", { name: /^Going\?/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  });

  // C14c: where the organiser lets guests change it, the line is Google's warning; with no entry of
  // the user's own, it never offers the reply.
  it("says Google's warning where guests may change it, and never a reply it can't send", async () => {
    rows = [row({ ...invite(), guests_can_modify: true })];
    await mount();
    fireEvent.click(chip());
    expect(screen.getByText(A11_NOTE)).toBeTruthy();
    cleanup();
    rows = [row({ ...invite(), can_respond: false, attendees: [] })];
    await mount();
    fireEvent.click(chip());
    expect(
      screen.getByText(
        "Someone else organises this event, so only they can change its title, time, place, description, visibility, guests, video call and calendar. Your notifications, colour and busy or free are yours to change.",
      ),
    ).toBeTruthy();
  });

  // C14 §9.5: an invitation can be changed now (its own settings), so "can be changed" no longer
  // means "yours": the organiser's copy is kept, even one that can't be deleted (locked), over an
  // editable invitation listed first.
  it("keeps the organiser's copy over an editable invitation of the same occurrence", async () => {
    rows = [invite(), row({ ...organiser(), locked: true, can_delete: false })];
    await mount();
    fireEvent.click(chip());
    const groups = screen.getAllByRole("group", { name: /^Going\?/ });
    expect(groups[0]).toBe(screen.getByRole("group", { name: "Going? Replying for Work." }));
    expect(
      screen.getByText(
        "Google has locked this event's title, time, place and description, so PM also keeps it on its calendar.",
      ),
    ).toBeTruthy();
    // An invitation still wins over a copy that can't be changed at all.
    cleanup();
    rows = [row({ ...organiser(), edit_block: "calendar_read_only", can_delete: false }), invite()];
    await mount();
    fireEvent.click(chip());
    expect(screen.getAllByRole("group", { name: /^Going\?/ })[0]).toBe(
      screen.getByRole("group", { name: "Going? Replying for Personal." }),
    );
    // Two invitations' copies rank the same: the first listed is kept.
    for (const [first, name] of [
      [PERSONAL, "Personal"],
      [WORK_CAL, "Work"],
    ] as const) {
      cleanup();
      const other = first === PERSONAL ? WORK_CAL : PERSONAL;
      rows = [
        row({ ...invite(), id: "i1", calendar_id: first }),
        row({ ...invite(), id: "i2", calendar_id: other }),
      ];
      await mount();
      fireEvent.click(chip());
      expect(screen.getAllByRole("group", { name: /^Going\?/ })[0]).toBe(
        screen.getByRole("group", { name: `Going? Replying for ${name}.` }),
      );
    }
  });

  it("after a reply lands, the popover shows it, and the event is struck through", async () => {
    rows = [invite()];
    ipc.respondToCalendarEvent.mockResolvedValue(saved);
    await mount();
    fireEvent.click(chip());
    const dialog = screen.getByRole("dialog", { name: "Standup" });
    expect(dialog.textContent).toContain("no reply");
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "No" })));
    // The backend landed Google's copy and says so; the list is read again.
    rows = [invite("declined")];
    await act(async () => writeLanded());
    expect(screen.getByRole("dialog", { name: "Standup" }).textContent).toContain("declined");
    expect(screen.getByRole("button", { name: "No" }).getAttribute("aria-pressed")).toBe("true");
    const struck = screen.getByRole("button", { name: /Standup, declined$/ });
    expect(within(struck).getByText("Standup").className).toContain("line-through");
  });

  // C14 critiques C1-M4, C2-H3: the kept copy may be the organiser's, which the reply doesn't
  // change; Google draws each calendar's copy apart.
  it("strikes a merged event through only when every copy with the user's entry declined", async () => {
    rows = [invite("declined"), organiser("accepted")];
    await mount();
    expect(chip().getAttribute("aria-label") ?? "").not.toMatch(/declined/);
    expect(within(chip()).getByText("Standup").className).not.toContain("line-through");
    cleanup();
    rows = [invite("declined"), organiser("declined")];
    await mount();
    expect(screen.getByRole("button", { name: /Standup, declined$/ })).toBeTruthy();
    // Hiding the organiser's calendar leaves the invitation's copy to say it, and the only one to
    // reply from (on the main calendar, so unnamed).
    cleanup();
    env.hidden = new Set([WORK_CAL]);
    rows = [invite("declined"), organiser("accepted")];
    await mount();
    fireEvent.click(screen.getByRole("button", { name: /Standup, declined$/ }));
    expect(screen.getAllByRole("group", { name: /^Going\?/ })).toEqual([
      screen.getByRole("group", { name: "Going?" }),
    ]);
  });

  // Two entries marked as the calendar (an alias): its own address is the user's, as the backend
  // reads it, for the strike-through and for the reply shown.
  it("reads the calendar's own entry when an alias is marked too", async () => {
    rows = [
      row({
        ...invite(),
        attendees: [own("Me+pm@x.com", "declined"), own("ME@x.com", "accepted")],
      }),
    ];
    await mount();
    expect(chip().getAttribute("aria-label") ?? "").not.toMatch(/declined/);
    fireEvent.click(chip());
    expect(screen.getByRole("button", { name: "Yes" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("asks which occurrences first on a repeating invitation", async () => {
    rows = [row({ ...invite(), series_id: "s1", recurring: true })];
    ipc.respondToCalendarEvent.mockResolvedValue(saved);
    // The popover asks how it repeats; PM can't say here.
    ipc.getSeriesSummary.mockResolvedValue(null);
    await mount();
    fireEvent.click(chip());
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    expect(screen.getByRole("group", { name: "Reply Yes to which occurrences?" })).toBeTruthy();
    expect(ipc.respondToCalendarEvent).not.toHaveBeenCalled();
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "All events" })));
    expect(ipc.respondToCalendarEvent).toHaveBeenCalledWith("i1", "yes", null, "all");
  });

  // PR B review round 1: the occurrence clicked kept its own No when Yes went to all events, and
  // its row lands still declined: "Going?" says No, as the guest list and the struck-through chip do.
  it("an occurrence that kept its own answer for all events keeps it in the popover", async () => {
    const declined = () => row({ ...invite("declined"), series_id: "s1", recurring: true });
    rows = [declined()];
    ipc.respondToCalendarEvent.mockResolvedValue({
      outcome: "saved",
      warnings: ["kept_own_reply"],
      left_behind: [today],
    });
    ipc.getSeriesSummary.mockResolvedValue(null);
    await mount();
    fireEvent.click(chip());
    const going = () =>
      ["Yes", "No", "Maybe"].filter(
        (name) => screen.getByRole("button", { name }).getAttribute("aria-pressed") === "true",
      );
    expect(going()).toEqual(["No"]);
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "All events" })));
    expect(ipc.respondToCalendarEvent).toHaveBeenCalledWith("i1", "yes", null, "all");
    expect(going()).toEqual(["No"]);
    rows = [declined()];
    await act(async () => writeLanded());
    expect(screen.getByRole("dialog", { name: "Standup" }).textContent).toContain("declined");
    expect(going()).toEqual(["No"]);
  });

  // PR B review round 2: an all-events reply PM couldn't read back yet may not have reached the
  // occurrence clicked (answered No on its own): "Going?" keeps No, as the guest list and the
  // struck-through chip do, through the next refresh that finds Google kept it.
  it("an all-events reply not read back yet keeps the occurrence's own answer in the popover", async () => {
    const declined = () => row({ ...invite("declined"), series_id: "s1", recurring: true });
    rows = [declined()];
    let synced: () => void = () => {};
    ipc.onCalendarSynced.mockImplementation((cb: () => void) => {
      synced = cb;
      return Promise.resolve(() => {});
    });
    ipc.respondToCalendarEvent.mockResolvedValue({
      outcome: "saved",
      warnings: ["mirror_refresh_pending"],
      left_behind: [],
    });
    ipc.getSeriesSummary.mockResolvedValue(null);
    await mount();
    fireEvent.click(chip());
    const going = () =>
      ["Yes", "No", "Maybe"].filter(
        (name) => screen.getByRole("button", { name }).getAttribute("aria-pressed") === "true",
      );
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "All events" })));
    expect(ipc.respondToCalendarEvent).toHaveBeenCalledWith("i1", "yes", null, "all");
    expect(going()).toEqual(["No"]);
    rows = [declined()];
    await act(async () => synced());
    expect(screen.getByRole("dialog", { name: "Standup" }).textContent).toContain("declined");
    expect(going()).toEqual(["No"]);
  });

  // C14 critiques C1-L7, C2-L7: the Terminal look's agenda (also its Day and Week) and month table.
  it("the Terminal look strikes it through too, and says so", async () => {
    env.system = "terminal";
    rows = [invite("declined")];
    await mount();
    const line = screen.getByRole("button", { name: /Standup, declined/ });
    expect(within(line).getByText("Standup").className).toContain("line-through");
    fireEvent.click(screen.getByRole("button", { name: "Month" }));
    const cell = await screen.findByRole("button", { name: /Standup, declined/ });
    expect(within(cell).getByText("Standup").className).toContain("line-through");
  });

  it("a reply that fails after the popover closed shows a notice", async () => {
    rows = [invite()];
    let answer: (o: WriteOutcome) => void = () => {};
    ipc.respondToCalendarEvent.mockReturnValue(new Promise<WriteOutcome>((r) => (answer = r)));
    await mount();
    fireEvent.click(chip());
    fireEvent.click(screen.getByRole("button", { name: "Maybe" }));
    press("Escape");
    expect(screen.queryByRole("dialog")).toBeNull();
    await act(async () => answer({ outcome: "busy" }));
    expect(
      screen.getByText(
        "Your reply to “Standup”: Google is busy right now. Reply again in a moment.",
      ),
    ).toBeTruthy();
  });
});

// Duplicate (#884, C18c): from the details, the original is read from Google afresh and the full
// editor opens on its copy; closing it puts focus back on the original's chip.
describe("CalendarView: Duplicate", () => {
  const chip = () => screen.getByRole("button", { name: /Standup/ });
  const original: EventForEdit = {
    summary: "Standup",
    location: "Room 2",
    description: "",
    time: {
      kind: "timed",
      start_date: today,
      start_time: "07:00",
      start_zone: "Europe/London",
      end_date: today,
      end_time: "07:30",
      end_zone: "Europe/London",
    },
    start_at: `${today}T06:00:00Z`,
    end_at: `${today}T06:30:00Z`,
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
    call: { kind: "meet", name: "Google Meet", join: "https://meet.google.com/x", state: "ready" },
    meet_offered: true,
  };
  const load: EditLoad = {
    outcome: "ready",
    session: "s1",
    event: original,
    permissions: {
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
    },
    seen: null,
    series: null,
  };

  it("opens the full editor on a copy, which says what it leaves out, and focus goes back to the original", async () => {
    // The original has a linked milestone, which stays with it.
    ipc.listAllMilestones.mockResolvedValue([{ ...milestone, event_uid: "u1" }]);
    ipc.getCalendarEventForEdit.mockResolvedValue(load);
    await mount();
    // A click focuses the chip, as the app's webviews do; the details hand focus back to it.
    act(() => chip().focus());
    fireEvent.click(chip());
    fireEvent.click(await screen.findByRole("button", { name: "Duplicate" }));
    const editor = await screen.findByRole("dialog", { name: "New event" });
    expect(ipc.getCalendarEventForEdit).toHaveBeenCalledOnce();
    expect(within(editor).getByText("A copy of “Standup”.")).toBeTruthy();
    expect(
      within(editor).getByText("The “Plant bulbs” milestone stays linked to the original."),
    ).toBeTruthy();
    // Personal offers no Meet here, so the note asks for no new call (review round 1 of PR 1).
    expect(within(editor).getByText(COPY_CALL_OFF_NOTE)).toBeTruthy();
    expect(within(editor).getByLabelText<HTMLInputElement>("Title").value).toBe("Standup");
    // On the original's calendar, which takes new events.
    expect(within(editor).getByRole("button", { name: "Calendar: Personal" })).toBeTruthy();
    fireEvent.click(within(editor).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog", { name: "New event" })).toBeNull();
    expect(ipc.createCalendarEvent).not.toHaveBeenCalled();
    // Critique C2-M9: the copy's editor opened from the copying dialog, which is gone.
    await waitFor(() => expect(document.activeElement).toBe(chip()));
  });

  it("is offered on a calendar that's only read, onto one that takes new events", async () => {
    rows = [row({ calendar_id: `${ME}:holidays` })];
    const withHolidays: CalendarOverview = {
      ...overview,
      calendars: [
        ...overview.calendars,
        calendar("holidays", { name: "Holidays", access_role: "reader" }),
      ],
    };
    ipc.calendarOverview.mockResolvedValue(withHolidays);
    ipc.getCalendarEventForEdit.mockResolvedValue(load);
    await mount();
    fireEvent.click(chip());
    fireEvent.click(await screen.findByRole("button", { name: "Duplicate" }));
    const editor = await screen.findByRole("dialog", { name: "New event" });
    // A reader calendar takes no new event: the copy starts where a new one would.
    expect(within(editor).getByRole("button", { name: "Calendar: Personal" })).toBeTruthy();
  });

  it("isn't offered without a calendar to put the copy on", async () => {
    ipc.calendarOverview.mockResolvedValue({ ...overview, editing: { [ME]: "off" } });
    await mount();
    fireEvent.click(chip());
    await screen.findByRole("dialog");
    expect(screen.queryByRole("button", { name: "Duplicate" })).toBeNull();
  });

  // Critique C1-L9, review round 1 of PR 1: a copy that starts on a calendar other than the
  // original's (its account has editing off) never invites that calendar to its own event: the
  // calendar organises it.
  it("a copy started on another account's calendar leaves that calendar off its guests", async () => {
    const OTHER = "gcal:other@x.com";
    const theirs: CalendarOverview = {
      ...overview,
      accounts: [...overview.accounts, { ...account, id: OTHER, email: "other@x.com" }],
      calendars: [
        ...overview.calendars,
        {
          ...calendar("other@x.com", { name: "Other", is_primary: true }),
          id: `${OTHER}:other@x.com`,
          source_id: OTHER,
        },
      ],
      editing: { [ME]: "off", [OTHER]: "on" },
    };
    const guest = { name: null, response: null, optional: false, room: false };
    const guests: GuestForEdit[] = [
      { ...guest, email: "me@x.com", organizer: true, self: true },
      { ...guest, email: "other@x.com", organizer: false, self: false },
      { ...guest, email: "b@x.org", organizer: false, self: false },
    ];
    ipc.calendarOverview.mockResolvedValue(theirs);
    ipc.getCalendarEventForEdit.mockResolvedValue({ ...load, event: { ...original, guests } });
    ipc.createCalendarEvent.mockResolvedValue({ outcome: "saved", warnings: [] });
    await mount();
    fireEvent.click(chip());
    fireEvent.click(await screen.findByRole("button", { name: "Duplicate" }));
    const editor = await screen.findByRole("dialog", { name: "New event" });
    expect(within(editor).getByRole("button", { name: "Calendar: Other" })).toBeTruthy();
    fireEvent.click(within(editor).getByRole("button", { name: "Save" }));
    const prompt = await screen.findByRole("dialog", { name: "Send invitation emails to guests?" });
    fireEvent.click(within(prompt).getByRole("radio", { name: /^Don't send/ }));
    fireEvent.click(within(prompt).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(ipc.createCalendarEvent).toHaveBeenCalled());
    const [on, sent] = ipc.createCalendarEvent.mock.calls[0] as [string, NewEventDraft];
    expect(on).toBe(`${OTHER}:other@x.com`);
    expect(sent.guests).toEqual([{ email: "b@x.org", optional: false }]);
  });
});

// Asking for edit access (#884, C20b): the details of an event on a calendar shared with the user to
// view only offer to ask its owner, whom the overview names; the email is the backend's, from PM's
// calendar id alone.
describe("CalendarView: asking a calendar's owner for edit access", () => {
  const SHARED = `${ME}:team@group.calendar.google.com`;
  const shared: CalendarOverview = {
    ...overview,
    calendars: [
      ...overview.calendars,
      calendar("team@group.calendar.google.com", { name: "Team", access_role: "reader" }),
    ],
    ask_access: { [SHARED]: { owner: "boss@x.org", connected: false } },
  };
  const chip = () => screen.getByRole("button", { name: /Standup/ });

  it("the popover of an event on a view-only calendar asks its owner", async () => {
    rows = [row({ calendar_id: SHARED, edit_block: "calendar_read_only", can_delete: false })];
    ipc.calendarOverview.mockResolvedValue(shared);
    ipc.requestCalendarAccess.mockResolvedValue({ outcome: "opened", owner: "boss@x.org" });
    await mount();
    fireEvent.click(chip());
    const dialog = await screen.findByRole("dialog", { name: "Standup" });
    expect(
      within(dialog).getByText(
        /^This calendar is shared with you to view only, so its events can't be changed here\./,
      ),
    ).toBeTruthy();
    await act(async () =>
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Ask boss@x.org for edit access" }),
      ),
    );
    // The calendar's id, and nothing the webview could make up: no address, no link.
    expect(ipc.requestCalendarAccess).toHaveBeenCalledExactlyOnceWith(SHARED);
    expect(within(dialog).getByRole("status").textContent).toBe(
      "PM asked your system to open an email to boss@x.org in your mail app. If nothing opened, write to boss@x.org and ask for “Make changes and see event details”.",
    );
  });

  it("no mail app names the calendar the event is on", async () => {
    rows = [row({ calendar_id: SHARED, edit_block: "calendar_read_only", can_delete: false })];
    ipc.calendarOverview.mockResolvedValue(shared);
    ipc.requestCalendarAccess.mockResolvedValue({ outcome: "no_mail_app", owner: "boss@x.org" });
    await mount();
    fireEvent.click(chip());
    const dialog = await screen.findByRole("dialog", { name: "Standup" });
    await act(async () =>
      fireEvent.click(
        within(dialog).getByRole("button", { name: "Ask boss@x.org for edit access" }),
      ),
    );
    expect(within(dialog).getByRole("status").textContent).toBe(
      "PM couldn't open your mail app. Write to boss@x.org and ask for “Make changes and see event details” on “Team”.",
    );
  });

  it("a connected owner gets how to share it from there, with this account's address", async () => {
    rows = [row({ calendar_id: SHARED, edit_block: "calendar_read_only", can_delete: false })];
    ipc.calendarOverview.mockResolvedValue({
      ...shared,
      ask_access: { [SHARED]: { owner: "boss@x.org", connected: true } },
    });
    await mount();
    fireEvent.click(chip());
    const dialog = await screen.findByRole("dialog", { name: "Standup" });
    expect(within(dialog).queryByRole("button", { name: /^Ask / })).toBeNull();
    expect(dialog.textContent).toContain(
      "boss@x.org is connected to PM too: open Google Calendar as boss@x.org and share this calendar with me@x.com, choosing “Make changes and see event details”.",
    );
  });

  it("offers nothing on a calendar the overview names no owner for", async () => {
    rows = [row({ calendar_id: SHARED, edit_block: "calendar_read_only", can_delete: false })];
    ipc.calendarOverview.mockResolvedValue({ ...shared, ask_access: {} });
    await mount();
    fireEvent.click(chip());
    const dialog = await screen.findByRole("dialog", { name: "Standup" });
    expect(within(dialog).queryByRole("button", { name: /^Ask / })).toBeNull();
    expect(within(dialog).queryAllByRole("status")).toHaveLength(0);
  });

  it("offers nothing on a calendar the overview doesn't list", async () => {
    rows = [row({ calendar_id: SHARED, edit_block: "calendar_read_only", can_delete: false })];
    ipc.calendarOverview.mockResolvedValue({ ...shared, calendars: overview.calendars });
    await mount();
    fireEvent.click(chip());
    const dialog = await screen.findByRole("dialog", { name: "Standup" });
    expect(within(dialog).queryByRole("button", { name: /^Ask / })).toBeNull();
  });
});
