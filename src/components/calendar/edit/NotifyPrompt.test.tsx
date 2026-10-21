// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// The email question on its own (#884, C12): Save waits for an answer (the plan's "disabled until
// answered"), focus starts on Back, and nothing is answered for the user. jsdom never turns Enter
// into a click, so "Enter on open answers nothing" is pinned as focus on Back, Back going back, and
// Enter sent to the dialog choosing nothing.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { GuestNotice } from "../../../lib/types";
import { NotifyPrompt } from "./NotifyPrompt";

afterEach(cleanup);

const NOTICE: GuestNotice = {
  updated: 1,
  invited: 0,
  removed: 0,
  cancelled: false,
  people: 1,
  emails: 1,
  complete: true,
};

function ask(over: Partial<Parameters<typeof NotifyPrompt>[0]> = {}) {
  const onSend = vi.fn();
  const onBack = vi.fn();
  render(
    <NotifyPrompt
      notice={NOTICE}
      kind="save"
      from="me@x.org"
      again={false}
      onSend={onSend}
      onBack={onBack}
      {...over}
    />,
  );
  return { onSend, onBack };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;
const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

describe("NotifyPrompt", () => {
  it("asks Google's question, with Save off until it's answered", () => {
    const { onSend } = ask();
    expect(screen.getByRole("dialog", { name: "Send update emails to guests?" })).toBeTruthy();
    expect(screen.getByText("1 guest already invited will get an update.")).toBeTruthy();
    const save = button("Save");
    expect(save.disabled).toBe(true);
    fireEvent.click(save);
    expect(onSend).not.toHaveBeenCalled();
    fireEvent.click(radio(/^Send/));
    expect(save.disabled).toBe(false);
  });

  it("starts on Back, so Enter on open goes back and answers nothing", () => {
    const { onSend, onBack } = ask();
    const back = button("Back");
    expect(document.activeElement).toBe(back);
    expect(screen.getAllByRole("radio")).not.toContain(document.activeElement);
    fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    expect(onSend).not.toHaveBeenCalled();
    fireEvent.click(back);
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("goes back on Escape", () => {
    const { onSend, onBack } = ask();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onBack).toHaveBeenCalledTimes(1);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("sends Send as all and Don't send as none, once", () => {
    const first = ask();
    fireEvent.click(radio(/^Send/));
    const save = button("Save");
    fireEvent.click(save);
    fireEvent.click(save);
    expect(first.onSend).toHaveBeenCalledTimes(1);
    expect(first.onSend).toHaveBeenCalledWith("all");
    cleanup();
    const second = ask();
    fireEvent.click(radio(/^Don't send/));
    fireEvent.click(button("Save"));
    expect(second.onSend).toHaveBeenCalledWith("none");
  });

  it("names its confirm by what it does, a delete's in the danger colour", () => {
    ask({ kind: "delete", notice: { ...NOTICE, cancelled: true } });
    expect(
      screen.getByRole("dialog", { name: "Send cancellation emails to guests?" }),
    ).toBeTruthy();
    expect(button("Delete").getAttribute("data-variant")).toBe("danger");
    cleanup();
    ask({ kind: "create", notice: { ...NOTICE, updated: 0, invited: 2, people: 2, emails: 2 } });
    expect(screen.getByRole("dialog", { name: "Send invitation emails to guests?" })).toBeTruthy();
    expect(button("Create").getAttribute("data-variant")).toBe("primary");
    expect(screen.getByText(/never get the invitation:/)).toBeTruthy();
  });

  it("says when the numbers changed since the user chose, without guessing why", () => {
    ask({ again: true });
    expect(
      screen.getByText(
        "PM checked with Google again, and these numbers have changed since you chose. Choose again.",
      ),
    ).toBeTruthy();
    cleanup();
    ask();
    expect(screen.queryByText(/numbers have changed/)).toBeNull();
  });
});
