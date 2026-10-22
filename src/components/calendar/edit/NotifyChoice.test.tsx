// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// Send or Don't send (#884, C12): nothing picked until the user picks (R9), the lines the notice
// calls for, and a group of its own wherever it's mounted.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import type { GuestNotice } from "../../../lib/types";
import { NotifyChoice } from "./NotifyChoice";

afterEach(cleanup);

const NOTICE: GuestNotice = {
  updated: 2,
  invited: 1,
  removed: 0,
  told_cancelled: 0,
  cancelled: false,
  people: 3,
  emails: 3,
  complete: true,
};

function show(over: Partial<Parameters<typeof NotifyChoice>[0]> = {}) {
  const onChange = vi.fn();
  render(
    <NotifyChoice
      notice={NOTICE}
      kind="save"
      scope={null}
      from="me@x.org"
      value={null}
      onChange={onChange}
      disabled={false}
      legend="shown"
      {...over}
    />,
  );
  return { onChange };
}

const radio = (name: RegExp) => screen.getByRole("radio", { name }) as HTMLInputElement;

describe("NotifyChoice", () => {
  it("opens with nothing picked, the question as its name and who gets what", () => {
    show();
    const group = screen.getByRole("group", { name: "Send update emails to guests?" });
    expect(radio(/^Send/).checked).toBe(false);
    expect(radio(/^Don't send/).checked).toBe(false);
    expect(within(group).getByText("2 guests already invited will get an update.")).toBeTruthy();
    expect(within(group).getByText("1 guest new to it will get an invitation.")).toBeTruthy();
    // The lines describe the group, so a screen reader hears them on reaching the radios.
    expect(group.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.getElementById(group.getAttribute("aria-describedby") ?? "")?.textContent).toBe(
      "2 guests already invited will get an update.1 guest new to it will get an invitation.",
    );
    expect(radio(/^Send/).labels?.[0].textContent).toBe(
      "SendGoogle sends the emails from me@x.org.",
    );
  });

  it("says the lines a notice calls for, by kind", () => {
    show({ notice: { ...NOTICE, updated: 3, invited: 0, cancelled: true }, kind: "delete" });
    expect(screen.getByRole("group", { name: "Send cancellation emails to guests?" })).toBeTruthy();
    expect(screen.getByText("3 guests will be told it's cancelled.")).toBeTruthy();
    expect(radio(/^Don't send/).labels?.[0].textContent).toMatch(/never get the cancellation:/);
  });

  it("calls back with the pick, and shows what it's given", () => {
    const { onChange } = show();
    fireEvent.click(radio(/^Send/));
    expect(onChange).toHaveBeenLastCalledWith("all");
    fireEvent.click(radio(/^Don't send/));
    expect(onChange).toHaveBeenLastCalledWith("none");
    cleanup();
    show({ value: "none" });
    expect(radio(/^Don't send/).checked).toBe(true);
    expect(radio(/^Send/).checked).toBe(false);
  });

  it("can be off", () => {
    const { onChange } = show({ disabled: true });
    expect(radio(/^Send/).disabled).toBe(true);
    expect(radio(/^Don't send/).disabled).toBe(true);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("keeps the question for screen readers only when a title already asks it", () => {
    show({ legend: "hidden" });
    const legend = screen.getByText("Send update emails to guests?");
    expect(legend.tagName).toBe("LEGEND");
    expect(legend.className).toContain("sr-only");
  });

  // Two at once (a dialog over another) never share a radio name or id, so a label never toggles
  // the other one's radio (C12 critique C1, C2-L4).
  it("is a group of its own wherever it's mounted", () => {
    const first = vi.fn();
    const second = vi.fn();
    render(
      <>
        {[first, second].map((onChange, i) => (
          <NotifyChoice
            key={i}
            notice={NOTICE}
            kind="save"
            scope={null}
            from={null}
            value={null}
            onChange={onChange}
            disabled={false}
            legend="shown"
          />
        ))}
      </>,
    );
    const sends = screen.getAllByRole("radio", { name: /^Send/ }) as HTMLInputElement[];
    expect(sends).toHaveLength(2);
    expect(sends[0].name).not.toBe(sends[1].name);
    expect(sends[0].id).not.toBe(sends[1].id);
    fireEvent.click(sends[1].labels![0]);
    expect(second).toHaveBeenCalledWith("all");
    expect(first).not.toHaveBeenCalled();
  });
});
