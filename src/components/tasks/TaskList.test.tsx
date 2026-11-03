// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The live task list: it re-reads on `tasks://changed` (any surface's write), hides archived
// projects' tasks unless scoped to one, folds parked and done tasks away, writes through the one
// status command, reorders within the open list, and turns to a plain summary below Standard depth.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Task } from "../../lib/types";
import { TaskList } from "./TaskList";

let tasksChanged: (() => void) | null = null;

vi.mock("../../lib/ipc", () => ({
  addTask: vi.fn(async () => 99),
  addTaskStack: vi.fn(async () => 1),
  changeTaskStatus: vi.fn(async () => {}),
  deleteTask: vi.fn(async () => {}),
  listAllCalendarEvents: vi.fn(async () => []),
  listAllMilestones: vi.fn(async () => []),
  listProjects: vi.fn(async () => []),
  listTasks: vi.fn(async () => []),
  listTaskStacks: vi.fn(async () => []),
  onTasksChanged: vi.fn(async (handler: () => void) => {
    tasksChanged = handler;
    return () => {
      tasksChanged = null;
    };
  }),
  reorderTasks: vi.fn(async () => {}),
  updateTask: vi.fn(async () => {}),
}));

let depth = "standard";
vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal()),
  useTheme: () => ({
    system: "slate",
    mode: "dark",
    modePref: "system",
    modeSource: "system",
    accent: "mono",
    depth,
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

beforeEach(() => {
  vi.clearAllMocks();
  depth = "standard";
  tasksChanged = null;
});
afterEach(cleanup);

describe("TaskList", () => {
  it("lists open tasks in order and folds parked and done ones away", async () => {
    mocked.listTasks.mockResolvedValue([
      task({ id: 2, title: "Second", ordinal: 1 }),
      task({ id: 1, title: "First", ordinal: 0, why: "Because the deck is due" }),
      task({ id: 3, title: "Shelved", parked: true }),
      task({ id: 4, title: "Shipped", status: "done", completed_at: "2026-11-01T00:00:00Z" }),
    ]);
    render(<TaskList project="Atlas" />);
    await screen.findByText("Second");
    const titles = screen
      .getAllByRole("button", { name: /^Edit / })
      .map((b) => b.getAttribute("aria-label"));
    expect(titles.slice(0, 2)).toEqual(["Edit First", "Edit Second"]);
    expect(screen.getByRole("button", { name: /Parked/ }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    expect(screen.getByRole("button", { name: /Done \(1\)/ }).getAttribute("aria-expanded")).toBe(
      "false",
    );
    expect(mocked.listTasks).toHaveBeenCalledWith("Atlas");

    // The title toggles its note.
    expect(screen.queryByText("Because the deck is due")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "First" }));
    expect(screen.getByText("Because the deck is due")).toBeTruthy();
  });

  it("hides archived projects' tasks in an all-tasks list, unless asked", async () => {
    mocked.listTasks.mockResolvedValue([
      task({ id: 1, title: "Live one" }),
      task({ id: 2, title: "Old one", project_name: "Old", project_archived: true }),
    ]);
    const { unmount } = render(<TaskList showProject />);
    await screen.findByText("Live one");
    expect(screen.queryByText("Old one")).toBeNull();
    expect(mocked.listTasks).toHaveBeenCalledWith(null);
    expect(screen.getByText("Atlas")).toBeTruthy();
    unmount();

    render(<TaskList includeArchived />);
    expect(await screen.findByText("Old one")).toBeTruthy();
  });

  it("cycles a task's status through the one status command", async () => {
    mocked.listTasks.mockResolvedValue([task({ id: 7, status: "doing" })]);
    render(<TaskList project="Atlas" />);
    const tick = await screen.findByRole("button", { name: "Doing: mark as done" });
    fireEvent.click(tick);
    await waitFor(() => expect(mocked.changeTaskStatus).toHaveBeenCalledWith(7, "done"));
  });

  it("re-reads when any surface changes a task", async () => {
    mocked.listTasks.mockResolvedValueOnce([task({ title: "Before" })]);
    render(<TaskList project="Atlas" />);
    await screen.findByText("Before");
    mocked.listTasks.mockResolvedValueOnce([task({ title: "After" })]);
    await waitFor(() => expect(tasksChanged).not.toBeNull());
    act(() => tasksChanged?.());
    expect(await screen.findByText("After")).toBeTruthy();
  });

  it("reorders within the open list", async () => {
    mocked.listTasks.mockResolvedValue([
      task({ id: 1, title: "A", ordinal: 0 }),
      task({ id: 2, title: "B", ordinal: 1 }),
      task({ id: 3, title: "C", ordinal: 2 }),
    ]);
    render(<TaskList project="Atlas" />);
    await screen.findByText("C");
    expect(screen.getByRole("button", { name: "Move A up" }).hasAttribute("disabled")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Move B down" }));
    await waitFor(() => expect(mocked.reorderTasks).toHaveBeenCalledWith([1, 3, 2]));
  });

  it("adds a task with the list's project on Enter", async () => {
    mocked.listTasks.mockResolvedValue([]);
    render(<TaskList project="Atlas" />);
    expect(await screen.findByText("No tasks for this project yet. Add one below.")).toBeTruthy();
    const box = screen.getByRole("textbox", { name: "New task" });
    fireEvent.change(box, { target: { value: "Call the venue" } });
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(mocked.addTask).toHaveBeenCalled());
    expect(mocked.addTask).toHaveBeenCalledWith(
      expect.objectContaining({ title: "Call the venue", project: "Atlas", deadline: null }),
    );
    await waitFor(() => expect((box as HTMLInputElement).value).toBe(""));
  });

  it("shows a refused write", async () => {
    mocked.listTasks.mockResolvedValue([task()]);
    mocked.changeTaskStatus.mockRejectedValueOnce("database is locked");
    render(<TaskList project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "To do: mark as doing" }));
    expect((await screen.findByRole("alert")).textContent).toContain("database is locked");
  });

  it("opens the editor from a row", async () => {
    mocked.listTasks.mockResolvedValue([task()]);
    render(<TaskList project="Atlas" />);
    fireEvent.click(await screen.findByRole("button", { name: "Edit Write the pitch" }));
    expect(await screen.findByRole("dialog", { name: "Edit task" })).toBeTruthy();
  });

  it("is a plain summary below Standard depth", async () => {
    depth = "min";
    mocked.listTasks.mockResolvedValue([
      task({ id: 1, title: "Open one" }),
      task({ id: 2, title: "Closed one", status: "done" }),
    ]);
    render(<TaskList project="Atlas" />);
    expect(await screen.findByText("Open one")).toBeTruthy();
    expect(screen.getByText("Closed one").className).toContain("line-through");
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});
