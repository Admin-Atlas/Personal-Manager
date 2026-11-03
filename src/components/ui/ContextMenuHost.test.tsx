// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ContextMenuHost } from "./ContextMenuHost";
import { closeContextMenu, openContextMenu } from "../../lib/contextMenu";

vi.mock("../../lib/ipc", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

const at = { clientX: 10, clientY: 20, preventDefault: () => {} };

describe("ContextMenuHost", () => {
  it("draws a surface's menu, runs a choice and closes", () => {
    const onSelect = vi.fn();
    render(<ContextMenuHost />);
    act(() =>
      openContextMenu(at, "Note", [
        { label: "Duplicate", onSelect },
        { kind: "separator" },
        { label: "Delete", danger: true, disabled: true, onSelect: () => {} },
      ]),
    );
    const menu = screen.getByRole("menu", { name: "Note" });
    expect((screen.getByRole("menuitem", { name: "Delete" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(onSelect).toHaveBeenCalledOnce();
    expect(menu.isConnected).toBe(false);
  });

  it("moves with the arrow keys and closes on Escape, giving focus back", () => {
    render(
      <>
        <button>before</button>
        <ContextMenuHost />
      </>,
    );
    const before = screen.getByRole("button", { name: "before" });
    before.focus();
    act(() =>
      openContextMenu(at, "Board", [
        { label: "One", onSelect: () => {} },
        { label: "Two", onSelect: () => {} },
      ]),
    );
    const menu = screen.getByRole("menu");
    expect(document.activeElement).toBe(menu);
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Two" }));
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "One" }));
    fireEvent.keyDown(menu, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(before);
  });

  it("closes on a press outside it", () => {
    render(<ContextMenuHost />);
    act(() => openContextMenu(at, "Board", [{ label: "One", onSelect: () => {} }]));
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("swaps the page's own menu for PM's: a link's Open and Copy", () => {
    render(
      <>
        <a href="https://example.com/">site</a>
        <p>plain</p>
        <ContextMenuHost />
      </>,
    );
    const onLink = fireEvent.contextMenu(screen.getByText("site"));
    expect(onLink).toBe(false); // prevented: no system menu
    screen.getByRole("menuitem", { name: "Open link" });
    act(() => closeContextMenu());
    // Nothing to offer on plain text with nothing selected, and still no system menu.
    expect(fireEvent.contextMenu(screen.getByText("plain"))).toBe(false);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("leaves text fields and Shift+right-click to the system menu", () => {
    render(
      <>
        <input aria-label="field" />
        <p>plain</p>
        <ContextMenuHost />
      </>,
    );
    expect(fireEvent.contextMenu(screen.getByLabelText("field"))).toBe(true);
    expect(fireEvent.contextMenu(screen.getByText("plain"), { shiftKey: true })).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("leaves a right-click a surface already answered alone", () => {
    render(
      <>
        <div onContextMenu={(e) => e.preventDefault()}>claimed</div>
        <ContextMenuHost />
      </>,
    );
    fireEvent.contextMenu(screen.getByText("claimed"));
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
