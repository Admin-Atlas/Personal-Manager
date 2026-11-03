// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Runway board (PA-2): it draws the columns and their lanes with each card where the model puts
// it, ticks a task through the task IPC, moves one from its Move… menu (the keyboard's path) and by
// dragging it (a drop writes the plan, then the lane's order, and says where it went; Escape puts
// it back), opens a stack to show its tasks, and adds and edits named windows. It never writes a
// calendar: only `planTask`, `reorderTasks` and `changeTaskStatus` are offered here.
//
// IPC is mocked whole-module and `useTheme` is stubbed, as AgendaView.test.tsx does.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { formatClockIso, formatDateLocal } from "../../../lib/format";
import type { CalendarEvent, Milestone, Task, TaskStack } from "../../../lib/types";

vi.mock("../../../lib/ipc", () => ({
  planTask: vi.fn(async () => {}),
  reorderTasks: vi.fn(async () => {}),
  changeTaskStatus: vi.fn(async () => {}),
  // What the task editor reads and writes when it opens.
  addTask: vi.fn(async () => 1),
  addTaskStack: vi.fn(async () => 1),
  updateTask: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => []),
  listTaskStacks: vi.fn(async () => []),
}));

const env = vi.hoisted(() => ({ depth: "standard" }));
vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: env.depth, accent: "mono", colorblind: false }),
}));

import * as ipc from "../../../lib/ipc";
import { RunwayBoard } from "./RunwayBoard";
import { EDGE_SCROLL_MAX, edgeScrollStep } from "./useRunwayDrag";

// The strip prints the locale clock (12-hour on CI's en-US runners, 24-hour on an en-GB box), so the
// expected name is built from the same formatter rather than hard-coding 09:00.
const standupName = (name: string) =>
  name.replace(/\s+/g, "") ===
  `${formatClockIso("2026-11-04T09:00:00")}Standup`.replace(/\s+/g, "");

const mocked = vi.mocked(ipc);

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
  localStorage.clear();
  env.depth = "standard";
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

// Wednesday 04-11-2026: Today, Tomorrow (Thu 05), the rest of the week (06–08), next week (09–15).
const NOW = new Date(2026, 10, 4, 10, 0);

function task(over: Partial<Task> = {}): Task {
  return {
    id: 1,
    title: "Draft the deck",
    why: null,
    project_name: null,
    project_archived: false,
    milestone_id: null,
    event_uid: null,
    do_start: "2026-11-04",
    do_end: null,
    deadline: null,
    deadline_kind: "hard",
    deadline_from: "date",
    effective_deadline: null,
    deadline_missing: false,
    milestone_label: null,
    event_title: null,
    status: "todo",
    completed_at: null,
    lane: "foreground",
    stack_id: null,
    parked: false,
    ordinal: over.id ?? 1,
    source: "user",
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T00:00:00Z",
    ...over,
  };
}

const milestone: Milestone = {
  id: 1,
  project_name: "Atlas",
  label: "Pitch",
  due_date: "2026-11-05",
  event_uid: null,
  calendar_linked: false,
  event_missing: false,
  state: null,
  status: null,
  source_type: null,
  external_id: null,
  sort_order: 0,
};

const stack: TaskStack = {
  id: 7,
  title: "Visa",
  kind: "sequential",
  note: null,
  created_at: "",
  updated_at: "",
};

function board(
  over: {
    tasks?: Task[];
    stacks?: TaskStack[];
    milestones?: Milestone[];
    events?: CalendarEvent[];
    windows?: RunwayWindow[];
    archived?: string[];
  } = {},
) {
  const spies = {
    onWindowsChange: vi.fn(),
    onOpenProject: vi.fn(),
    onEventClick: vi.fn(),
    announce: vi.fn(),
  };
  render(
    <RunwayBoard
      tasks={over.tasks ?? [task()]}
      stacks={over.stacks ?? []}
      milestones={over.milestones ?? []}
      events={over.events ?? []}
      archived={new Set(over.archived ?? [])}
      windows={over.windows ?? []}
      now={NOW}
      {...spies}
    />,
  );
  return spies;
}

const column = (name: string) => screen.getByRole("region", { name });

describe("RunwayBoard", () => {
  it("draws the columns from today, each card where it belongs, and empty lanes", () => {
    board({
      tasks: [
        task({ id: 1 }),
        task({ id: 2, title: "Call the bank", do_start: "2026-11-05", lane: "background" }),
        task({ id: 3, title: "Someday", do_start: null }),
        task({ id: 4, title: "On the shelf", parked: true }),
        task({ id: 5, title: "Far off", do_start: "2026-12-01" }),
      ],
      milestones: [milestone],
    });
    expect(screen.getAllByRole("heading", { level: 3 }).map((h) => h.textContent)).toEqual([
      "Today",
      "Tomorrow",
      "Rest of this week",
      "Next week",
      "Later",
      "Parked",
    ]);
    expect(within(column("Today")).getByText("Draft the deck")).toBeTruthy();
    const tomorrow = column("Tomorrow");
    expect(within(tomorrow).getByText("Call the bank")).toBeTruthy();
    expect(within(tomorrow).getByRole("button", { name: "◆ Pitch · Atlas" })).toBeTruthy();
    expect(within(column("Later")).getByText("Far off")).toBeTruthy();
    // With no plan and no deadline, a task waits on the shelf with the parked ones, not in Later.
    const shelf = within(screen.getByRole("region", { name: "Parked" }));
    expect(shelf.getByText("On the shelf")).toBeTruthy();
    expect(shelf.getByText("Someday")).toBeTruthy();
    // Before today starts folded, as a rail.
    expect(screen.getByRole("button", { name: "Show Before today" })).toBeTruthy();
    expect(within(column("Next week")).getAllByText("empty")).toHaveLength(2);
    // The strip: progress and what's next.
    expect(screen.getByText("0/4 done")).toBeTruthy();
    expect(document.querySelector('[data-help="calendar-runway"]')).not.toBeNull();
  });

  it("hides every column's events on request, and remembers it on this device", () => {
    const standup: CalendarEvent = {
      id: "e1",
      calendar_id: "c",
      summary: "Standup",
      description: null,
      location: null,
      start: "2026-11-04",
      end: null,
      all_day: true,
      html_link: null,
      uid: "u1",
    };
    board({ events: [standup] });
    const strip = () => within(column("Today")).queryByRole("group", { name: "Events, Today" });
    expect(strip()).not.toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Hide events" }));
    expect(strip()).toBeNull();
    // The tasks stay where they were.
    expect(within(column("Today")).getByText("Draft the deck")).toBeTruthy();
    cleanup();

    board({ events: [standup] });
    expect(strip()).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show events" }));
    expect(strip()).not.toBeNull();
  });

  it("ticks a task through the task IPC", async () => {
    board();
    fireEvent.click(screen.getByRole("button", { name: "To do: mark as doing" }));
    await waitFor(() => expect(mocked.changeTaskStatus).toHaveBeenCalledWith(1, "doing"));
  });

  it("moves a task from its Move… menu, and says where it went", async () => {
    const { announce } = board();
    fireEvent.click(screen.getByRole("button", { name: "Move Draft the deck" }));
    const menu = screen.getByRole("group", { name: "Move Draft the deck" });
    // Where it is now isn't offered.
    expect(
      within(menu)
        .getByRole("button", { name: /^Today/ })
        .hasAttribute("disabled"),
    ).toBe(true);
    fireEvent.click(within(menu).getByRole("button", { name: /^Tomorrow/ }));
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-05", null, "foreground", false),
    );
    expect(announce).toHaveBeenCalledWith("Moved to Tomorrow");
  });

  it("moves to the other lane and parks from the menu, keeping the dates", async () => {
    board({ tasks: [task({ id: 1, do_end: "2026-11-06" })] });
    fireEvent.click(screen.getByRole("button", { name: "Move Draft the deck" }));
    fireEvent.click(screen.getByRole("button", { name: "Background lane" }));
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(
        1,
        "2026-11-04",
        "2026-11-06",
        "background",
        false,
      ),
    );
    fireEvent.click(screen.getByRole("button", { name: "Move Draft the deck" }));
    fireEvent.click(screen.getByRole("button", { name: "Park it" }));
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenLastCalledWith(
        1,
        "2026-11-04",
        "2026-11-06",
        "foreground",
        true,
      ),
    );
  });

  it("folds two tasks of a stack into one card that opens to show them", () => {
    board({
      tasks: [
        task({ id: 1, title: "Book the slot", stack_id: 7 }),
        task({ id: 2, title: "Fill the form", stack_id: 7 }),
      ],
      stacks: [stack],
    });
    const card = screen.getByRole("button", { name: /^≡ Visa\s*· 2 left/ });
    expect(card.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Fill the form")).toBeNull();
    fireEvent.click(card);
    expect(card.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("Book the slot")).toBeTruthy();
    expect(screen.getByText("Fill the form")).toBeTruthy();
  });

  it("a single task of a stack is a plain card", () => {
    board({ tasks: [task({ id: 1, stack_id: 7 })], stacks: [stack] });
    expect(screen.queryByRole("button", { name: /left/ })).toBeNull();
    expect(screen.getByText("Draft the deck")).toBeTruthy();
  });

  it("adds a named window from + Window", () => {
    const { onWindowsChange } = board();
    fireEvent.click(screen.getByRole("button", { name: "+ Window" }));
    const dialog = screen.getByRole("dialog", { name: "New window" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Lisbon" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add window" }));
    // No last day yet: it says so and saves nothing.
    expect(within(dialog).getByText("Pick its first and last day.")).toBeTruthy();
    expect(onWindowsChange).not.toHaveBeenCalled();
    const end = within(dialog).getByLabelText<HTMLInputElement>("Last day");
    end.focus();
    fireEvent.change(end, { target: { value: "12-11-2026" } });
    fireEvent.blur(end);
    fireEvent.click(within(dialog).getByRole("button", { name: "Add window" }));
    expect(onWindowsChange).toHaveBeenCalledTimes(1);
    const [saved] = onWindowsChange.mock.calls[0][0] as RunwayWindow[];
    expect(saved).toMatchObject({ label: "Lisbon", start: "2026-11-04", end: "2026-11-12" });
  });

  it("shows a window as its own column, and edits and deletes it from its head", () => {
    const trip: RunwayWindow = { id: "w1", label: "Trip", start: "2026-11-10", end: "2026-11-12" };
    const { onWindowsChange } = board({
      tasks: [task({ id: 1, do_start: "2026-11-11" })],
      windows: [trip],
    });
    expect(within(column("Trip")).getByText("Draft the deck")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit the window Trip" }));
    const dialog = screen.getByRole("dialog", { name: "Edit window" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Lisbon" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(onWindowsChange).toHaveBeenLastCalledWith([{ ...trip, label: "Lisbon" }]);

    fireEvent.click(screen.getByRole("button", { name: "Edit the window Trip" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete window" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "Delete this window?" })).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(onWindowsChange).toHaveBeenLastCalledWith([]);
  });

  it("opens a task's project and its linked event", () => {
    const ev: CalendarEvent = {
      id: "e1",
      calendar_id: "c",
      summary: "Review",
      description: null,
      location: null,
      start: "2026-11-06",
      end: null,
      all_day: true,
      html_link: null,
      uid: "u1",
    };
    const { onOpenProject, onEventClick } = board({
      tasks: [
        task({
          project_name: "Atlas",
          event_uid: "u1",
          deadline_from: "event",
          effective_deadline: "2026-11-06",
          event_title: "Review",
        }),
      ],
      events: [ev],
    });
    fireEvent.click(screen.getByRole("button", { name: "Atlas" }));
    expect(onOpenProject).toHaveBeenCalledWith("Atlas");
    fireEvent.click(screen.getByRole("button", { name: "Open the event Review" }));
    expect(onEventClick).toHaveBeenCalledWith(ev, expect.anything());
    // The deadline follows the event, and says so.
    expect(screen.getByText(/^due .* · Review$/)).toBeTruthy();
  });

  it("opens the shared task editor from ✎ and from + Task", async () => {
    board();
    fireEvent.click(screen.getByRole("button", { name: "Edit Draft the deck" }));
    expect(await screen.findByRole("dialog", { name: "Edit task" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.click(screen.getByRole("button", { name: "+ Task" }));
    expect(await screen.findByRole("dialog", { name: "New task" })).toBeTruthy();
  });

  it("heads each column with its events and pinboard entries, read-only, opening their details", () => {
    const ev = (id: string, start: string, over: Partial<CalendarEvent> = {}): CalendarEvent => ({
      id,
      calendar_id: "c",
      summary: id,
      description: null,
      location: null,
      start,
      end: null,
      all_day: !start.includes("T"),
      html_link: null,
      uid: id,
      ...over,
    });
    const standup = ev("Standup", "2026-11-04T09:00:00");
    const later = Array.from({ length: 12 }, (_, i) =>
      ev(`Later ${i + 1}`, `2026-12-${String(i + 1).padStart(2, "0")}`),
    );
    const onShowList = vi.fn();
    const spies = {
      onWindowsChange: vi.fn(),
      onEventClick: vi.fn(),
      announce: vi.fn(),
    };
    render(
      <RunwayBoard
        tasks={[]}
        stacks={[]}
        milestones={[]}
        events={[
          standup,
          ev("Bulbs", "2026-11-05", { calendar_id: "pm:pinboard", uid: null }),
          ev("Old", "2026-11-02T09:00:00"),
          ...later,
        ]}
        archived={new Set()}
        windows={[]}
        now={NOW}
        colorOf={() => "var(--ink3)"}
        onShowList={onShowList}
        {...spies}
      />,
    );
    const today = within(column("Today")).getByRole("group", { name: "Events, Today" });
    fireEvent.click(within(today).getByRole("button", { name: standupName }));
    expect(spies.onEventClick).toHaveBeenCalledWith(standup, expect.anything());
    // A pinboard entry rides along, on its day.
    expect(
      within(column("Tomorrow")).getByRole("button", { name: /all-day\s*Bulbs/ }),
    ).toBeTruthy();
    // Nothing for a column with none, nor for Before today.
    expect(within(column("Next week")).queryByRole("group", { name: /^Events/ })).toBeNull();
    expect(screen.queryByText("Old")).toBeNull();
    // Later: the next ten, then the rest are in the List.
    const laterStrip = within(column("Later")).getByRole("group", { name: "Events, Later" });
    expect(within(laterStrip).getAllByRole("listitem")).toHaveLength(10);
    fireEvent.click(within(laterStrip).getByRole("button", { name: "+2 more in List" }));
    expect(onShowList).toHaveBeenCalled();
  });

  it("shows a milestone's date in its tone wherever it sits", () => {
    board({
      tasks: [],
      milestones: [
        milestone,
        { ...milestone, id: 2, label: "Launch", due_date: "2026-11-20" },
        { ...milestone, id: 3, label: "Late", due_date: "2026-11-01" },
      ],
    });
    const tomorrow = within(column("Tomorrow")).getByText("tomorrow");
    expect(tomorrow.className).toContain("text-st-look");
    expect(within(column("Later")).getByText(formatDateLocal(new Date(2026, 10, 20)))).toBeTruthy();
    expect(within(column("Today")).getByText("3d late").className).toContain("text-st-due");
  });

  it("says 'planned' only for a task whose plan rolled, not a missed deadline", () => {
    board({
      tasks: [
        task({ id: 1, title: "Planned Monday", do_start: "2026-11-02" }),
        task({ id: 2, title: "Due Friday", do_start: null, effective_deadline: "2026-10-30" }),
      ],
    });
    const today = column("Today");
    expect(within(today).getByText("planned 2d ago")).toBeTruthy();
    expect(within(today).queryByText(/planned 5d ago/)).toBeNull();
    // The overdue deadline says it.
    expect(within(today).getByText("5d late")).toBeTruthy();
  });

  it("opens a stack to its whole chain, where each is planned, and its note", () => {
    board({
      tasks: [
        task({ id: 1, title: "Book the slot", stack_id: 7 }),
        task({ id: 2, title: "Fill the form", stack_id: 7 }),
        task({ id: 3, title: "Post it", stack_id: 7, do_start: "2026-11-10" }),
        task({ id: 4, title: "Pick up", stack_id: 7, parked: true }),
      ],
      stacks: [{ ...stack, note: "ref 4471" }],
    });
    // Four left over the whole stack, though two sit here.
    const card = within(column("Today")).getByRole("button", { name: /^≡ Visa\s*· 4 left/ });
    fireEvent.click(card);
    const chain = within(column("Today")).getByRole("list", { name: "The stack Visa" });
    const rows = within(chain).getAllByRole("listitem");
    expect(rows.map((r) => within(r).getByText(/^(Book|Fill|Post|Pick)/).textContent)).toEqual([
      "Book the slot",
      "Fill the form",
      "Post it",
      "Pick up",
    ]);
    expect(within(rows[0]).getByText("Today")).toBeTruthy();
    expect(within(rows[2]).getByText("Next week")).toBeTruthy();
    expect(within(rows[3]).getByText("Parked")).toBeTruthy();
    expect(within(column("Today")).getByText("ref 4471")).toBeTruthy();
  });

  it("moves a task up or down its lane from the Move… menu", async () => {
    const { announce } = board({
      tasks: [task({ id: 1 }), task({ id: 2, title: "Call the bank" })],
    });
    fireEvent.click(screen.getByRole("button", { name: "Move Call the bank" }));
    let menu = screen.getByRole("group", { name: "Move Call the bank" });
    expect(within(menu).getByRole("button", { name: "Move down" }).hasAttribute("disabled")).toBe(
      true,
    );
    fireEvent.click(within(menu).getByRole("button", { name: "Move up" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 1]));
    expect(announce).toHaveBeenCalledWith("Moved up in Today");
    expect(mocked.planTask).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Move Draft the deck" }));
    menu = screen.getByRole("group", { name: "Move Draft the deck" });
    expect(within(menu).getByRole("button", { name: "Move up" }).hasAttribute("disabled")).toBe(
      true,
    );
    fireEvent.click(within(menu).getByRole("button", { name: "Move down" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenLastCalledWith([2, 1]));
    expect(announce).toHaveBeenLastCalledWith("Moved down in Today");
  });
});

describe("RunwayBoard: dragging", () => {
  /** Lay the zones out by geometry: each lane gets a box (and a card, by its key, may too); the
   *  board's own visible box is `root`; everything else sits off screen. */
  function layOut(
    boxes: Record<string, [number, number, number, number]>,
    root: [number, number, number, number] = [-1000, -1000, 1000, 1000],
  ) {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      const zone = this.dataset.runwayZone;
      const key = this.dataset.runwayKey;
      const isRoot = this.dataset.help === "calendar-runway";
      const [left, top, right, bottom] = (isRoot
        ? root
        : zone
          ? boxes[zone]
          : key
            ? boxes[key]
            : undefined) ?? [-999, -999, -998, -998];
      return {
        left,
        top,
        right,
        bottom,
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
        toJSON: () => ({}),
      } as DOMRect;
    });
  }
  const at = (x: number, y: number) => ({
    pointerId: 1,
    pointerType: "mouse",
    // jsdom's pointer events default to a secondary pointer; a mouse's is the primary one.
    isPrimary: true,
    button: 0,
    buttons: 1,
    clientX: x,
    clientY: y,
  });
  const card = (title: string) => screen.getByText(title).closest("li")!;

  it("a drop on another column's lane writes the plan, then the order, and says where", async () => {
    const { announce } = board({
      tasks: [task({ id: 1 }), task({ id: 2, title: "Call the bank", do_start: "2026-11-05" })],
    });
    layOut({ "tomorrow:background": [0, 100, 200, 200] });
    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    // Under the threshold: still a press.
    fireEvent.pointerMove(document.body, at(12, 11));
    expect(announce).not.toHaveBeenCalled();
    fireEvent.pointerMove(document.body, at(50, 150));
    expect(announce).toHaveBeenCalledWith("Moving Draft the deck");
    expect(document.querySelector("[data-runway-ghost]")).not.toBeNull();
    fireEvent.pointerUp(document.body, { ...at(50, 150), buttons: 0 });
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-05", null, "background", false),
    );
    expect(announce).toHaveBeenCalledWith("Moved to Tomorrow, background");
    // A lane of one needs no order.
    expect(mocked.reorderTasks).not.toHaveBeenCalled();
  });

  it("a drop into a lane with tasks puts it in place there", async () => {
    board({
      tasks: [
        task({ id: 1 }),
        task({ id: 2, title: "Call the bank", do_start: "2026-11-05" }),
        task({ id: 3, title: "Pay the bill", do_start: "2026-11-05" }),
      ],
    });
    layOut({ "tomorrow:foreground": [0, 100, 200, 300] });
    // Below both of tomorrow's cards (their boxes sit off screen, above): last.
    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    fireEvent.pointerMove(document.body, at(50, 150));
    fireEvent.pointerUp(document.body, { ...at(50, 150), buttons: 0 });
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 3, 1]));
    expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-05", null, "foreground", false);
  });

  it("drops onto the parked shelf, keeping the dates", async () => {
    board();
    layOut({ parked: [0, 500, 400, 600] });
    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    fireEvent.pointerMove(document.body, at(50, 550));
    fireEvent.pointerUp(document.body, { ...at(50, 550), buttons: 0 });
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-04", null, "foreground", true),
    );
  });

  it("Escape puts it back with nothing written; a drop on no zone too", async () => {
    const { announce } = board();
    layOut({ "tomorrow:foreground": [0, 100, 200, 200] });
    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    fireEvent.pointerMove(document.body, at(50, 150));
    act(() => void fireEvent.keyDown(window, { key: "Escape" }));
    expect(announce).toHaveBeenLastCalledWith("Put back.");
    fireEvent.pointerUp(document.body, { ...at(50, 150), buttons: 0 });

    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    fireEvent.pointerMove(document.body, at(900, 900));
    fireEvent.pointerUp(document.body, { ...at(900, 900), buttons: 0 });
    expect(announce).toHaveBeenLastCalledWith("Put back.");
    await act(async () => {});
    expect(mocked.planTask).not.toHaveBeenCalled();
  });

  it("a click on a card is still a click", async () => {
    // The click a drag's release swallows is let go on the next tick: let the last test's go.
    await act(() => new Promise((r) => setTimeout(r, 0)));
    board({ tasks: [task({ why: "For Friday's call" })] });
    fireEvent.pointerDown(card("Draft the deck"), at(10, 10));
    fireEvent.pointerUp(document.body, { ...at(10, 10), buttons: 0 });
    fireEvent.click(screen.getByRole("button", { name: "Draft the deck" }));
    expect(screen.getByText("For Friday's call")).toBeTruthy();
    expect(mocked.planTask).not.toHaveBeenCalled();
  });

  it("never drags a milestone", () => {
    const { announce } = board({ tasks: [], milestones: [milestone] });
    layOut({ "today:foreground": [0, 100, 200, 200] });
    const m = screen.getByRole("button", { name: "◆ Pitch · Atlas" }).closest("li")!;
    fireEvent.pointerDown(m, at(10, 10));
    fireEvent.pointerMove(document.body, at(50, 150));
    fireEvent.pointerUp(document.body, { ...at(50, 150), buttons: 0 });
    expect(announce).not.toHaveBeenCalled();
    expect(mocked.planTask).not.toHaveBeenCalled();
  });

  it("a reorder inside its own column keeps the task's days", async () => {
    // Friday and Saturday, both in the rest of the week (Fri 06 – Sun 08).
    board({
      tasks: [
        task({ id: 1, title: "Sat thing", do_start: "2026-11-07" }),
        task({ id: 2, title: "Fri thing", do_start: "2026-11-06" }),
        task({ id: 3, title: "Window", do_start: "2026-11-06", do_end: "2026-11-08" }),
      ],
    });
    layOut({
      "rest:foreground": [0, 100, 200, 300],
      "task:1": [0, 110, 200, 130],
      "task:2": [0, 140, 200, 160],
      "task:3": [0, 170, 200, 190],
    });
    // Saturday's card dragged below Friday's: the order changes, the days don't.
    fireEvent.pointerDown(card("Sat thing"), at(10, 120));
    fireEvent.pointerMove(document.body, at(50, 165));
    fireEvent.pointerUp(document.body, { ...at(50, 165), buttons: 0 });
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 1, 3]));
    expect(mocked.planTask).not.toHaveBeenCalled();
    await act(() => new Promise((r) => setTimeout(r, 0)));

    // The window task dragged to the background lane there: the lane alone changes.
    layOut({ "rest:background": [0, 400, 200, 500] });
    fireEvent.pointerDown(card("Window"), at(10, 10));
    fireEvent.pointerMove(document.body, at(50, 450));
    fireEvent.pointerUp(document.body, { ...at(50, 450), buttons: 0 });
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(
        3,
        "2026-11-06",
        "2026-11-08",
        "background",
        false,
      ),
    );
  });

  it("drops between two parked cards of a row by where across the row it is", async () => {
    board({
      tasks: [
        task({ id: 1, title: "Mover" }),
        task({ id: 2, title: "P one", parked: true }),
        task({ id: 3, title: "P two", parked: true }),
        task({ id: 4, title: "P three", parked: true }),
      ],
    });
    // One row of three on the shelf's grid.
    layOut({
      parked: [0, 500, 400, 600],
      "task:2": [0, 520, 100, 580],
      "task:3": [110, 520, 210, 580],
      "task:4": [220, 520, 320, 580],
    });
    fireEvent.pointerDown(card("Mover"), at(10, 10));
    // Right of the first card's middle, left of the second's: between them.
    fireEvent.pointerMove(document.body, at(105, 560));
    // No line in the grid's flow; the second card is marked on its leading edge instead.
    const shelf = screen.getByRole("region", { name: "Parked" });
    expect(shelf.querySelector("[data-runway-ghost]")).toBeNull();
    expect(card("P two").getAttribute("data-runway-mark")).toBe("before");
    fireEvent.pointerMove(document.body, at(390, 560));
    expect(card("P three").getAttribute("data-runway-mark")).toBe("after");
    fireEvent.pointerMove(document.body, at(105, 560));
    fireEvent.pointerUp(document.body, { ...at(105, 560), buttons: 0 });
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 1, 3, 4]));
    expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-04", null, "foreground", true);
  });

  it("takes no drop outside the board's visible box", async () => {
    const { announce } = board();
    // Tomorrow's lane is scrolled up under the header, above the board's top edge at y=200.
    layOut({ "tomorrow:foreground": [0, 0, 200, 300] }, [0, 200, 1000, 800]);
    fireEvent.pointerDown(card("Draft the deck"), at(10, 400));
    fireEvent.pointerMove(document.body, at(50, 100));
    fireEvent.pointerUp(document.body, { ...at(50, 100), buttons: 0 });
    expect(announce).toHaveBeenLastCalledWith("Put back.");
    await act(async () => {});
    expect(mocked.planTask).not.toHaveBeenCalled();
  });

  it("scrolls the board near its top and bottom edges while dragging", () => {
    const box = { left: 0, right: 400, top: 100, bottom: 700, height: 600 } as DOMRect;
    expect(edgeScrollStep(200, 400, box)).toBe(0);
    expect(edgeScrollStep(200, 110, box)).toBeLessThan(0);
    expect(edgeScrollStep(200, 690, box)).toBeGreaterThan(0);
    // Past the edge (over the header): full speed; nearer the edge, faster.
    expect(edgeScrollStep(200, 20, box)).toBe(-EDGE_SCROLL_MAX);
    expect(Math.abs(edgeScrollStep(200, 105, box))).toBeGreaterThan(
      Math.abs(edgeScrollStep(200, 140, box)),
    );
    // Off to the side: nothing.
    expect(edgeScrollStep(500, 20, box)).toBe(0);
  });
});
