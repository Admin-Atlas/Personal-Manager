// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal Runway (PA-2): the same columns and cards as the board, as mono text. `── Today ──`
// rules with `fg`/`bg` sub-heads, `[ ]`/`[~]`/`[x]` ticks, accent on today alone, every column
// folding from its rule, each column's events under it, and no pointer drag: a task moves (and
// moves up or down its lane) from its Move… menu, a stack opens to its whole chain, and windows are
// added and edited from the same dialog as the board's.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { RunwayWindow } from "../../../lib/calendarPrefs";
import { formatClockIso } from "../../../lib/format";
import type { CalendarEvent, Task, TaskStack } from "../../../lib/types";

vi.mock("../../../lib/ipc", () => ({
  planTask: vi.fn(async () => {}),
  reorderTasks: vi.fn(async () => {}),
  changeTaskStatus: vi.fn(async () => {}),
  addTask: vi.fn(async () => 1),
  addTaskStack: vi.fn(async () => 1),
  updateTask: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => []),
  listTaskStacks: vi.fn(async () => []),
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "terminal", depth: "standard", accent: "mono", colorblind: false }),
}));

import * as ipc from "../../../lib/ipc";
import { TerminalRunway } from "./TerminalRunway";

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

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  localStorage.clear();
});

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

function runway(over: { tasks?: Task[]; stacks?: TaskStack[]; windows?: RunwayWindow[] } = {}) {
  const spies = { onWindowsChange: vi.fn(), announce: vi.fn(), onOpenProject: vi.fn() };
  render(
    <TerminalRunway
      tasks={over.tasks ?? [task()]}
      stacks={over.stacks ?? []}
      milestones={[]}
      events={[]}
      archived={new Set()}
      windows={over.windows ?? []}
      now={NOW}
      {...spies}
    />,
  );
  return spies;
}

describe("TerminalRunway", () => {
  it("draws mono column rules with fg/bg lanes, the tick as text, and today in accent", () => {
    runway({ tasks: [task(), task({ id: 2, title: "Done one", status: "done" })] });
    const today = screen.getByRole("button", { name: "── Today (2) ▾ ──" });
    expect(today.className).toContain("text-accent-text");
    expect(screen.getByRole("button", { name: "── Tomorrow (0) ▾ ──" }).className).not.toContain(
      "text-accent-text",
    );
    const section = screen.getByRole("region", { name: "Today" });
    expect(within(section).getAllByText("fg").length).toBe(1);
    expect(within(section).getByText("bg")).toBeTruthy();
    expect(within(section).getByText("[ ]")).toBeTruthy();
    expect(within(section).getByText("[x]")).toBeTruthy();
    // Before today starts folded.
    expect(screen.getByRole("button", { name: /── Before today \(0\) ▸ ──/ })).toBeTruthy();
    expect(document.querySelector('[data-help="calendar-runway"]')).not.toBeNull();
  });

  it("has no pointer drag: nothing is marked to drag or drop on", () => {
    runway();
    expect(document.querySelector("[data-runway-zone]")).toBeNull();
    expect(document.querySelector("[data-runway-card]")).toBeNull();
  });

  it("ticks a task, and moves it from its Move… menu", async () => {
    const { announce } = runway();
    fireEvent.click(screen.getByRole("button", { name: "To do: mark as doing" }));
    await waitFor(() => expect(mocked.changeTaskStatus).toHaveBeenCalledWith(1, "doing"));
    fireEvent.click(screen.getByRole("button", { name: "Move Draft the deck" }));
    fireEvent.click(screen.getByRole("button", { name: /^Next week/ }));
    await waitFor(() =>
      expect(mocked.planTask).toHaveBeenCalledWith(1, "2026-11-09", null, "foreground", false),
    );
    expect(announce).toHaveBeenCalledWith("Moved to Next week");
  });

  it("opens a stack to show its tasks", () => {
    runway({
      tasks: [
        task({ id: 1, title: "Book the slot", stack_id: 7 }),
        task({ id: 2, title: "Fill the form", stack_id: 7 }),
      ],
      stacks: [
        { id: 7, title: "Visa", kind: "sequential", note: null, created_at: "", updated_at: "" },
      ],
    });
    const stack = screen.getByRole("button", { name: /≡ Visa \(2 left\)/ });
    expect(screen.queryByText("Book the slot")).toBeNull();
    fireEvent.click(stack);
    expect(screen.getByText("Book the slot")).toBeTruthy();
    expect(screen.getByText("Fill the form")).toBeTruthy();
  });

  it("adds a window from the same dialog as the board's", () => {
    const { onWindowsChange } = runway();
    // Terminal's buttons read `[ + Window ]`.
    fireEvent.click(screen.getByRole("button", { name: /\+ Window/ }));
    const dialog = screen.getByRole("dialog", { name: "New window" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "Sprint" } });
    const end = within(dialog).getByLabelText<HTMLInputElement>("Last day");
    end.focus();
    fireEvent.change(end, { target: { value: "06-11-2026" } });
    fireEvent.blur(end);
    fireEvent.click(within(dialog).getByRole("button", { name: /Add window/ }));
    const [saved] = onWindowsChange.mock.calls[0][0] as RunwayWindow[];
    expect(saved).toMatchObject({ label: "Sprint", start: "2026-11-04", end: "2026-11-06" });
  });

  it("folds any column from its rule, not only Before today", () => {
    runway({ tasks: [task({ do_start: null, title: "Someday" })] });
    const later = screen.getByRole("button", { name: "── Later (1) ▾ ──" });
    expect(later.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText("Someday")).toBeTruthy();
    fireEvent.click(later);
    expect(screen.queryByText("Someday")).toBeNull();
    expect(
      screen.getByRole("button", { name: "── Later (1) ▸ ──" }).getAttribute("aria-expanded"),
    ).toBe("false");
    fireEvent.click(screen.getByRole("button", { name: "── Later (1) ▸ ──" }));
    expect(screen.getByText("Someday")).toBeTruthy();
  });

  it("lists each column's events under its rule, and a click opens one", () => {
    const standup: CalendarEvent = {
      id: "e1",
      calendar_id: "c",
      summary: "Standup",
      description: null,
      location: null,
      start: "2026-11-04T09:00:00",
      end: null,
      all_day: false,
      html_link: null,
      uid: "u1",
    };
    const onEventClick = vi.fn();
    render(
      <TerminalRunway
        tasks={[]}
        stacks={[]}
        milestones={[
          {
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
          },
        ]}
        events={[standup]}
        archived={new Set()}
        windows={[]}
        now={NOW}
        onWindowsChange={vi.fn()}
        onEventClick={onEventClick}
        announce={vi.fn()}
      />,
    );
    const strip = within(screen.getByRole("region", { name: "Today" })).getByRole("group", {
      name: "Events, Today",
    });
    fireEvent.click(within(strip).getByRole("button", { name: standupName }));
    expect(onEventClick).toHaveBeenCalledWith(standup, expect.anything());
    // A milestone shows its date in its tone, not only once it's late.
    const tomorrow = within(screen.getByRole("region", { name: "Tomorrow" })).getByText("tomorrow");
    expect(tomorrow.className).toContain("text-st-look");
  });

  it("moves a task up its lane from the Move… menu, the Terminal's way to reorder", async () => {
    const { announce } = runway({
      tasks: [task({ id: 1 }), task({ id: 2, title: "Call the bank" })],
    });
    fireEvent.click(screen.getByRole("button", { name: "Move Call the bank" }));
    fireEvent.click(screen.getByRole("button", { name: "Move up" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 1]));
    expect(announce).toHaveBeenCalledWith("Moved up in Today");
  });

  it("names Before today when a done task moves up there", async () => {
    const done = { status: "done" as const, completed_at: "2026-11-02T10:00:00Z" };
    const { announce } = runway({
      tasks: [
        task({ id: 1, do_start: "2026-11-02", ...done }),
        task({ id: 2, title: "Call the bank", do_start: "2026-11-02", ...done }),
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: /── Before today \(2\) ▸ ──/ }));
    fireEvent.click(screen.getByRole("button", { name: "Move Call the bank" }));
    fireEvent.click(screen.getByRole("button", { name: "Move up" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([2, 1]));
    expect(announce).toHaveBeenCalledWith(expect.stringMatching(/^Moved up in Before today/));
  });

  it("opens a stack to its whole chain, saying where each is planned", () => {
    runway({
      tasks: [
        task({ id: 1, title: "Book the slot", stack_id: 7 }),
        task({ id: 2, title: "Fill the form", stack_id: 7 }),
        task({ id: 3, title: "Post it", stack_id: 7, do_start: "2026-11-10" }),
      ],
      stacks: [
        {
          id: 7,
          title: "Visa",
          kind: "sequential",
          note: "ref 4471",
          created_at: "",
          updated_at: "",
        },
      ],
    });
    const today = screen.getByRole("region", { name: "Today" });
    fireEvent.click(within(today).getByRole("button", { name: /≡ Visa \(3 left\)/ }));
    const chain = within(today).getByRole("list", { name: "The stack Visa" });
    expect(within(chain).getByText("Post it")).toBeTruthy();
    expect(within(chain).getByText("Next week")).toBeTruthy();
    expect(within(today).getByText("ref 4471")).toBeTruthy();
  });

  it("shows a window as its own rule, with an edit beside it", () => {
    runway({
      tasks: [task({ do_start: "2026-11-11" })],
      windows: [{ id: "w1", label: "Trip", start: "2026-11-10", end: "2026-11-12" }],
    });
    const trip = screen.getByRole("region", { name: "Trip" });
    expect(within(trip).getByText("Draft the deck")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Edit the window Trip" }));
    expect(screen.getByRole("dialog", { name: "Edit window" })).toBeTruthy();
  });
});
