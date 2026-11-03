// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The delete dialog states what has no choice: the milestones go, the tasks stay without a project,
// and (when the project has milestones) a task deadline that followed one goes back to the task's
// own date.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DeleteProjectDialog } from "./DeleteProjectDialog";

vi.mock("../lib/ipc", () => ({
  deleteProject: vi.fn(async () => {}),
  deleteProjectPreview: vi.fn(async () => ({
    files: 0,
    chats: 0,
    milestones: 2,
    tasks: 3,
    canonical: "Atlas",
  })),
}));

// `useTheme` is stubbed so the primitives render without the ThemeProvider (which pulls in IPC).
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
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

import * as ipc from "../lib/ipc";

afterEach(cleanup);

describe("DeleteProjectDialog", () => {
  it("says a deadline that followed one of its milestones goes back to the task's own date", async () => {
    render(<DeleteProjectDialog project="Atlas" onClose={() => {}} onDeleted={() => {}} />);
    expect(
      await screen.findByText(
        /Its 3 tasks stay, without a project, and a deadline that followed one of its milestones goes back to the task's own date\./,
      ),
    ).toBeTruthy();
  });

  it("only says the tasks stay when the project has no milestones", async () => {
    vi.mocked(ipc.deleteProjectPreview).mockResolvedValueOnce({
      files: 0,
      chats: 0,
      milestones: 0,
      tasks: 1,
      canonical: "Atlas",
    });
    render(<DeleteProjectDialog project="Atlas" onClose={() => {}} onDeleted={() => {}} />);
    expect(await screen.findByText(/Its 1 task stays, without a project\./)).toBeTruthy();
    expect(screen.queryByText(/followed one of its milestones/)).toBeNull();
  });
});
