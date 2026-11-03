// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The board's right-click menus (#1038), on the whole view: the grid's (a new card where you
// clicked), a card's (its own actions), and what's left to the system menu.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { PinboardView } from "./PinboardView";
import { ContextMenuHost } from "./ui/ContextMenuHost";
import { closeContextMenu } from "../lib/contextMenu";
import { BOARD_VERSION, type Board } from "../lib/pinboard/types";

const ipc = vi.hoisted(() => ({
  getPref: vi.fn(),
  setPref: vi.fn(() => Promise.resolve()),
  listDocuments: vi.fn(() => Promise.resolve([])),
  listProjects: vi.fn(() => Promise.resolve([])),
  listMilestones: vi.fn(() => Promise.resolve([])),
  openUrl: vi.fn(() => Promise.resolve()),
}));
vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  ...ipc,
}));

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
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

// jsdom lays nothing out; ProseMirror measures when it scrolls a selection into view, and the board
// measures its scroller.
beforeAll(() => {
  const rects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
  Range.prototype.getClientRects = rects;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
  Element.prototype.getClientRects = rects;
  document.elementFromPoint = () => null;
  Element.prototype.scrollTo = () => {};
  // A 1000×800 box for everything, so the board has room: 39 cells across with no padding (jsdom
  // gives the scroller none), at 0,0.
  Element.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 1000, 800);
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

const BOARD: Board = {
  version: BOARD_VERSION,
  widgets: [
    { id: "n1", kind: "note", rect: { x: 0, y: 0, w: 7, h: 5 }, title: "Plan", text: "hi" },
  ],
};

beforeEach(() => {
  ipc.getPref.mockResolvedValue(JSON.stringify(BOARD));
});
afterEach(() => {
  act(() => closeContextMenu());
  cleanup();
});

async function mount() {
  const utils = render(
    <>
      <PinboardView />
      <ContextMenuHost />
    </>,
  );
  await act(async () => {});
  const grid = utils.container.querySelector("[data-help=pinboard-board]") as HTMLElement;
  const tile = utils.container.querySelector("[data-widget-id=n1]") as HTMLElement;
  return { ...utils, grid, tile };
}

/** Each row's name: its label, or a swatch's colour name. A shortcut hint beside a label isn't part of
 *  it. */
const items = () =>
  [...screen.getByRole("menu").querySelectorAll("[role^=menuitem]")].map(
    (b) => b.getAttribute("aria-label") ?? b.querySelector("span")?.textContent,
  );

describe("PinboardView right-click", () => {
  it("offers a new card where you right-click the empty grid", async () => {
    const { grid, container } = await mount();
    // jsdom puts the grid at 0,0: (130, 70) is cell (5, 2).
    expect(fireEvent.contextMenu(grid, { clientX: 130, clientY: 70 })).toBe(false);
    expect(screen.getByRole("menu", { name: "Pinboard" })).toBeTruthy();
    expect(items()).toEqual([
      "New note here",
      "New timeline here",
      "New task list here",
      "New folder here",
      "Undo",
      "Redo",
    ]);
    // Nothing done yet on this visit, so nothing to undo.
    expect((screen.getByRole("menuitem", { name: /Undo/ }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    fireEvent.click(screen.getByRole("menuitem", { name: "New note here" }));
    // The original note covers cells 0-6 across, so a note at (5, 2) would overlap it: it takes the
    // first free slot instead.
    const tiles = container.querySelectorAll("[data-widget-id]");
    expect(tiles).toHaveLength(2);
  });

  it("puts the new card at the spot when it's clear", async () => {
    const { grid, container } = await mount();
    fireEvent.contextMenu(grid, { clientX: 24 * 10 + 5, clientY: 24 * 8 + 5 });
    fireEvent.click(screen.getByRole("menuitem", { name: "New timeline here" }));
    const tiles = [...container.querySelectorAll<HTMLElement>("[data-widget-id]")];
    const added = tiles.find((t) => t.dataset.widgetId !== "n1")!;
    expect(added.style.left).toBe(`${24 * 10}px`);
    expect(added.style.top).toBe(`${24 * 8}px`);
  });

  it("offers a card's own actions on the card, and Duplicate copies it", async () => {
    const { tile, container } = await mount();
    const grip = tile.querySelector("[data-help]") ?? tile;
    // The header's bar, not its title field (which keeps the system menu).
    fireEvent.contextMenu(grip.closest("[data-widget-id]")!, { clientX: 5, clientY: 5 });
    expect(screen.getByRole("menu", { name: "Plan" })).toBeTruthy();
    expect(items()).toEqual([
      "Rename",
      "Duplicate",
      "Sage",
      "Coral",
      "Amber",
      "Stone",
      "Teal",
      "Delete",
    ]);
    fireEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    expect(container.querySelectorAll("[data-widget-id]")).toHaveLength(2);
  });

  it("leaves a card's title and its text to the system menu", async () => {
    const { tile } = await mount();
    expect(fireEvent.contextMenu(tile.querySelector("input")!)).toBe(true);
    expect(fireEvent.contextMenu(tile.querySelector(".ProseMirror p")!)).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });
});
