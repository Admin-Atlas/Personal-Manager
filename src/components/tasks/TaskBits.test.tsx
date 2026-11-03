// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared chips: a stack chip carries the stack's note (in its tooltip, and written out where a
// surface asks for it), and a deadline chip whose milestone was deleted says the link was removed.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { Task, TaskStack } from "../../lib/types";
import { DeadlineChip, StackChip } from "./TaskBits";

const stack: TaskStack = {
  id: 3,
  title: "Visa paperwork",
  kind: "waiting_on",
  note: "ref 4471",
  created_at: "",
  updated_at: "",
};

afterEach(cleanup);

describe("StackChip", () => {
  it("puts the note in its tooltip, and writes it out only when asked", () => {
    const { rerender } = render(<StackChip stack={stack} />);
    const chip = screen.getByText(/Visa paperwork/);
    expect(chip.getAttribute("title")).toBe("Visa paperwork · Waiting on someone\nref 4471");
    expect(screen.queryByText("ref 4471")).toBeNull();

    rerender(<StackChip stack={stack} showNote />);
    expect(screen.getByText("ref 4471")).toBeTruthy();

    rerender(<StackChip stack={{ ...stack, note: null }} showNote />);
    expect(screen.getByText(/Visa paperwork/).getAttribute("title")).toBe(
      "Visa paperwork · Waiting on someone",
    );
  });
});

describe("DeadlineChip", () => {
  it("says a deadline's milestone was removed once its link is gone", () => {
    const t = {
      id: 1,
      status: "todo",
      deadline: null,
      deadline_kind: "hard",
      deadline_from: "milestone",
      effective_deadline: null,
      deadline_missing: true,
      milestone_id: null,
      milestone_label: null,
      event_uid: null,
      event_title: null,
    } as unknown as Task;
    render(<DeadlineChip task={t} today={new Date(2026, 10, 3)} />);
    expect(screen.getByText(/no date/).getAttribute("title")).toBe(
      "The linked milestone was removed, so this is your own date",
    );
  });
});
