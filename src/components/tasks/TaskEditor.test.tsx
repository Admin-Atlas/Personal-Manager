// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The task editor writes what the backend will accept: links stay independent, a deadline can only
// follow a link that's set (and falls back to the own date when that link is gone), a new stack is
// made before the task that joins it, and a refused save says why and keeps the dialog open.
//
// IPC is mocked whole-module, as MilestoneList.test.tsx does, and `useTheme` is stubbed so the
// primitives render without the ThemeProvider (which pulls in IPC).

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { addDays, dayKey } from "../../lib/calendar-layout";
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
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Prep demo" } });
    fireEvent.click(screen.getByRole("button", { name: "Add task" }));
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    expect(mocked.addTask).toHaveBeenCalledWith(
      expect.objectContaining({ eventUid: "uid-demo", deadlineFrom: "event" }),
    );
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

  it("renders nothing while closed", () => {
    render(<TaskEditor open={false} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
