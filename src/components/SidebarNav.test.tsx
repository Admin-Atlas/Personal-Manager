// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidebar's tab list, end to end through the real right-click menu: what a right-click offers,
// that a choice lands in the sidebar at once, and that the tabs with something extra (Chats' New,
// Review's count) keep it. The rule itself is unit-tested in lib/nav.test.ts.

import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { closeContextMenu } from "../lib/contextMenu";

const gates = vi.hoisted(() => ({ depth: "standard", devMode: false }));

vi.mock("../lib/ipc", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: gates.depth }),
}));
vi.mock("../lib/capabilities", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDevMode: () => ({ devMode: gates.devMode, setDevMode: () => {} }),
}));

import { SidebarNav } from "./SidebarNav";
import { ContextMenuHost } from "./ui/ContextMenuHost";

afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
  localStorage.clear();
  gates.depth = "standard";
  gates.devMode = false;
});

function renderNav(props: Partial<Parameters<typeof SidebarNav>[0]> = {}) {
  return render(
    <>
      <SidebarNav view="focus" onNavigate={() => {}} reviewCount={0} onNew={() => {}} {...props} />
      <ContextMenuHost />
    </>,
  );
}

/** The tab names, in the order the sidebar shows them. */
const tabs = () =>
  [...document.querySelectorAll("nav [data-nav-id]")].map((row) => row.getAttribute("data-nav-id"));
const last = <T,>(list: T[]) => list[list.length - 1];

function rightClick(el: Element, init: MouseEventInit = {}) {
  fireEvent.contextMenu(el, { clientX: 5, clientY: 5, ...init });
}

describe("SidebarNav", () => {
  it("draws the tabs it always had, in order", () => {
    renderNav();
    expect(tabs()).toEqual([
      "focus",
      "chat",
      "calendar",
      "documents",
      "review",
      "teach",
      "graph",
      "pinboard",
    ]);
  });

  it("lights Focus while a project is open, and keeps Chats' New and Review's count", () => {
    const onNew = vi.fn();
    renderNav({ view: "project", reviewCount: 3, onNew });
    expect(screen.getByRole("button", { name: "Focus" }).getAttribute("aria-current")).toBe("page");
    fireEvent.click(screen.getByRole("button", { name: "New conversation" }));
    expect(onNew).toHaveBeenCalledOnce();
    expect(screen.getByRole("button", { name: /Review/ }).textContent).toContain("3");
  });

  it("navigates to a tab's own view", () => {
    const onNavigate = vi.fn();
    renderNav({ onNavigate });
    fireEvent.click(screen.getByRole("button", { name: "Map" }));
    expect(onNavigate).toHaveBeenCalledWith("graph");
  });

  it("hides a tab from its right-click menu, and shows it again from another tab's", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Map" }));
    const menu = screen.getByRole("menu", { name: "Map tab" });
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Hide" }));
    expect(tabs()).not.toContain("graph");

    rightClick(screen.getByRole("button", { name: "Focus" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Show Map" }));
    expect(tabs()).toContain("graph");
  });

  it("pins a tab to the top, and unpins it", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Pinboard" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Pin to the top" }));
    expect(tabs()[0]).toBe("pinboard");

    rightClick(screen.getByRole("button", { name: "Pinboard" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Unpin" }));
    expect(last(tabs())).toBe("pinboard");
  });

  it("hides the learning tools together", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Teach" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Hide Review and Teach" }));
    expect(tabs()).not.toContain("review");
    expect(tabs()).not.toContain("teach");
  });

  it("gives Chats' New the Chats menu, and offers no Hide on Focus", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "New conversation" }));
    expect(screen.getByRole("menu", { name: "Chats tab" })).toBeTruthy();
    act(() => closeContextMenu());

    rightClick(screen.getByRole("button", { name: "Focus" }));
    const menu = screen.getByRole("menu", { name: "Focus tab" });
    expect(within(menu).queryByRole("menuitem", { name: "Hide" })).toBeNull();
  });

  it("on Minimal, leaves the learning tools and Map out until shown, which pins", () => {
    gates.depth = "min";
    renderNav();
    expect(tabs()).toEqual(["focus", "chat", "calendar", "documents", "pinboard"]);
    rightClick(screen.getByRole("button", { name: "Focus" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Show Review and Teach" }));
    expect(tabs().slice(0, 2)).toEqual(["review", "teach"]);
  });

  it("adds Dev only in developer mode, and never offers to hide it", () => {
    renderNav();
    expect(tabs()).not.toContain("dev");
    cleanup();
    gates.devMode = true;
    renderNav();
    expect(last(tabs())).toBe("dev");
    rightClick(screen.getByRole("button", { name: "Dev" }));
    expect(screen.queryByRole("menuitem", { name: "Hide" })).toBeNull();
  });

  it("leaves Shift+right-click to the system menu", () => {
    renderNav();
    rightClick(screen.getByRole("button", { name: "Map" }), { shiftKey: true });
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
