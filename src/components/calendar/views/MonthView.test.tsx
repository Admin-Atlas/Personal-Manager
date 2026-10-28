// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Month view draws the event being made (#884, C10c) as its ghost: a chip, or a band across days,
// dashed and in its calendar's colour, never greyed as past, named as the new event, beside synced
// events drawn as ever; never folded into "+N more" on a busy day; and at Minimal depth, a dashed
// ring among the dots. On a calendar that drags (C16c) it marks what the drag measures and what a
// press must never create on, draws the ghost over the days it would land on, and stops snapping
// while a drag scrolls it; without that, nothing changes.

import { cleanup, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { DRAFT_EVENT_ID } from "../../../lib/calendar-layout";
import { ghostEvent } from "../../../lib/calendarDrag";
import { formatClock } from "../../../lib/format";
import type { MonthInteraction } from "./MonthView";

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

function month(events: CalendarEvent[], interaction?: MonthInteraction) {
  const view = (i?: MonthInteraction) => (
    <MonthView
      cursor={new Date(2026, 9, 1)}
      events={events}
      colorOf={(id) => (id === "work" ? "#336699" : "#993366")}
      onFocusDate={() => {}}
      // Every event here is over by now: a synced one greys, the ghost doesn't.
      now={new Date(2026, 9, 20)}
      onEventClick={vi.fn()}
      interaction={i}
    />
  );
  const utils = render(view(interaction));
  return { rerender: (i?: MonthInteraction) => utils.rerender(view(i)) };
}

const still: MonthInteraction = {
  originId: null,
  ghost: null,
  pendingIds: new Set(),
  live: false,
};
const stream = () => document.querySelector<HTMLElement>(".overflow-y-auto")!;

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

  // #884 C14b: an event the user declined (the calendar marks it, `markDeclined`) is struck
  // through, as Google draws it, and its name says so; any other is drawn as ever.
  it("strikes through an event the user declined, as a chip or a band, and says so", () => {
    const declined = (e: CalendarEvent) => ({ ...e, declined: true }) as CalendarEvent;
    month([
      declined(ev({ id: "c", summary: "Review", start: "2026-10-13T09:00:00" })),
      ev({ id: "p", summary: "Plain", start: "2026-10-14T09:00:00" }),
      declined(
        ev({ id: "b", summary: "Away", start: "2026-10-20", end: "2026-10-23", all_day: true }),
      ),
    ]);
    const chip = screen.getByRole("button", { name: `${clock(9)}, Review, declined` });
    expect(within(chip).getByText("Review").className).toContain("line-through");
    const plain = screen.getByRole("button", { name: `${clock(9)}, Plain` });
    expect(within(plain).getByText("Plain").className).not.toContain("line-through");
    const band = screen.getByRole("button", { name: "Away, declined" });
    expect(within(band).getByText("Away").className).toContain("line-through");
  });
});

// #884 C16c: Month on a calendar that drags. Each row's text is checked through the attributes and
// classes the drag and the eye read, since jsdom lays nothing out.
describe("MonthView: dragging", () => {
  const busy = [
    ev({ id: "a", summary: "One", start: "2026-10-13T08:00:00" }),
    ev({ id: "b", summary: "Two", start: "2026-10-13T09:00:00" }),
    ev({ id: "c", summary: "Three", start: "2026-10-13T10:00:00" }),
    ev({ id: "d", summary: "Four", start: "2026-10-13T11:00:00" }),
    ev({ id: "w", summary: "Away", start: "2026-10-20", end: "2026-10-23", all_day: true }),
  ];

  it("without interaction nothing changes: no marks, and the weeks snap", () => {
    month(busy);
    expect(
      document.querySelector("[data-surface], [data-event-id], [data-no-create], [data-day]"),
    ).toBeNull();
    expect(stream().className).toContain("snap-y");
    expect(stream().className).not.toContain("select-none");
  });

  it("marks the weeks, each day, and each chip and band with its event", () => {
    month(busy, still);
    const s = stream();
    expect(s.dataset.surface).toBe("month");
    // 20 months before October 2026, from its Monday; a sixth of the 600 px pane a week.
    expect(s.dataset.gridStart).toBe("2025-01-27");
    expect(s.dataset.weekH).toBe("100");
    // One row a week, and the drag knows how many.
    expect(Number(s.dataset.weeks)).toBe(s.children.length);
    expect(s.children.length).toBeGreaterThan(170);
    expect(s.className).toContain("select-none");
    expect(document.querySelector("[data-day='2026-10-13']")).not.toBeNull();
    const chip = screen.getByRole("button", { name: `${clock(8)}, One` });
    expect(chip.dataset.eventId).toBe("a");
    const band = screen.getByRole("button", { name: "Away" });
    expect(band.dataset.eventId).toBe("w");
  });

  it("day numbers, month names, more and the counts never create", () => {
    const marked = () =>
      [...document.querySelectorAll<HTMLElement>("[data-no-create]")].map((el) => el.textContent);
    const inCell = (day: string) =>
      [...document.querySelectorAll<HTMLElement>(`[data-day='${day}'] [data-no-create]`)].map(
        (el) => el.textContent,
      );
    month(busy, still);
    expect(inCell("2026-10-13")).toEqual(["13", "+1 more"]);
    const first = new Date(2026, 9, 1).toLocaleDateString(undefined, { month: "short" });
    expect(inCell("2026-10-01")).toEqual([first, "1"]);
    cleanup();
    for (const [d, count] of [
      ["power", "4"],
      ["min", "+1"],
    ] as const) {
      depth = d;
      try {
        month(
          d === "min"
            ? [8, 9, 10, 11, 12, 13].map((h) =>
                ev({ id: `s${h}`, start: `2026-10-13T${String(h).padStart(2, "0")}:00:00` }),
              )
            : busy,
          still,
        );
        expect(inCell("2026-10-13")).toContain(count);
        expect(marked().length).toBeGreaterThan(0);
      } finally {
        depth = "standard";
        cleanup();
      }
    }
  });

  it("a live drag turns snapping off, and it comes back after", () => {
    const { rerender } = month(busy, still);
    expect(stream().className).toContain("snap-y");
    rerender({ ...still, live: true });
    expect(stream().className).toContain("snap-none");
    expect(stream().className).not.toContain("snap-y");
    rerender(still);
    expect(stream().className).toContain("snap-y");
    expect(stream().className).not.toContain("snap-none");
  });

  it("draws the ghost over the days it would land on, in each week it touches", () => {
    const away = busy[4];
    // Moved to Saturday 17-10 to Tuesday 20-10: two days in each of two weeks.
    const event = ghostEvent(away, { kind: "all_day", first: "2026-10-17", last: "2026-10-20" });
    month(busy, {
      ...still,
      live: true,
      originId: away.id,
      ghost: { event, kind: "move", invalid: null, label: "17-10 – 20-10" },
    });
    const ghosts = [...document.querySelectorAll<HTMLElement>("[data-drag-ghost]")];
    expect(ghosts).toHaveLength(2);
    expect(ghosts.map((g) => [parseFloat(g.style.left), parseFloat(g.style.width)])).toEqual([
      [(5 / 7) * 100, (2 / 7) * 100],
      [0, (2 / 7) * 100],
    ]);
    expect(ghosts[0].getAttribute("aria-hidden")).toBe("true");
    expect(ghosts[0].textContent).toBe("Away · 17-10 – 20-10");
  });

  it("the dragged event dims where it was, and one being saved is dashed", () => {
    month(busy, { ...still, live: true, originId: "a", pendingIds: new Set(["w"]) });
    const dragged = screen.getByRole("button", { name: `${clock(8)}, One` });
    expect(dragged.className).toContain("opacity-40");
    expect(dragged.className).not.toContain("opacity-60");
    const saving = screen.getByRole("button", { name: "Away" });
    expect(saving.className).toContain("opacity-70");
    expect(saving.className).toContain("outline-dashed");
    expect(saving.className).not.toContain("opacity-60");
    // Everything else as ever: past, so greyed.
    expect(screen.getByRole("button", { name: `${clock(9)}, Two` }).className).toContain(
      "opacity-60",
    );
  });
});
