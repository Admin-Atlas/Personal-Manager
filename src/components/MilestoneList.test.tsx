// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The milestone row's LIFECYCLE, which is where it silently corrupted dates. A row is keyed by id, so
// the same instance survives every refetch — and its `persist` compares the local draft against the
// live prop, so a local value that never adopted a refetched one is exactly the case that skips the
// no-op guard and writes the outdated day back over the newer one. The dates a milestone owns are
// also the dates the flags layer and Due-soon read, so a silent overwrite here is not cosmetic.
//
// The other half is the optimistic date commit: a REFUSED write triggers no refetch, so `m.due_date`
// never changes, the adopt effect never re-fires, and without an explicit rollback the field is left
// showing — and primed to re-commit — a date the backend rejected. Both halves are tested, because
// neither closes the finding on its own.
//
// `useTheme` is stubbed the way ConnectorItemRow.test.tsx does it, so the row's Button/Input/Select
// render without the full ThemeProvider (which pulls in IPC).

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Milestone } from "../lib/types";
import { MilestoneList } from "./MilestoneList";

vi.mock("../lib/ipc", () => ({
  addMilestone: vi.fn(async () => 1),
  deleteMilestone: vi.fn(async () => {}),
  reorderMilestones: vi.fn(async () => {}),
  setMilestoneEvent: vi.fn(async () => {}),
  setMilestoneStatus: vi.fn(async () => {}),
  updateMilestone: vi.fn(async () => {}),
}));

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

import { setMilestoneEvent, updateMilestone } from "../lib/ipc";

const mockUpdate = vi.mocked(updateMilestone);
const mockSetEvent = vi.mocked(setMilestoneEvent);

function milestone(over: Partial<Milestone> = {}): Milestone {
  return {
    id: 1,
    project_name: "Atlas",
    label: "pitch",
    due_date: "2026-08-01",
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

function renderList(m: Milestone) {
  return render(<MilestoneList project="Atlas" milestones={[m]} onChanged={() => {}} />);
}

/** The editable deadline field (absent while the milestone is calendar-linked). */
function dateInput(): HTMLInputElement {
  return screen.getByLabelText("Milestone deadline") as HTMLInputElement;
}

function labelInput(): HTMLInputElement {
  return screen.getByPlaceholderText("label") as HTMLInputElement;
}

beforeEach(() => {
  vi.clearAllMocks();
});

afterEach(cleanup);

describe("a refetch moving the milestone underneath the row", () => {
  it("shows the new date, not the one it mounted with", () => {
    const { rerender } = renderList(milestone());
    expect(dateInput().value).toBe("01-08-2026");

    // Same id — React keeps the instance, so only the adopt effect can update the field.
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    expect(dateInput().value).toBe("01-09-2026");
  });

  // THE silent overwrite. Blurring the label without touching it must be a no-op; with a stale local
  // date it isn't — `nextDate` differs from the live `curDate`, the guard doesn't fire, and the row
  // writes its mount-time day back over the one the sync just delivered.
  it("does not write its stale date back when the label is merely blurred", async () => {
    const { rerender } = renderList(milestone());
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );

    fireEvent.focus(labelInput());
    fireEvent.blur(labelInput());
    await waitFor(() => expect(mockUpdate).not.toHaveBeenCalled());
  });

  it("adopts a changed label but keeps text the user is still typing", () => {
    const { rerender } = renderList(milestone());
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ label: "renamed remotely" })]}
        onChanged={() => {}}
      />,
    );
    expect(labelInput().value).toBe("renamed remotely");

    // A refetch that brings back the SAME label must not stamp on an in-flight edit: the dep never
    // changes, so the effect never re-runs.
    fireEvent.change(labelInput(), { target: { value: "mid-typing" } });
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ label: "renamed remotely" })]}
        onChanged={() => {}}
      />,
    );
    expect(labelInput().value).toBe("mid-typing");
  });

  // The end-to-end shape of the real defect: while linked the field isn't rendered at all, so a
  // stale local date is invisible right up until Unlink makes it editable again.
  it("shows the MOVED date after a calendar-linked milestone is unlinked", async () => {
    const linked = milestone({ calendar_linked: true, event_uid: "uid-1" });
    const { rerender } = renderList(linked);
    expect(screen.queryByLabelText("Milestone deadline")).toBeNull();

    // A calendar sync moves the event; a sibling mutation's refetch delivers the new date.
    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ ...linked, due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    fireEvent.click(screen.getByTitle(/Unlink from calendar/));
    await waitFor(() => expect(mockSetEvent).toHaveBeenCalledWith(1, null, "2026-09-01"));

    rerender(
      <MilestoneList
        project="Atlas"
        milestones={[milestone({ due_date: "2026-09-01" })]}
        onChanged={() => {}}
      />,
    );
    expect(dateInput().value).toBe("01-09-2026");
  });
});

describe("a date the backend refuses", () => {
  it("rolls the field back and says so", async () => {
    mockUpdate.mockRejectedValueOnce(new Error("that date is in the past"));
    renderList(milestone());

    fireEvent.change(dateInput(), { target: { value: "15-09-2026" } });
    fireEvent.blur(dateInput());

    // The error line is the only thing on screen that explains the revert.
    await screen.findByRole("alert");
    expect(screen.getByRole("alert").textContent).toContain("that date is in the past");
    // No refetch follows a refusal, so `m.due_date` never moves and only the explicit rollback can
    // put the field back.
    await waitFor(() => expect(dateInput().value).toBe("01-08-2026"));
  });
});
