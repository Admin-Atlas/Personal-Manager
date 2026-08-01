// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The popover's focus contract, and the fact that it is deliberately NOT a modal.
//
// It declared `role="dialog"` and then never moved focus into itself. Event chips are activatable
// from the keyboard, and this panel renders LAST in the calendar's DOM, so opening it with Enter
// left focus on the chip and reaching the panel's own Close / "Join the call" buttons meant tabbing
// forward through every remaining event in the grid. Closing then dropped focus on <body>.
//
// The other half is a guard against a future "fix": adding `aria-modal` or a focus trap here would
// be wrong. There is no scrim, the calendar behind stays live, and clicking a different event is how
// you move between them — Tab must be able to leave.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";

const eventFlags = vi.fn();

vi.mock("../../../lib/ipc", () => ({
  eventFlags: (...a: unknown[]) => eventFlags(...a),
  openUrl: vi.fn(),
}));

// The description is untrusted provider text and renders through the real sanitising boundary in the
// app; here it is stubbed so the test doesn't drag react-markdown in for a fixture with no body.
vi.mock("../../../lib/markdown", () => ({
  Markdown: ({ children }: { children: string }) => <div>{children}</div>,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
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
    mapVisible: true,
    setSystem: () => {},
    setModePref: () => {},
    setAccent: () => {},
    setDepth: () => {},
    setAutoLocation: () => {},
    setTeachVisible: () => {},
  }),
}));

import { CalendarEventPopover } from "./CalendarEventPopover";

function calendarEvent(over: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id: "e1",
    calendar_id: "c1",
    summary: "Design review",
    description: null,
    location: null,
    start: "2026-08-03T10:00:00",
    end: "2026-08-03T11:00:00",
    all_day: false,
    html_link: null,
    uid: null,
    ...over,
  };
}

/** jsdom has no layout, so a chip's rect is supplied directly — which is exactly how the real
 *  calendar drives this panel (a `DOMRect` handed up by whichever chip was activated). */
function rect(top: number): DOMRect {
  return {
    x: 10,
    y: top,
    left: 10,
    top,
    right: 200,
    bottom: top + 24,
    width: 190,
    height: 24,
    toJSON: () => ({}),
  } as DOMRect;
}

/** An event chip: focusable, and the thing focus has to come back to. */
function chip(label: string): HTMLElement {
  const el = document.createElement("div");
  el.setAttribute("role", "button");
  el.tabIndex = 0;
  el.textContent = label;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  eventFlags.mockResolvedValue([]);
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

describe("CalendarEventPopover", () => {
  it("moves focus into the panel when it opens", () => {
    const opener = chip("Design review");
    opener.focus();

    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    const panel = getByRole("dialog");
    expect(panel.contains(document.activeElement)).toBe(true);
  });

  it("hands focus back to the chip on Escape", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();

    render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={onClose}
      />,
    );

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
  });

  it("hands focus back to the chip on Close", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();

    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={onClose}
      />,
    );

    fireEvent.click(getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
  });

  it("re-points at the NEW chip when the panel is re-used without closing", () => {
    // The keyboard path: activating another chip with Enter swaps `event`/`anchor` on the mounted
    // instance rather than unmounting it (the mouse path closes first, via the outside-mousedown
    // dismissal). A capture-on-mount opener would hand focus back to the FIRST chip of the session.
    const first = chip("Design review");
    const second = chip("Standup");
    first.focus();

    const { rerender } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    second.focus();
    rerender(
      <CalendarEventPopover
        event={calendarEvent({ id: "e2", summary: "Standup" })}
        anchor={rect(300)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.activeElement).toBe(second);
  });

  it("is a NON-modal dialog: no aria-modal", () => {
    // A guard, not a description. Turning this into a modal would put a trap on a panel that sits
    // over a live calendar, and a scrim over the grid you click to move between events.
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    expect(getByRole("dialog").getAttribute("aria-modal")).toBeNull();
  });

  it("does not restore focus on an outside click", () => {
    // The outside click has ALREADY moved focus to whatever was clicked; restoring would yank it
    // straight back off the thing the user just aimed at.
    const opener = chip("Design review");
    opener.focus();
    const elsewhere = chip("Somewhere else");

    render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    elsewhere.focus();
    fireEvent.mouseDown(elsewhere);
    expect(document.activeElement).toBe(elsewhere);
  });

  it("still names itself when the event has no title", () => {
    // Nothing pins the "summary is never blank" rule at the type — it is three separate producer-side
    // guarantees (Google's parse, the milestone suffix, the pinboard fallback), one of them in Rust.
    // A dialog announced with no name is a WCAG failure, so the naming lives at the consumer.
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent({ summary: "   " })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    const panel = getByRole("dialog");
    expect(panel.getAttribute("aria-label")).toBe("(no title)");
    // The visible heading says the same thing, rather than rendering as an empty line.
    expect(panel.querySelector("h2")?.textContent).toBe("(no title)");
  });

  it("names itself from the summary when there is one", () => {
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    expect(getByRole("dialog").getAttribute("aria-label")).toBe("Design review");
  });
});
