// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
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
});
