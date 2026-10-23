// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

// The floating briefing, with something to show; its body isn't what these read.
vi.mock("../../../lib/briefing", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useBriefing: () => ({ briefing: { briefing: "Stand-up at ten." }, busy: false }),
}));
vi.mock("../../Briefing", () => ({ Briefing: () => null }));

import { Modal } from "../../ui";
import { BriefingPanel } from "../../BriefingPanel";
import { WriteNotices } from "./WriteNotices";
import type { WriteNotice } from "./useEventWrites";
import type { MilestoneMove } from "../../../lib/types";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const deleting = (id: number, summary: string): WriteNotice => ({
  id,
  tone: "ok",
  text: `Deleting “${summary}”.`,
  undoToken: `t${id}`,
  undoLabel: `Undo deleting “${summary}”`,
});

/** An element's one z-index utility, as a number (`z-50`, `z-[45]`). jsdom paints nothing, so the
 *  stacking is read from the classes. */
function zOf(el: Element | null): number {
  const z = Array.from(el?.classList ?? [])
    .map((c) => /^z-\[?(\d+)\]?$/.exec(c))
    .filter((m) => m !== null);
  expect(z).toHaveLength(1);
  return Number(z[0]![1]);
}

describe("WriteNotices", () => {
  // A notice that never goes (Check again) covered a new event's Cancel and Save on a short window,
  // and sat outside the dialog's focus trap: notices stack under any open dialog's scrim. Tied with
  // the floating briefing, which comes after them in the page as here, they went under it instead:
  // they sit strictly between the two.
  it("sits under an open dialog's scrim and over the floating briefing", () => {
    localStorage.setItem("pm.briefing.float", "inApp");
    render(
      <>
        <Modal open onClose={vi.fn()} label="New event">
          <button type="button">Save</button>
        </Modal>
        <WriteNotices notices={[deleting(1, "Gym")]} onUndo={vi.fn()} onDismiss={vi.fn()} />
        <BriefingPanel />
      </>,
    );
    const scrim = screen.getByRole("dialog").parentElement;
    const briefing = screen.getByRole("complementary", { name: "Today's briefing" });
    const stack = screen.getByText("Deleting “Gym”.").closest(".fixed");
    expect(zOf(stack)).toBeLessThan(zOf(scrim));
    expect(zOf(stack)).toBeGreaterThan(zOf(briefing));
  });

  // C16a: a move's Undo hands back its kind, so the calendar sends the right one; a delete's, which
  // carries none, is a delete's. The newest of either kind is the shortcut's.
  it("Undo hands its kind back", () => {
    const onUndo = vi.fn();
    const moved: WriteNotice = {
      id: 2,
      tone: "ok",
      text: "“Dentist” is now 12-10 · 10:00–11:00.",
      undoToken: "u2",
      undoLabel: "Undo moving “Dentist”",
      undoKind: "change",
      undoRow: "cal:e",
    };
    render(
      <WriteNotices notices={[deleting(1, "Gym"), moved]} onUndo={onUndo} onDismiss={vi.fn()} />,
    );
    const move = screen.getByRole("button", { name: "Undo moving “Dentist”" });
    expect(move.getAttribute("aria-keyshortcuts")).toBe("Control+Z Meta+Z");
    fireEvent.click(move);
    fireEvent.click(screen.getByRole("button", { name: "Undo deleting “Gym”" }));
    expect(onUndo.mock.calls).toEqual([
      ["u2", "change"],
      ["t1", "delete"],
    ]);
  });

  it("names the shortcut on the newest Undo only", () => {
    render(
      <WriteNotices
        notices={[deleting(1, "Gym"), deleting(2, "Dentist")]}
        onUndo={vi.fn()}
        onDismiss={vi.fn()}
      />,
    );
    const older = screen.getByRole("button", { name: "Undo deleting “Gym”" });
    const newest = screen.getByRole("button", { name: "Undo deleting “Dentist”" });
    expect(newest.getAttribute("aria-keyshortcuts")).toBe("Control+Z Meta+Z");
    expect(older.getAttribute("aria-keyshortcuts")).toBeNull();
  });

  // Ctrl+Z with focus on an older Undo undoes the newest; the older then becomes the newest, and keeps
  // the focus (it used to be swapped for a new button, dropping focus to the page).
  it("keeps focus on an Undo when it becomes the newest", () => {
    const props = { onUndo: vi.fn(), onDismiss: vi.fn() };
    const view = render(
      <WriteNotices notices={[deleting(1, "Gym"), deleting(2, "Dentist")]} {...props} />,
    );
    const older = screen.getByRole("button", { name: "Undo deleting “Gym”" });
    older.focus();
    view.rerender(
      <WriteNotices
        notices={[deleting(1, "Gym"), { id: 2, tone: "ok", text: "Kept “Dentist”." }]}
        {...props}
      />,
    );
    const now = screen.getByRole("button", { name: "Undo deleting “Gym”" });
    expect(now).toBe(older);
    expect(document.activeElement).toBe(older);
    expect(now.getAttribute("aria-keyshortcuts")).toBe("Control+Z Meta+Z");
  });

  // A "this and following" change PM couldn't finish keeps its buttons, in one order, until settled.
  it("offers the actions a change PM couldn't finish allows, in order", () => {
    const partial = (id: number, title: string, settling = false): WriteNotice => ({
      id,
      tone: "warn",
      text: `“${title}”: PM couldn't finish this change.`,
      split: { token: `s${id}`, offers: ["check", "remove", "finish"], title },
      settling,
    });
    const onSettle = vi.fn();
    const onDismiss = vi.fn();
    const view = render(
      <WriteNotices
        notices={[partial(1, "Gym")]}
        onUndo={vi.fn()}
        onDismiss={onDismiss}
        onSettle={onSettle}
      />,
    );
    const labels = screen
      .getAllByRole("button")
      .map((b) => b.textContent)
      .filter((t) => t !== "×");
    expect(labels).toEqual(["Finish", "Remove the new part", "Check again"]);
    fireEvent.click(screen.getByRole("button", { name: "Remove the new part" }));
    expect(onSettle).toHaveBeenCalledWith("s1", "remove", "Gym");
    // It can still be dismissed.
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(onDismiss).toHaveBeenCalledWith(1);
    // Settling: every button off. Two up: each names its event.
    view.rerender(
      <WriteNotices
        notices={[partial(1, "Gym", true), partial(2, "Standup")]}
        onUndo={vi.fn()}
        onDismiss={onDismiss}
        onSettle={onSettle}
      />,
    );
    const gym = screen.getByRole("button", { name: "Finish (“Gym”)" }) as HTMLButtonElement;
    const standup = screen.getByRole("button", { name: "Finish (“Standup”)" }) as HTMLButtonElement;
    expect(gym.disabled).toBe(true);
    expect(standup.disabled).toBe(false);
  });

  // A new event PM may already have made keeps Check again until Google shows how it ended (C10).
  it("offers Check again on a new event PM may have made, off while it checks", () => {
    const held = (id: number, summary: string, checking = false): WriteNotice => ({
      id,
      tone: "warn",
      text: `PM may already have made “${summary}”.`,
      create: {
        token: `ref-${id}`,
        sent: {
          calendarId: "gcal:me@x.com:me@x.com",
          draft: {
            summary,
            time: { kind: "all_day", first_day: "2026-10-12", last_day: "2026-10-12" },
            location: "",
            description: "",
            show_as: "free",
            visibility: "default",
            recurrence: null,
            guests: [],
            reminders: null,
            colour: null,
          },
        },
      },
      checking,
    });
    const onCheckCreate = vi.fn();
    const props = { onUndo: vi.fn(), onDismiss: vi.fn(), onCheckCreate };
    const view = render(<WriteNotices notices={[held(1, "Gym")]} {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    expect(onCheckCreate).toHaveBeenCalledWith(1);
    // Checking: its button off. Two up: each names its event.
    view.rerender(<WriteNotices notices={[held(1, "Gym", true), held(2, " ")]} {...props} />);
    const gym = screen.getByRole("button", { name: "Check again (“Gym”)" }) as HTMLButtonElement;
    const untitled = screen.getByRole("button", {
      name: "Check again (“(no title)”)",
    }) as HTMLButtonElement;
    expect(gym.disabled).toBe(true);
    expect(untitled.disabled).toBe(false);
  });

  // C9b-2: a milestone's move after a "this and following" change.
  const launch: MilestoneMove = {
    milestoneId: 7,
    label: "Launch",
    project: "Atlas",
    newTitle: "Retro",
    earlierTitle: "Gym",
    toIcal: "tail@google.com",
    toDate: "2026-10-19",
    backIcal: "abc@google.com",
    backDate: "2026-10-19",
    earlierLastDay: "2026-10-12",
    today: "2026-10-20",
    pending: false,
  };
  const offer = (moving = false): WriteNotice => ({
    id: 4,
    tone: "ok",
    text: "Move the “Launch” milestone in Atlas…?",
    move: launch,
    moving,
  });
  const moved = (moving = false): WriteNotice => ({
    id: 4,
    tone: "ok",
    text: "Moved “Launch” to the new part of “Retro”.",
    moveBack: launch,
    moving,
  });

  it("an offer shows Move with its name, unpressable while moving, and Keep it there", () => {
    const props = { onUndo: vi.fn(), onDismiss: vi.fn(), onMove: vi.fn(), onMoveBack: vi.fn() };
    const view = render(<WriteNotices notices={[offer()]} {...props} />);
    const move = screen.getByRole("button", { name: "Move “Launch” to the new part of “Retro”" });
    expect(move.textContent).toBe("Move");
    fireEvent.click(move);
    expect(props.onMove).toHaveBeenCalledWith(4);
    fireEvent.click(
      screen.getByRole("button", {
        name: "Keep it there: “Launch” stays on the earlier part of “Gym”",
      }),
    );
    expect(props.onDismiss).toHaveBeenCalledWith(4);
    // Moving: still focusable (aria-disabled, never disabled), and a press does nothing.
    move.focus();
    view.rerender(<WriteNotices notices={[offer(true)]} {...props} />);
    const waiting = screen.getByRole("button", { name: /^Move “Launch”/ }) as HTMLButtonElement;
    expect(waiting.disabled).toBe(false);
    expect(waiting.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(waiting);
    expect(props.onMove).toHaveBeenCalledTimes(1);
    // Moved: the very button, now Move back, keeps the focus.
    view.rerender(<WriteNotices notices={[moved()]} {...props} />);
    const back = screen.getByRole("button", {
      name: "Move back “Launch” to the earlier part of “Gym”",
    });
    expect(back).toBe(move);
    expect(document.activeElement).toBe(move);
    expect(back.textContent).toBe("Move back");
  });

  // WCAG 2.5.3: saying "click Move back" or "click Keep it there" finds the button.
  it("starts each move button's name with the words it shows", () => {
    const props = { onUndo: vi.fn(), onDismiss: vi.fn(), onMove: vi.fn(), onMoveBack: vi.fn() };
    const view = render(<WriteNotices notices={[offer()]} {...props} />);
    const named = () =>
      screen
        .getAllByRole("button")
        .filter((b) => b.textContent !== "×")
        .map((b) => [b.textContent ?? "", b.getAttribute("aria-label") ?? ""]);
    const offered = named();
    view.rerender(<WriteNotices notices={[moved()]} {...props} />);
    const all = [...offered, ...named()];
    expect(all.map(([shown]) => shown)).toEqual(["Move", "Keep it there", "Move back"]);
    for (const [shown, name] of all) expect(name).toMatch(new RegExp(`^${shown}\\b`));
  });

  // The second click of a double-click on Move, or Enter held on it, lands on the same button, by
  // then Move back.
  it("a double-click's second click, or a held Enter, never presses Move back", () => {
    const props = { onUndo: vi.fn(), onDismiss: vi.fn(), onMove: vi.fn(), onMoveBack: vi.fn() };
    render(<WriteNotices notices={[moved()]} {...props} />);
    const back = screen.getByRole("button", { name: /^Move back/ });
    fireEvent.click(back, { detail: 2 });
    expect(props.onMoveBack).not.toHaveBeenCalled();
    fireEvent.click(back, { detail: 1 });
    expect(props.onMoveBack).toHaveBeenCalledWith(4);
    // A repeat of a held Enter presses nothing (false: its default was prevented); a press does.
    expect(fireEvent.keyDown(back, { key: "Enter", repeat: true })).toBe(false);
    expect(fireEvent.keyDown(back, { key: "Enter" })).toBe(true);
    expect(fireEvent.keyDown(back, { key: "Tab", repeat: true })).toBe(true);
  });

  it("Move back shows only on a moved notice, and holds its window while the pointer or focus is on it", () => {
    const props = {
      onUndo: vi.fn(),
      onDismiss: vi.fn(),
      onMove: vi.fn(),
      onMoveBack: vi.fn(),
      onHold: vi.fn(),
    };
    const view = render(<WriteNotices notices={[offer()]} {...props} />);
    expect(screen.queryByRole("button", { name: /^Move back/ })).toBeNull();
    view.rerender(<WriteNotices notices={[moved()]} {...props} />);
    expect(screen.queryByRole("button", { name: /Keep/ })).toBeNull();
    const back = screen.getByRole("button", { name: /^Move back/ });
    fireEvent.click(back);
    expect(props.onMoveBack).toHaveBeenCalledWith(4);
    const notice = screen.getByRole("status");
    props.onHold.mockClear();
    fireEvent.pointerEnter(notice);
    fireEvent.pointerLeave(notice);
    fireEvent.focus(back);
    // Focus moving within the notice holds on; leaving it lets go.
    fireEvent.blur(back, { relatedTarget: screen.getByRole("button", { name: "Dismiss" }) });
    fireEvent.blur(back, { relatedTarget: document.body });
    expect(props.onHold.mock.calls).toEqual([
      [4, "pointer", true],
      [4, "pointer", false],
      [4, "focus", true],
      [4, "focus", false],
    ]);
  });

  it("without the handlers, no move buttons", () => {
    render(
      <WriteNotices
        notices={[offer(), { ...moved(), id: 5 }]}
        onUndo={vi.fn()}
        onDismiss={vi.fn()}
      />,
    );
    expect(screen.getAllByRole("button").map((b) => b.textContent)).toEqual(["×", "×"]);
  });
});
