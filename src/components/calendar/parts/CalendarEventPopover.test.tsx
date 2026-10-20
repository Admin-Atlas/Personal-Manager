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

import { act, cleanup, fireEvent, render } from "@testing-library/react";
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

  // CalendarView's Ctrl+Z closes the panel when it brings an event back (#884, F5): focus comes back
  // to the chip from the panel, but someone already back in the grid is left where they are.
  it("hands focus back when closed from outside, only if it was in the panel", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();
    const closeRef = { current: null as (() => void) | null };
    const props = {
      event: calendarEvent(),
      anchor: rect(100),
      calendar: null,
      color: "#000",
      milestone: null,
      onClose,
      closeRef,
    };

    const view = render(<CalendarEventPopover {...props} />);
    expect(view.getByRole("dialog").contains(document.activeElement)).toBe(true);
    closeRef.current!();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
    view.unmount();
    expect(closeRef.current).toBeNull();

    render(<CalendarEventPopover {...props} />);
    const elsewhere = chip("Standup");
    elsewhere.focus();
    closeRef.current!();
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(document.activeElement).toBe(elsewhere);
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

  // #884 C7: an iCal series says how it repeats; a row still holding a raw rule from before says
  // only "Repeats", as does one with no summary. A Google occurrence holds no rule: its words come
  // from Google through `describeSeries` (C9b-2, below), and without it it reads "Repeats" too.
  it.each([
    ["Weekly on Monday", "Weekly on Monday"],
    ["FREQ=WEEKLY;BYDAY=MO", "Repeats"],
    [null, "Repeats"],
  ])("shows the repeat line for a summary of %s", (summary, expected) => {
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent({ recurring: true, recurrence_summary: summary })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    const text = getByRole("dialog").textContent ?? "";
    expect(text).toContain(expected);
    expect(text).not.toContain("FREQ=");
  });

  describe("how a Google occurrence repeats (C9b-2)", () => {
    const occurrence = (id = "e1") =>
      calendarEvent({ id, recurring: true, series_id: "abc", summary: "Gym" });

    /** A `describeSeries` whose answers the test gives, one per call, in order. */
    function deferred() {
      const answers: Array<{ resolve: (w: string | null) => void; reject: (e: Error) => void }> =
        [];
      const ask = vi.fn<(id: string) => Promise<string | null>>(
        () =>
          new Promise<string | null>((resolve, reject) => {
            answers.push({ resolve, reject });
          }),
      );
      return { ask, answers };
    }

    function popover(
      event: CalendarEvent,
      anchor: DOMRect,
      ask?: (id: string) => Promise<string | null>,
    ) {
      return (
        <CalendarEventPopover
          event={event}
          anchor={anchor}
          calendar={null}
          color="#000"
          milestone={null}
          onClose={() => {}}
          describeSeries={ask}
        />
      );
    }

    it('a Google occurrence reads "Repeats", then how it repeats', async () => {
      const { ask, answers } = deferred();
      const view = render(popover(occurrence(), rect(100), ask));
      expect(view.getByText("Repeats")).toBeTruthy();
      expect(ask).toHaveBeenCalledTimes(1);
      expect(ask).toHaveBeenCalledWith("e1");
      await act(async () => answers[0].resolve("Weekly on Monday"));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      expect(view.queryByText("Repeats")).toBeNull();
    });

    it('a failed or empty answer keeps "Repeats"', async () => {
      for (const answer of [new Error("offline"), null, "  "]) {
        const { ask, answers } = deferred();
        const view = render(popover(occurrence(), rect(100), ask));
        await act(async () =>
          answer instanceof Error ? answers[0].reject(answer) : answers[0].resolve(answer),
        );
        expect(view.getByText("Repeats")).toBeTruthy();
        view.unmount();
      }
    });

    it("a row with its own words, or a one-off, never asks", () => {
      const { ask } = deferred();
      const feed = calendarEvent({ recurring: true, recurrence_summary: "Weekly on Monday" });
      const view = render(popover(feed, rect(100), ask));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      view.unmount();
      const once = render(popover(calendarEvent({ series_id: null }), rect(100), ask));
      expect(once.queryByText("Repeats")).toBeNull();
      expect(ask).not.toHaveBeenCalled();
    });

    it("re-pointed at another event, the old words never show", async () => {
      const { ask, answers } = deferred();
      const view = render(popover(occurrence("e1"), rect(100), ask));
      await act(async () => answers[0].resolve("Weekly on Monday"));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      // The keyboard path re-points the mounted panel: the next event reads "Repeats" at once.
      view.rerender(popover(occurrence("e2"), rect(300), ask));
      expect(view.queryByText("Weekly on Monday")).toBeNull();
      expect(view.getByText("Repeats")).toBeTruthy();
      // And an answer for an event no longer shown is dropped.
      view.rerender(popover(occurrence("e3"), rect(500), ask));
      await act(async () => answers[1].resolve("Weekly on Tuesday"));
      expect(view.queryByText("Weekly on Tuesday")).toBeNull();
      await act(async () => answers[2].resolve("Daily"));
      expect(view.getByText("Daily")).toBeTruthy();
    });

    it("the words arriving never move focus", async () => {
      // jsdom has no layout: the panel is taller once the words are in, and opens flipped above a
      // chip near the bottom, so the words move it.
      const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetHeight");
      Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
        configurable: true,
        get(this: HTMLElement) {
          return this.textContent?.includes("Weekly on Monday") ? 300 : 200;
        },
      });
      try {
        const { ask, answers } = deferred();
        const view = render(popover(occurrence(), rect(window.innerHeight - 60), ask));
        const panel = view.getByRole("dialog");
        const before = panel.style.top;
        const elsewhere = chip("Standup");
        elsewhere.focus();
        await act(async () => answers[0].resolve("Weekly on Monday"));
        expect(panel.style.top).not.toBe(before);
        expect(document.activeElement).toBe(elsewhere);
      } finally {
        if (height) Object.defineProperty(HTMLElement.prototype, "offsetHeight", height);
      }
    });

    it("re-pointed at another event, the old flags go even when its read fails", async () => {
      eventFlags.mockResolvedValueOnce([{ id: 1, type: "prep-needed" }]);
      const view = render(popover(calendarEvent({ uid: "u1" }), rect(100)));
      await act(async () => {});
      expect(view.getByText("prep needed")).toBeTruthy();
      eventFlags.mockRejectedValueOnce(new Error("locked"));
      view.rerender(popover(calendarEvent({ id: "e2", uid: "u2" }), rect(300)));
      await act(async () => {});
      expect(view.queryByText("prep needed")).toBeNull();
    });
  });

  // #884: CalendarView passes `editing`; FocusUpcoming doesn't, and must stay as it was.
  it("offers no editing at all without the editing prop", () => {
    const { queryByRole, getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(queryByRole("button", { name: "Edit" })).toBeNull();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();
    expect(getByRole("dialog").textContent).not.toContain("PM can");
  });

  it("offers Edit and Delete when the event can change, and says why when it can't", () => {
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    const { getByRole, rerender, queryByRole, getByText } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{ canEdit: true, canDelete: true, reason: null, onEdit, onDelete }}
      />,
    );
    fireEvent.click(getByRole("button", { name: "Edit" }));
    expect(onEdit).toHaveBeenCalledOnce();
    fireEvent.click(getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledOnce();

    // A locked copy: its busy/free can change, but it can't be deleted, and the line says why.
    rerender(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{
          canEdit: true,
          canDelete: false,
          reason: "Google has locked this event's title, time, place and description.",
          onEdit,
          onDelete,
        }}
      />,
    );
    expect(getByRole("button", { name: "Edit" })).toBeTruthy();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();

    rerender(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{
          canEdit: false,
          canDelete: false,
          reason: "Someone else organises this event.",
          onEdit,
          onDelete,
        }}
      />,
    );
    expect(queryByRole("button", { name: "Edit" })).toBeNull();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();
    expect(getByText("Someone else organises this event.")).toBeTruthy();
  });

  // A description is shown line by line: the wrapper's class is what index.css gives `pre-line`
  // (GFM alone would fold single line breaks into spaces).
  it("shows a description inside the wrapper that keeps its lines", () => {
    const { getByText } = render(
      <CalendarEventPopover
        event={calendarEvent({ description: "Agenda\n1. intro\n2. demo" })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    const text = getByText(
      (_, el) =>
        el?.textContent === "Agenda\n1. intro\n2. demo" && el.tagName === "DIV" && !el.className,
    );
    expect(text.closest(".pm-event-desc")).not.toBeNull();
  });

  // The colour an event was given in Google is named, not only shown: the fill's hue alone doesn't
  // carry it for everyone.
  it("names the colour an event was given in Google, and says nothing for one without", () => {
    const { getByText, queryByText, rerender } = render(
      <CalendarEventPopover
        event={calendarEvent({ color_id: "1" })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(getByText("Colour")).toBeTruthy();
    expect(getByText("Lavender")).toBeTruthy();
    rerender(
      <CalendarEventPopover
        event={calendarEvent({ color_id: null })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(queryByText("Colour")).toBeNull();
  });
});
