// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { WriteNotices } from "./WriteNotices";
import type { WriteNotice } from "./useEventWrites";

afterEach(cleanup);

const deleting = (id: number, summary: string): WriteNotice => ({
  id,
  tone: "ok",
  text: `Deleting “${summary}”.`,
  undoToken: `t${id}`,
  undoLabel: `Undo deleting “${summary}”`,
});

describe("WriteNotices", () => {
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
});
