// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The editor's Colour (#884, C13): a button naming the colour opens a listbox with the calendar's
// colour and Google's eleven, in Google's order; opening puts focus on the colour chosen, the arrow
// keys and Home/End move through all of them, Enter, Space or a click chooses and closes, Tab and
// Escape close, and focus always goes back to the button. A colour PM doesn't know is the current,
// named choice; a label's colour is said to be the label's.

import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ColourChoice } from "../../../lib/types";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { EventColorPicker } from "./EventColorPicker";

// jsdom ships no ResizeObserver, and the list's popover (escapeClipping) builds one.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

afterEach(() => {
  cleanup();
});

/** The picker in a dialog, as the editor has it, holding what was chosen. */
function harness(start: ColourChoice, opts: { labelled?: boolean; disabled?: boolean } = {}) {
  const chosen: ColourChoice[] = [];
  function Host() {
    const [value, setValue] = useState(start);
    return (
      <div role="dialog" aria-label="Edit event">
        <EventColorPicker
          value={value}
          calendarColour="var(--ink4)"
          labelled={opts.labelled ?? false}
          disabled={opts.disabled ?? false}
          onChange={(next) => {
            chosen.push(next);
            setValue(next);
          }}
        />
      </div>
    );
  }
  render(<Host />);
  return chosen;
}

/** Open the list from its button, and wait for focus to land in it. */
async function openList(name: string) {
  const trigger = screen.getByRole("button", { name });
  fireEvent.click(trigger);
  const list = screen.getByRole("listbox", { name: "Colour" });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0));
  });
  return { trigger, list };
}

const ORDER = [
  "Calendar colour",
  "Tomato",
  "Flamingo",
  "Tangerine",
  "Banana",
  "Sage",
  "Basil",
  "Peacock",
  "Blueberry",
  "Lavender",
  "Grape",
  "Graphite",
];

describe("EventColorPicker", () => {
  it("is a named listbox of the calendar's colour and Google's eleven, in Google's order", async () => {
    harness({ kind: "calendar" });
    const { trigger, list } = await openList("Colour: Calendar colour");
    expect(trigger.getAttribute("aria-haspopup")).toBe("listbox");
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    const options = within(list).getAllByRole("option");
    expect(options.map((o) => o.getAttribute("aria-label"))).toEqual(ORDER);
    expect(options.map((o) => o.getAttribute("aria-selected"))).toEqual([
      "true",
      ...Array(11).fill("false"),
    ]);
  });

  it("opens on the colour chosen, with one tab stop, and the arrows, Home and End move", async () => {
    harness({ kind: "colour", id: "5" });
    const { list } = await openList("Colour: Banana");
    const option = (name: string) => within(list).getByRole("option", { name });
    expect(document.activeElement).toBe(option("Banana"));
    expect(
      within(list)
        .getAllByRole("option")
        .filter((o) => o.tabIndex === 0),
    ).toHaveLength(1);
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    expect(document.activeElement).toBe(option("Sage"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(option("Basil"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    fireEvent.keyDown(document.activeElement!, { key: "ArrowLeft" });
    expect(document.activeElement).toBe(option("Banana"));
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(document.activeElement).toBe(option("Graphite"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    expect(document.activeElement).toBe(option("Graphite"));
    // Still the one tab stop at the end: the list never loses it past either end.
    expect(option("Graphite").tabIndex).toBe(0);
    fireEvent.keyDown(document.activeElement!, { key: "Home" });
    expect(document.activeElement).toBe(option("Calendar colour"));
    // Moving chooses nothing, and the name of the one under focus shows under the list.
    expect(option("Banana").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText("Calendar colour", { selector: "p" })).toBeTruthy();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    expect(screen.getByText("Tomato", { selector: "p" })).toBeTruthy();
  });

  it("Enter chooses and closes, focus back on the button", async () => {
    const chosen = harness({ kind: "calendar" });
    await openList("Colour: Calendar colour");
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    fireEvent.keyDown(document.activeElement!, { key: "Enter" });
    expect(chosen).toEqual([{ kind: "colour", id: "11" }]);
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Colour: Tomato" }));
  });

  it("a click chooses and closes; the colour already chosen reports nothing", async () => {
    const chosen = harness({ kind: "colour", id: "1" });
    let { list } = await openList("Colour: Lavender");
    fireEvent.click(within(list).getByRole("option", { name: "Lavender" }));
    expect(chosen).toEqual([]);
    expect(screen.queryByRole("listbox")).toBeNull();
    ({ list } = await openList("Colour: Lavender"));
    fireEvent.click(within(list).getByRole("option", { name: "Calendar colour" }));
    expect(chosen).toEqual([{ kind: "calendar" }]);
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Colour: Calendar colour" }),
    );
  });

  it("Tab and Escape close it without a choice, focus back on the button", async () => {
    const chosen = harness({ kind: "calendar" });
    const { trigger } = await openList("Colour: Calendar colour");
    fireEvent.keyDown(document.activeElement!, { key: "ArrowRight" });
    fireEvent.keyDown(document.activeElement!, { key: "Tab" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    await openList("Colour: Calendar colour");
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(document.activeElement).toBe(trigger);
    expect(chosen).toEqual([]);
  });

  it("checks the chosen swatch in the colour's own text colour", async () => {
    harness({ kind: "colour", id: "3" });
    const { list } = await openList("Colour: Grape");
    const grape = within(list).getByRole("option", { name: "Grape" });
    expect(grape.textContent).toBe("✓");
    expect((grape.firstElementChild as HTMLElement).style.color).toBe("rgb(255, 255, 255)");
    expect(within(list).getByRole("option", { name: "Sage" }).textContent).toBe("");
  });

  it("a colour PM doesn't know is the current choice, named, and can't be chosen again", async () => {
    const chosen = harness({ kind: "colour", id: "12" });
    const { list } = await openList("Colour: A colour PM doesn't know");
    const unknown = within(list).getByRole("option", { name: "A colour PM doesn't know" });
    expect(within(list).getAllByRole("option")[0]).toBe(unknown);
    expect(unknown.getAttribute("aria-selected")).toBe("true");
    expect(unknown.getAttribute("aria-disabled")).toBe("true");
    // The arrows land on it, so its name is heard.
    expect(document.activeElement).toBe(unknown);
    fireEvent.keyDown(unknown, { key: "Enter" });
    fireEvent.click(unknown);
    expect(chosen).toEqual([]);
    expect(screen.getByRole("listbox")).toBeTruthy();
  });

  it("a labelled event's button says the label sets the colour; off, it doesn't open", () => {
    harness({ kind: "colour", id: "5" }, { labelled: true, disabled: true });
    const trigger = screen.getByRole<HTMLButtonElement>("button", {
      name: "Colour: Set by its Google Calendar label",
    });
    expect(trigger.disabled).toBe(true);
    fireEvent.click(trigger);
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("turned off while open (a save on its way), it closes for good", async () => {
    const props = {
      value: { kind: "calendar" } as ColourChoice,
      calendarColour: "var(--ink4)",
      labelled: false,
      onChange: () => {},
    };
    const view = render(<EventColorPicker {...props} disabled={false} />);
    fireEvent.click(screen.getByRole("button", { name: "Colour: Calendar colour" }));
    expect(screen.getByRole("listbox")).toBeTruthy();
    view.rerender(<EventColorPicker {...props} disabled />);
    expect(screen.queryByRole("listbox")).toBeNull();
    // Back on, it stays closed until opened again.
    view.rerender(<EventColorPicker {...props} disabled={false} />);
    await act(async () => {});
    expect(screen.queryByRole("listbox")).toBeNull();
  });
});
