// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The "which occurrences?" dialog (#884): nothing is sent until Save or Delete, Enter on open can't
// choose, a row Google wouldn't allow says why, and a change PM couldn't finish is settled from one
// block. jsdom never turns Enter into a click, so "Enter on open sends nothing" is pinned as focus
// on Back (no radio, not Save) and Back closing.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { ScopeRow, SeriesWarning } from "../../../lib/types";
import type { ReadyChoices } from "../../../lib/calendarEdit/saveChoices";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"] });
  vi.setSystemTime(new Date("2026-10-18T12:00:00"));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const offered = (token: string, warnings: SeriesWarning[] = []): ScopeRow => ({
  row: "offered",
  warnings,
  token,
});
const ready = (
  this_: ScopeRow = offered("t"),
  following: ScopeRow = offered("f"),
  all: ScopeRow = offered("a"),
): ReadyChoices => ({ outcome: "ready", this: this_, following, all });
const view = (choices: ReadyChoices, again = false, notice: string | null = null): ChoicesView => ({
  state: "ready",
  choices,
  again,
  notice,
});
const VISIBILITY =
  "Who can see an event applies to every occurrence of a repeating event, so it can't change for this one only.";
const pending: ScopeRow = {
  row: "pending",
  token: "p",
  offers: ["check", "remove", "finish"],
  from: "2026-11-02",
};

function open(props: Partial<Parameters<typeof SaveChoicesDialog>[0]> = {}) {
  const onChoose = vi.fn();
  const onSettle = vi.fn();
  const onClose = vi.fn();
  const utils = render(
    <SaveChoicesDialog
      kind="save"
      title="Gym"
      view={view(ready())}
      settling={false}
      onChoose={onChoose}
      onSettle={onSettle}
      onClose={onClose}
      {...props}
    />,
  );
  return { ...utils, onChoose, onSettle, onClose };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

describe("SaveChoicesDialog", () => {
  it("is named as Google Calendar names it", () => {
    open();
    expect(screen.getByRole("dialog", { name: "Edit repeating event" })).toBeTruthy();
    cleanup();
    open({ kind: "delete" });
    expect(screen.getByRole("dialog", { name: "Delete repeating event" })).toBeTruthy();
  });

  it("picks This event and focuses Back, so Enter on open sends nothing", () => {
    const { onChoose, onClose } = open();
    expect(radio(/^This event/).checked).toBe(true);
    const back = button("Back");
    expect(document.activeElement).toBe(back);
    expect(screen.getAllByRole("radio")).not.toContain(document.activeElement);
    expect(document.activeElement).not.toBe(button("Save"));
    fireEvent.click(back);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onChoose).not.toHaveBeenCalled();
  });

  it("sends nothing until Save, then the picked row's scope and token, once", () => {
    const { onChoose } = open();
    fireEvent.click(radio(/^All events/));
    fireEvent.click(radio(/^This and following events/));
    expect(onChoose).not.toHaveBeenCalled();
    const save = button("Save");
    fireEvent.click(save);
    fireEvent.click(save);
    expect(onChoose).toHaveBeenCalledTimes(1);
    expect(onChoose).toHaveBeenCalledWith({ scope: "following", token: "f" });
  });

  it("shows rows that can't be chosen disabled, with their reasons", () => {
    open({
      view: view(
        ready(
          { row: "refused", message: VISIBILITY },
          { row: "conflict", fields: ["summary"] },
          pending,
        ),
      ),
    });
    expect(radio(/^This event/).disabled).toBe(true);
    expect(radio(/^This and following events/).disabled).toBe(true);
    expect(radio(/^All events/).disabled).toBe(true);
    expect(screen.getAllByText(VISIBILITY).length).toBeGreaterThan(0);
    expect(screen.getByText("Changed in Google since you opened it (the title).")).toBeTruthy();
    expect(screen.getByText("Not until the earlier change below is settled.")).toBeTruthy();
    // Every row refused: Save stays off, and Back is the way out.
    expect(button("Save").disabled).toBe(true);
  });

  it("waits for a pick when This event can't be, saying why in the lead", () => {
    const { onChoose } = open({ view: view(ready({ row: "refused", message: VISIBILITY })) });
    expect(screen.getAllByRole("radio").some((r) => (r as HTMLInputElement).checked)).toBe(false);
    expect(button("Save").disabled).toBe(true);
    // The reason a keyboard user can't reach on the disabled radio, in the lead (C2-M7).
    expect(screen.getByText(/should change\? Who can see an event/)).toBeTruthy();
    fireEvent.click(radio(/^All events/));
    expect(button("Save").disabled).toBe(false);
    fireEvent.click(button("Save"));
    expect(onChoose).toHaveBeenCalledWith({ scope: "all", token: "a" });
  });

  it("says what 'this and following' means from the first occurrence", () => {
    open({ view: view(ready(offered("t"), { row: "as_all", warnings: [], token: "x" })) });
    expect(radio(/^This and following events/)).toBeTruthy();
    expect(
      screen.getByText(
        "This is the first occurrence, so this changes every occurrence, like All events.",
      ),
    ).toBeTruthy();
  });

  it("says the picked row's question in the live region, naming both days", () => {
    const move: SeriesWarning = {
      warning: "repeat_day_changes",
      from: "2026-10-18",
      to: "2026-10-19",
      before: "every Sunday",
      after: "every Monday",
      shift_days: 1,
    };
    open({ view: view(ready(offered("t"), offered("f"), offered("a", [move]))) });
    // One region, mounted while empty, so its lines are heard when the pick changes (C2-M7).
    const status = screen.getByRole("status");
    expect(status.textContent).toBe("");
    fireEvent.click(radio(/^All events/));
    expect(screen.getByRole("status")).toBe(status);
    expect(status.textContent).toBe(
      "“Gym” repeats every Sunday. Change it to repeat every Monday?",
    );
  });

  it("says a 'this and following' delete skips the Trash, and how Undo works", () => {
    open({ kind: "delete", day: "19-10", when: "Mon 19-10, 09:00", calendarName: "Work" });
    expect(
      screen.getByText(
        "This occurrence and every one after it. Google Calendar can't restore these from its Trash.",
      ),
    ).toBeTruthy();
    expect(screen.getByText("Only the one on 19-10.")).toBeTruthy();
    expect(
      screen.getByText(
        "Which occurrences of “Gym” (Mon 19-10, 09:00) should be deleted from Work in Google Calendar?",
      ),
    ).toBeTruthy();
    expect(screen.getByText(/You can undo it for a few seconds/)).toBeTruthy();
    expect(button("Cancel")).toBe(document.activeElement);
    expect(button("Delete").disabled).toBe(false);
  });

  it("shows the notice above the rows, and the re-ask's line", () => {
    open({
      view: view(ready(), true, "PM closed before it finished changing this repeating event."),
    });
    const notice = screen.getByText("PM closed before it finished changing this repeating event.");
    const again = screen.getByText(/changed since you chose which occurrences to change/);
    const first = radio(/^This event/);
    expect(notice.compareDocumentPosition(first) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(again.getAttribute("role")).toBe("status");
  });

  it("settles a pending change from one block, buttons in order; settling disables everything", () => {
    const { onSettle, rerender, onChoose, onClose } = open({
      view: view(ready(offered("t"), pending, pending)),
    });
    const labels = ["Finish", "Remove the new part", "Check again"];
    const buttons = labels.map(button);
    expect(screen.getAllByText(/may show twice from 02-11/)).toHaveLength(1);
    fireEvent.click(buttons[1]);
    expect(onSettle).toHaveBeenCalledWith("p", "remove");
    rerender(
      <SaveChoicesDialog
        kind="save"
        title="Gym"
        view={view(ready(offered("t"), pending, pending))}
        settling
        onChoose={onChoose}
        onSettle={onSettle}
        onClose={onClose}
      />,
    );
    for (const label of labels) expect(button(label).disabled).toBe(true);
    for (const r of screen.getAllByRole("radio"))
      expect((r as HTMLInputElement).disabled).toBe(true);
    expect(button("Save").disabled).toBe(true);
    // The pick survives the re-render: same answer, same selection.
    expect(radio(/^This event/).checked).toBe(true);
  });

  it("keeps the user's pick across re-renders, and picks afresh for a new answer", () => {
    const props = {
      kind: "save" as const,
      title: "Gym",
      settling: false,
      onChoose: vi.fn(),
      onSettle: vi.fn(),
      onClose: vi.fn(),
    };
    const choices = ready();
    const { rerender } = render(<SaveChoicesDialog {...props} view={view(choices)} />);
    fireEvent.click(radio(/^All events/));
    rerender(<SaveChoicesDialog {...props} view={view(choices)} />);
    expect(radio(/^All events/).checked).toBe(true);
    rerender(<SaveChoicesDialog {...props} view={view(ready(), true)} />);
    expect(radio(/^This event/).checked).toBe(true);
  });

  it("loads with a skeleton, Save off and Back working", () => {
    const { onClose } = open({ view: { state: "loading" } });
    expect(screen.getByText("Checking “Gym” in Google…")).toBeTruthy();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(button("Save").disabled).toBe(true);
    fireEvent.click(button("Back"));
    expect(onClose).toHaveBeenCalled();
  });

  it("shows a refusal's words, with no rows", () => {
    open({
      view: { state: "refused", tone: "error", text: "Editing isn't on for this Google account." },
    });
    expect(screen.getByText("Editing isn't on for this Google account.")).toBeTruthy();
    expect(screen.queryAllByRole("radio")).toHaveLength(0);
    expect(button("Save").disabled).toBe(true);
  });

  it("announces a refusal of any tone, since it arrives after the dialog opened", () => {
    open({ view: { state: "refused", tone: "warn", text: "It changed in Google." } });
    const announced = screen.getAllByRole("status").map((el) => el.textContent);
    expect(announced).toContain("It changed in Google.");
  });

  it("closes on Escape", () => {
    const { onClose } = open();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
