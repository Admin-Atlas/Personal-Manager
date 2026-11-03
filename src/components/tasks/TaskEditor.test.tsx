// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The task editor writes what the backend will accept: links stay independent, a deadline can only
// follow a link that's set (and falls back to the own date when that link is gone), an event
// deadline pins the occurrence that was picked, a saved link reads as the backend resolved it, a
// new stack is made before the task that joins it, a stack can be edited or deleted from its row,
// and a refused save says why and keeps the dialog open.
//
// IPC is mocked whole-module, as MilestoneList.test.tsx does, and `useTheme` is stubbed so the
// primitives render without the ThemeProvider (which pulls in IPC).

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { addDays, dayKey } from "../../lib/calendar-layout";
import { formatDateLocal, formatDateOnly } from "../../lib/format";
import type { CalendarEvent, Milestone, Task } from "../../lib/types";
import { TaskEditor } from "./TaskEditor";

vi.mock("../../lib/ipc", () => ({
  addTask: vi.fn(async () => 42),
  addTaskStack: vi.fn(async () => 9),
  changeTaskStatus: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => ["Atlas", "Home"]),
  listTaskStacks: vi.fn(async () => []),
  updateTask: vi.fn(async () => {}),
  updateTaskStack: vi.fn(async () => {}),
  deleteTaskStack: vi.fn(async () => {}),
}));

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth: "standard",
    autoLocation: "",
    teachVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import * as ipc from "../../lib/ipc";

const mocked = vi.mocked(ipc);

function milestone(over: Partial<Milestone> = {}): Milestone {
  return {
    id: 5,
    project_name: "Atlas",
    label: "pitch",
    due_date: "2026-12-04",
    event_uid: null,
    calendar_linked: false,
    event_missing: false,
    state: "unmet",
    status: "not_started",
    source_type: null,
    external_id: null,
    sort_order: 0,
    ...over,
  };
}

function task(over: Partial<Task> = {}): Task {
  return {
    id: 1,
    title: "Write the pitch",
    why: null,
    project_name: "Atlas",
    project_archived: false,
    milestone_id: null,
    event_uid: null,
    do_start: null,
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
    ordinal: 0,
    source: "user",
    created_at: "",
    updated_at: "",
    ...over,
  };
}

function calEvent(over: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id: "e1",
    calendar_id: "c",
    summary: "Demo day",
    description: null,
    location: null,
    start: dayKey(new Date()),
    end: null,
    all_day: true,
    html_link: null,
    uid: "uid-demo",
    ...over,
  };
}

const segment = (name: string) => screen.getByRole("button", { name });

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listAllMilestones.mockResolvedValue([]);
  mocked.listAllCalendarEvents.mockResolvedValue([]);
  mocked.listTaskStacks.mockResolvedValue([]);
});
afterEach(cleanup);

describe("TaskEditor", () => {
  it("adds a new task with the defaults it was opened with", async () => {
    const onSaved = vi.fn();
    const onClose = vi.fn();
    render(
      <TaskEditor
        open
        defaults={{ project: "Home", doStart: "2026-11-09" }}
        onClose={onClose}
        onSaved={onSaved}
      />,
    );
    expect(screen.getByRole("dialog", { name: "New task" })).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "  Book the hall  " } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(42));
    expect(mocked.addTask).toHaveBeenCalledWith(
      expect.objectContaining({
        title: "Book the hall",
        project: "Home",
        doStart: "2026-11-09",
        doEnd: null,
        deadline: null,
        deadlineFrom: "date",
        milestoneId: null,
        eventUid: null,
      }),
    );
    expect(onClose).toHaveBeenCalled();
  });

  it("only lets the deadline follow a link once that link is set", async () => {
    mocked.listAllMilestones.mockResolvedValue([milestone()]);
    render(<TaskEditor open onClose={() => {}} />);
    expect(segment("Milestone's date").hasAttribute("disabled")).toBe(true);
    expect(segment("Event's date").hasAttribute("disabled")).toBe(true);

    // Picking a milestone with no project brings its project along.
    const select = screen.getByLabelText("Milestone");
    await waitFor(() => expect(within(select).getByRole("option", { name: /pitch/ })).toBeTruthy());
    expect(within(select).getByRole("option", { name: /^Atlas · pitch/ })).toBeTruthy();
    fireEvent.change(select, { target: { value: "5" } });
    expect((screen.getByLabelText("Project") as HTMLInputElement).value).toBe("Atlas");

    fireEvent.click(segment("Milestone's date"));
    expect(screen.getByText(/◆ pitch: 04-12/)).toBeTruthy();
    // The own-date controls give way to the resolved date.
    expect(screen.queryByRole("button", { name: "Hard deadline" })).toBeNull();

    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Slides" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    expect(mocked.addTask).toHaveBeenCalledWith(
      expect.objectContaining({ project: "Atlas", milestoneId: 5, deadlineFrom: "milestone" }),
    );
  });

  it("clearing a link the deadline follows puts it back on the own date", async () => {
    mocked.listAllMilestones.mockResolvedValue([milestone()]);
    render(
      <TaskEditor
        open
        task={task({ milestone_id: 5, deadline_from: "milestone", milestone_label: "pitch" })}
        onClose={() => {}}
      />,
    );
    await waitFor(() => expect(screen.getByText(/◆ pitch/)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Clear the milestone" }));
    expect(segment("My own date").getAttribute("aria-pressed")).toBe("true");
    expect(segment("Milestone's date").hasAttribute("disabled")).toBe(true);
  });

  it("links an event through the picker and can take its date as the deadline", async () => {
    const friday = addDays(new Date(), 3);
    const ev: CalendarEvent = {
      id: "e1",
      calendar_id: "c",
      summary: "Demo day",
      description: null,
      location: null,
      start: dayKey(friday),
      end: null,
      all_day: true,
      html_link: null,
      uid: "uid-demo",
    };
    mocked.listAllCalendarEvents.mockResolvedValue([ev]);
    render(<TaskEditor open onClose={() => {}} />);
    const box = await screen.findByRole("combobox", { name: "Calendar event" });
    await waitFor(() => expect(box.hasAttribute("disabled")).toBe(false));
    fireEvent.focus(box);
    fireEvent.change(box, { target: { value: "demo" } });
    fireEvent.mouseDown(screen.getByRole("option", { name: /Demo day/ }));
    expect(screen.queryByRole("combobox", { name: "Calendar event" })).toBeNull();

    fireEvent.click(segment("Event's date"));
    // The picker shows the link, and the Deadline row now shows the event's date.
    expect(screen.getAllByText(/Demo day/).some((el) => el.tagName === "P")).toBe(true);
    expect(screen.getByText(`📅 Demo day: ${formatDateLocal(friday)}`)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Prep demo" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    // The picked occurrence's day is written as the pin.
    expect(mocked.addTask).toHaveBeenCalledWith(
      expect.objectContaining({
        eventUid: "uid-demo",
        deadlineFrom: "event",
        deadline: dayKey(friday),
      }),
    );
  });

  it("keeps a recurring event's pinned occurrence after it has passed", async () => {
    // Pinned to last Friday's review; the mirror's next one is this coming Friday.
    const lastFriday = dayKey(addDays(new Date(), -4));
    const nextFriday = addDays(new Date(), 3);
    mocked.listAllCalendarEvents.mockResolvedValue([
      calEvent({ summary: "Friday review", start: dayKey(nextFriday), uid: "uid-review" }),
    ]);
    render(
      <TaskEditor
        open
        task={task({
          event_uid: "uid-review",
          event_title: "Friday review",
          deadline_from: "event",
          deadline: lastFriday,
          effective_deadline: lastFriday,
        })}
        onClose={() => {}}
      />,
    );
    const shown = `${formatDateOnly(lastFriday)} · Friday review`;
    await waitFor(() => expect(screen.getByText(`📅 ${shown}`)).toBeTruthy());
    expect(screen.getByText(`📅 Friday review: ${formatDateOnly(lastFriday)}`)).toBeTruthy();
    expect(screen.queryByText(new RegExp(formatDateLocal(nextFriday)))).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalled());
    expect(mocked.updateTask).toHaveBeenCalledWith(
      1,
      expect.objectContaining({ deadlineFrom: "event", deadline: lastFriday }),
    );
  });

  it("pins the shown occurrence when switching to the event's date, and keeps the own date", async () => {
    const soon = addDays(new Date(), 3);
    mocked.listAllCalendarEvents.mockResolvedValue([
      calEvent({ summary: "Demo day", start: dayKey(soon), uid: "uid-demo" }),
    ]);
    const linked = task({ event_uid: "uid-demo", event_title: "Demo day", deadline: "2030-12-20" });

    render(<TaskEditor open task={linked} onClose={() => {}} />);
    await waitFor(() => expect(segment("Event's date").hasAttribute("disabled")).toBe(false));
    await waitFor(() =>
      expect(screen.getByText(`📅 ${formatDateLocal(soon)} · Demo day`)).toBeTruthy(),
    );
    fireEvent.click(segment("Event's date"));
    expect(screen.getByText(`📅 Demo day: ${formatDateLocal(soon)}`)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalledTimes(1));
    expect(mocked.updateTask).toHaveBeenLastCalledWith(
      1,
      expect.objectContaining({ deadlineFrom: "event", deadline: dayKey(soon) }),
    );
    cleanup();

    // Over to the event's date and back: the own date is still there.
    render(<TaskEditor open task={linked} onClose={() => {}} />);
    await waitFor(() => expect(segment("Event's date").hasAttribute("disabled")).toBe(false));
    fireEvent.click(segment("Event's date"));
    fireEvent.click(segment("My own date"));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalledTimes(2));
    expect(mocked.updateTask).toHaveBeenLastCalledWith(
      1,
      expect.objectContaining({ deadlineFrom: "date", deadline: "2030-12-20" }),
    );
  });

  it("says what the backend resolved a saved event link to, even for an old event", async () => {
    // The event was 49 days ago: past the picker's 30-day window, but the backend still follows it.
    const old = dayKey(addDays(new Date(), -49));
    mocked.listAllCalendarEvents.mockResolvedValueOnce([
      calEvent({ summary: "Review", start: old, uid: "uid-old" }),
    ]);
    const linked = task({
      event_uid: "uid-old",
      event_title: "Review",
      deadline_from: "event",
      deadline: old,
      effective_deadline: old,
    });
    render(<TaskEditor open task={linked} onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(`📅 ${formatDateOnly(old)} · Review`)).toBeTruthy(),
    );
    expect(screen.getByText(`📅 Review: ${formatDateOnly(old)}`)).toBeTruthy();
    expect(screen.queryByText(/your own date is used/)).toBeNull();
    expect(screen.queryByText(/not in your calendars/)).toBeNull();
    cleanup();

    // Only the backend's `deadline_missing` makes it say the own date is used.
    render(<TaskEditor open task={{ ...linked, deadline_missing: true }} onClose={() => {}} />);
    await waitFor(() =>
      expect(
        screen.getByText(
          `That event isn't in your calendars right now, so your own date is used (${formatDateOnly(old)}).`,
        ),
      ).toBeTruthy(),
    );
  });

  it("clearing the project lets a linked milestone go, and its deadline back to the own date", async () => {
    mocked.listAllMilestones.mockResolvedValue([milestone()]);
    render(
      <TaskEditor
        open
        task={task({
          milestone_id: 5,
          milestone_label: "pitch",
          deadline_from: "milestone",
          deadline: "2026-11-20",
        })}
        onClose={() => {}}
      />,
    );
    await waitFor(() => expect(screen.getByText(/◆ pitch/)).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "Clear the project" }));
    expect((screen.getByLabelText("Milestone") as HTMLSelectElement).value).toBe("");
    expect(segment("My own date").getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalled());
    expect(mocked.updateTask).toHaveBeenCalledWith(
      1,
      expect.objectContaining({
        project: null,
        milestoneId: null,
        deadlineFrom: "date",
        deadline: "2026-11-20",
      }),
    );
  });

  it("shows a moved event's new day in the event field, as the deadline line does", async () => {
    const pin = dayKey(addDays(new Date(), 3));
    const moved = dayKey(addDays(new Date(), 6));
    mocked.listAllCalendarEvents.mockResolvedValueOnce([
      calEvent({ summary: "Review", start: moved, uid: "uid-moved" }),
    ]);
    const linked = task({
      event_uid: "uid-moved",
      event_title: "Review",
      deadline_from: "event",
      deadline: pin,
      effective_deadline: moved,
    });
    render(<TaskEditor open task={linked} onClose={() => {}} />);
    await waitFor(() =>
      expect(screen.getByText(`📅 ${formatDateOnly(moved)} · Review`)).toBeTruthy(),
    );
    expect(screen.getByText(`📅 Review: ${formatDateOnly(moved)}`)).toBeTruthy();
    expect(screen.queryByText(new RegExp(formatDateOnly(pin)))).toBeNull();
  });

  it("emptying the project field by hand lets a linked milestone go too", async () => {
    mocked.listAllMilestones.mockResolvedValue([milestone()]);
    render(<TaskEditor open task={task({ milestone_id: 5 })} onClose={() => {}} />);
    const select = screen.getByLabelText("Milestone") as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe("5"));
    const field = screen.getByLabelText("Project");
    fireEvent.change(field, { target: { value: "  " } });
    fireEvent.blur(field);
    expect(select.value).toBe("");
  });

  it("keeps the milestone when the project is retyped in another case", async () => {
    mocked.listAllMilestones.mockResolvedValue([milestone()]);
    render(<TaskEditor open task={task({ milestone_id: 5 })} onClose={() => {}} />);
    const select = screen.getByLabelText("Milestone") as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe("5"));
    const field = screen.getByLabelText("Project");
    fireEvent.change(field, { target: { value: "atlas" } });
    fireEvent.blur(field);
    expect(select.value).toBe("5");
  });

  it("shows the stack's note, and edits or deletes the stack from its row", async () => {
    mocked.listTaskStacks.mockResolvedValue([
      {
        id: 3,
        title: "Visa paperwork",
        kind: "waiting_on",
        note: "ref 4471",
        created_at: "",
        updated_at: "",
      },
    ]);
    render(<TaskEditor open task={task({ stack_id: 3 })} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText("Waiting on someone · ref 4471")).toBeTruthy());

    fireEvent.click(screen.getByRole("button", { name: "Edit the stack" }));
    const editor = screen.getByRole("dialog", { name: "Edit stack" });
    fireEvent.change(within(editor).getByLabelText("Stack name"), {
      target: { value: "Visa forms" },
    });
    fireEvent.change(within(editor).getByLabelText("How its tasks relate"), {
      target: { value: "in_order" },
    });
    fireEvent.change(within(editor).getByLabelText("Note (optional)"), {
      target: { value: "  " },
    });
    fireEvent.click(within(editor).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit stack" })).toBeNull());
    expect(mocked.updateTaskStack).toHaveBeenCalledWith(3, "Visa forms", "in_order", null);
    const stackSelect = screen.getByLabelText("Stack") as HTMLSelectElement;
    expect(within(stackSelect).getByRole("option", { name: "Visa forms" })).toBeTruthy();
    expect(screen.getByText("In order")).toBeTruthy();

    // Deleting asks first; the task stays, out of the stack.
    fireEvent.click(screen.getByRole("button", { name: "Edit the stack" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete stack" }));
    const confirm = screen.getByRole("dialog", { name: "Delete this stack?" });
    expect(mocked.deleteTaskStack).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(mocked.deleteTaskStack).toHaveBeenCalledWith(3));
    await waitFor(() => expect(stackSelect.value).toBe(""));
    expect(within(stackSelect).queryByRole("option", { name: "Visa forms" })).toBeNull();
    expect(mocked.deleteTask).not.toHaveBeenCalled();
  });

  it("closes only the stack editor, or only the confirm, on one Escape", async () => {
    mocked.listTaskStacks.mockResolvedValue([
      {
        id: 3,
        title: "Visa paperwork",
        kind: "sequential",
        note: null,
        created_at: "",
        updated_at: "",
      },
    ]);
    const onClose = vi.fn();
    render(<TaskEditor open task={task({ stack_id: 3 })} onClose={onClose} />);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Edit the stack" })).toBeTruthy(),
    );
    fireEvent.click(screen.getByRole("button", { name: "Edit the stack" }));
    expect(screen.getByRole("dialog", { name: "Edit stack" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Edit stack" })).toBeNull());
    expect(onClose).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Delete task" }));
    expect(screen.getByRole("dialog", { name: "Delete this task?" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("dialog", { name: "Delete this task?" })).toBeNull(),
    );
    expect(onClose).not.toHaveBeenCalled();
  });

  it("saves a task whose milestone was deleted on its own date again", async () => {
    render(
      <TaskEditor
        open
        task={task({ deadline_from: "milestone", milestone_id: null, deadline: "2026-11-20" })}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.updateTask).toHaveBeenCalled());
    expect(mocked.updateTask).toHaveBeenCalledWith(
      1,
      expect.objectContaining({ deadlineFrom: "date", milestoneId: null, deadline: "2026-11-20" }),
    );
    expect(mocked.changeTaskStatus).not.toHaveBeenCalled();
  });

  it("changes the status after the fields, only when it changed", async () => {
    render(<TaskEditor open task={task()} onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("Status"), { target: { value: "done" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mocked.changeTaskStatus).toHaveBeenCalledWith(1, "done"));
    expect(mocked.updateTask.mock.invocationCallOrder[0]).toBeLessThan(
      mocked.changeTaskStatus.mock.invocationCallOrder[0],
    );
  });

  it("makes a new stack first, then files the task into it", async () => {
    render(<TaskEditor open onClose={() => {}} />);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Passport photo" } });
    fireEvent.change(screen.getByLabelText("Stack"), { target: { value: "new" } });
    fireEvent.change(screen.getByLabelText("Stack name"), { target: { value: "Visa" } });
    fireEvent.change(screen.getByLabelText("How its tasks relate"), {
      target: { value: "waiting_on" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    expect(mocked.addTaskStack).toHaveBeenCalledWith("Visa", "waiting_on", null);
    expect(mocked.addTask).toHaveBeenCalledWith(expect.objectContaining({ stackId: 9 }));
  });

  it("shows a refused save and stays open; a retry doesn't make the stack twice", async () => {
    const onClose = vi.fn();
    mocked.addTask.mockRejectedValueOnce("that milestone belongs to another project");
    render(<TaskEditor open onClose={onClose} />);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "X" } });
    fireEvent.change(screen.getByLabelText("Stack"), { target: { value: "new" } });
    fireEvent.change(screen.getByLabelText("Stack name"), { target: { value: "Visa" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "that milestone belongs to another project",
    );
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mocked.addTaskStack).toHaveBeenCalledTimes(1);
    expect(mocked.addTask).toHaveBeenLastCalledWith(expect.objectContaining({ stackId: 9 }));
  });

  it("asks before deleting a task", async () => {
    const onDeleted = vi.fn();
    render(<TaskEditor open task={task()} onClose={() => {}} onDeleted={onDeleted} />);
    fireEvent.click(screen.getByRole("button", { name: "Delete task" }));
    const confirm = screen.getByRole("dialog", { name: "Delete this task?" });
    expect(mocked.deleteTask).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(mocked.deleteTask).toHaveBeenCalledWith(1);
  });

  it("hides the project field when the project is fixed, and writes that project", async () => {
    render(<TaskEditor open fixedProject="Atlas" onClose={() => {}} />);
    expect(screen.queryByLabelText("Project")).toBeNull();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Y" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    expect(mocked.addTask).toHaveBeenCalledWith(expect.objectContaining({ project: "Atlas" }));
    expect(mocked.listProjects).not.toHaveBeenCalled();
  });

  it("remembers folding Links away, so the next task opens folded, and says what's linked", async () => {
    localStorage.clear();
    const links = () => screen.getByRole("button", { name: /^Links \(all optional\)/ });
    const { unmount } = render(
      <TaskEditor
        open
        task={task({ project_name: "Home", event_uid: "uid-demo" })}
        onClose={() => {}}
      />,
    );
    expect(links().getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(links());
    expect(links().getAttribute("aria-expanded")).toBe("false");
    expect(links().textContent).toContain("project, event");
    unmount();

    render(<TaskEditor open onClose={() => {}} />);
    expect(links().getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(links());
    expect(links().getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByLabelText("Milestone")).toBeTruthy();
    localStorage.clear();
  });

  it("says what a stack is until one is picked", async () => {
    render(<TaskEditor open onClose={() => {}} />);
    expect(screen.getByText(/Group tasks that belong to one job/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Stack"), { target: { value: "new" } });
    expect(screen.queryByText(/Group tasks that belong to one job/)).toBeNull();
    const kind = screen.getByLabelText("How its tasks relate") as HTMLSelectElement;
    expect([...kind.options].map((o) => o.textContent)).toEqual([
      "In sequence: each step waits for the one before",
      "In order: a suggested order, nothing waits",
      "Waiting on someone: stuck until someone else does their part",
    ]);
  });

  it("renders nothing while closed", () => {
    render(<TaskEditor open={false} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
