// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Month view draws the event being made (#884, C10c) as its ghost: a chip, or a band across days,
// dashed and in its calendar's colour, never greyed as past, named as the new event, beside synced
// events drawn as ever; never folded into "+N more" on a busy day; and at Minimal depth, a dashed
// ring among the dots.

import { cleanup, render, screen } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { DRAFT_EVENT_ID } from "../../../lib/calendar-layout";
import { formatClock } from "../../../lib/format";

let depth = "standard";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth }),
}));

import { MonthView } from "./MonthView";

// jsdom lays nothing out and ships no ResizeObserver, and the view draws its weeks only once it has
// measured its pane: give every element a height, and the pane a scrollTo, then put them back.
let restore: (() => void) | null = null;
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  const height = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight");
  const scrollTo = Object.getOwnPropertyDescriptor(Element.prototype, "scrollTo");
  Object.defineProperty(Element.prototype, "clientHeight", { configurable: true, get: () => 600 });
  Object.defineProperty(Element.prototype, "scrollTo", { configurable: true, value: () => {} });
  restore = () => {
    if (height) Object.defineProperty(Element.prototype, "clientHeight", height);
    if (scrollTo) Object.defineProperty(Element.prototype, "scrollTo", scrollTo);
    else delete (Element.prototype as { scrollTo?: unknown }).scrollTo;
  };
});
afterAll(() => restore?.());

afterEach(cleanup);

const ev = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "home",
  summary: "Synced",
  description: null,
  location: null,
  start: "2026-10-12T09:00:00",
  end: "2026-10-12T10:00:00",
  all_day: false,
  html_link: null,
  uid: null,
  ...over,
});

// A chip says its time in the reader's own clock (09:00 here, 09:00 AM on CI's runner), so the names
// are built with the helper the view uses and hold in any locale.
const clock = (h: number) => formatClock(new Date(2026, 9, 13, h));

function month(events: CalendarEvent[]) {
  render(
    <MonthView
      cursor={new Date(2026, 9, 1)}
      events={events}
      colorOf={(id) => (id === "work" ? "#336699" : "#993366")}
      onFocusDate={() => {}}
      // Every event here is over by now: a synced one greys, the ghost doesn't.
      now={new Date(2026, 9, 20)}
      onEventClick={vi.fn()}
    />,
  );
}

describe("MonthView", () => {
  it("draws the new event's ghost dashed in its calendar's colour, as a chip or a band", () => {
    month([
      ev({}),
      ev({
        id: DRAFT_EVENT_ID,
        calendar_id: "work",
        summary: "Dentist",
        start: "2026-10-13T11:00:00",
      }),
    ]);
    // Named as the new event, not yet saved, so it isn't heard as one that's in the calendar.
    const chip = screen.getByRole("button", {
      name: `New event, not saved yet: ${clock(11)}, Dentist`,
    });
    // Dashed inside its edge: a cell's list clips its chips, and would take an outer dash away.
    expect(chip.className).toContain("outline-dashed");
    expect(chip.className).toContain("-outline-offset-1");
    expect(chip.className).not.toContain("opacity-60");
    expect(chip.hasAttribute("data-draft-event")).toBe(true);
    expect(chip.style.borderLeftColor).toBe("rgb(51, 102, 153)");
    const synced = screen.getByRole("button", { name: `${clock(9)}, Synced` });
    expect(synced.className).not.toContain("outline-dashed");
    expect(synced.className).toContain("opacity-60");
    cleanup();
    month([
      {
        ...ev({
          id: DRAFT_EVENT_ID,
          calendar_id: "work",
          summary: "Away",
          start: "2026-10-13",
          end: "2026-10-16",
          all_day: true,
        }),
        draft_state: "saving",
      } as CalendarEvent,
    ]);
    const band = screen.getByRole("button", { name: "New event, saving: Away" });
    expect(band.className).toContain("outline-dashed");
    expect(band.className).toContain("-outline-offset-1");
    expect(band.className).not.toContain("opacity-60");
    expect(band.hasAttribute("data-draft-event")).toBe(true);
  });

  // Three events earlier in the day fill the cell: the ghost, later than all of them, still shows as
  // it's typed, in the last place, and the count stays honest.
  it("keeps the ghost on a busy day, in the last place, rather than in +N more", () => {
    month([
      ev({ id: "a", summary: "One", start: "2026-10-13T08:00:00" }),
      ev({ id: "b", summary: "Two", start: "2026-10-13T09:00:00" }),
      ev({ id: "c", summary: "Three", start: "2026-10-13T10:00:00" }),
      ev({ id: DRAFT_EVENT_ID, summary: "Dentist", start: "2026-10-13T15:00:00" }),
    ]);
    expect(
      screen.getByRole("button", { name: `New event, not saved yet: ${clock(15)}, Dentist` }),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: `${clock(10)}, Three` })).toBeNull();
    expect(screen.getByText("+1 more")).toBeTruthy();
  });

  // Minimal depth draws dots: the ghost's is a dashed ring, not a fill, carries the mark that keeps a
  // click on it from closing quick create, and is the one dot with a name. On a day with more dots
  // than fit, it takes the last place rather than folding into the count.
  it("draws the ghost as a dashed ring among the dots at Minimal depth", () => {
    depth = "min";
    try {
      month([
        ...[8, 9, 10, 11, 12].map((h) =>
          ev({ id: `s${h}`, start: `2026-10-12T${String(h).padStart(2, "0")}:00:00` }),
        ),
        ev({
          id: DRAFT_EVENT_ID,
          calendar_id: "work",
          summary: "Dentist",
          start: "2026-10-12T15:00:00",
        }),
      ]);
      const ghost = document.querySelector("[data-draft-event]");
      expect(ghost).not.toBeNull();
      const ring = ghost!.querySelector<HTMLElement>("[aria-hidden]")!;
      expect(ring.className).toContain("border-dashed");
      expect(ring.style.borderColor).toBe("rgb(51, 102, 153)");
      expect(ring.style.backgroundColor).toBe("");
      expect(ghost!.textContent).toBe("New event, not saved yet: Dentist");
      // A synced event's dot is filled, as ever, and has no mark: four of five, and one counted.
      const dots = document.querySelectorAll<HTMLElement>("span[aria-hidden]");
      const filled = [...dots].filter((d) => d.style.backgroundColor === "rgb(153, 51, 102)");
      expect(filled).toHaveLength(4);
      expect(filled.every((d) => d.closest("[data-draft-event]") === null)).toBe(true);
      expect(screen.getByText("+1")).toBeTruthy();
    } finally {
      depth = "standard";
    }
  });
});
