// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Day/Week grid draws a timed event that runs past midnight in the hours of every day it
// touches (#884), not in the all-day strip: one piece per day, each opening the same event, and only
// the first a tab stop. All-day events still go to the strip.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { DRAFT_EVENT_ID } from "../../../lib/calendar-layout";
import type { DragCapability } from "../../../lib/calendarDrag";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { TimeGridView, type GridInteraction } from "./TimeGridView";

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

afterEach(cleanup);

// Timed values without a zone read as local time.
const ev = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "c1",
  summary: "Night shift",
  description: null,
  location: null,
  start: "2026-10-12T22:00:00",
  end: "2026-10-13T02:00:00",
  all_day: false,
  html_link: null,
  uid: null,
  ...over,
});

function grid(events: CalendarEvent[], onEventClick = vi.fn()) {
  render(
    <TimeGridView
      days={[new Date(2026, 9, 12), new Date(2026, 9, 13)]}
      events={events}
      colorOf={() => "#336699"}
      range="day"
      bounds={{ startHour: 0, endHour: 24 }}
      zones={[]}
      onZonesChange={() => {}}
      allowZones={false}
      now={new Date(2026, 9, 1)}
      onEventClick={onEventClick}
    />,
  );
  return onEventClick;
}

describe("TimeGridView", () => {
  it("fills an overnight event's hours on both days, not the all-day strip", () => {
    const onEventClick = grid([ev({})]);
    expect(screen.queryByText("all-day")).toBeNull();
    const pieces = screen.getAllByRole("button", { name: /Night shift/ });
    expect(pieces).toHaveLength(2);
    // One tab stop for the event; the second piece is still clickable.
    expect(pieces.map((p) => p.tabIndex)).toEqual([0, -1]);
    fireEvent.click(pieces[1]);
    expect(onEventClick).toHaveBeenCalledWith(
      expect.objectContaining({ id: "e1" }),
      expect.anything(),
    );
    // Its accessible name carries both dates, so either piece says when it is.
    expect(pieces[0].getAttribute("aria-label")).toMatch(/Night shift, .+ – .+/);
  });

  it("still puts an all-day event in the strip", () => {
    grid([
      ev({ id: "a", summary: "Holiday", start: "2026-10-12", end: "2026-10-14", all_day: true }),
    ]);
    expect(screen.getByText("all-day")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "Holiday" })).toHaveLength(1);
  });

  // Google's look: the fill is the colour the event was given, solid, in that colour's text colour; the
  // stripe stays its calendar's.
  it("fills an event given its own colour with it, keeping the calendar's stripe", () => {
    grid([
      ev({
        id: "c",
        summary: "Coloured",
        start: "2026-10-12T09:00:00",
        end: "2026-10-12T10:00:00",
        color_id: "1",
      }),
      ev({ id: "p", summary: "Plain", start: "2026-10-12T11:00:00", end: "2026-10-12T12:00:00" }),
    ]);
    const coloured = screen.getByRole("button", { name: /Coloured/ });
    const plain = screen.getByRole("button", { name: /Plain/ });
    // (jsdom reports a plain colour in rgb() form.)
    expect(coloured.style.background).toBe("rgb(121, 134, 203)");
    expect(coloured.style.borderLeftColor).toBe("rgb(51, 102, 153)");
    expect(within(coloured).getByText("Coloured").style.color).toBe("rgb(31, 31, 31)");
    expect(plain.style.background).toContain("#336699 18%");
    expect(within(plain).getByText("Plain").style.color).toBe("");
  });

  // The event being made (#884, C10c): dashed, in its calendar's colour, and never greyed as past
  // (one opacity utility only: `cn` doesn't resolve two).
  it("draws the new event's ghost dashed in its calendar's colour, timed or all day", () => {
    // Both days are over by `now`, so a synced event greys and the ghost doesn't.
    const draw = (events: CalendarEvent[]) =>
      render(
        <TimeGridView
          days={[new Date(2026, 9, 12), new Date(2026, 9, 13)]}
          events={events}
          colorOf={(id) => (id === "work" ? "#336699" : "#993366")}
          range="day"
          bounds={{ startHour: 0, endHour: 24 }}
          zones={[]}
          onZonesChange={() => {}}
          allowZones={false}
          now={new Date(2026, 9, 20)}
          onEventClick={vi.fn()}
        />,
      );
    draw([
      ev({ id: DRAFT_EVENT_ID, calendar_id: "work", summary: "Dentist", end: null }),
      ev({ id: "e2", summary: "Synced", start: "2026-10-12T09:00:00", end: null }),
    ]);
    // Named as the new event, not yet saved, so it isn't heard as one that's in the calendar.
    const ghost = screen.getByRole("button", { name: /^New event, not saved yet: Dentist, / });
    // Dashed inside its edge, where nothing around it can clip the dash away.
    expect(ghost.className).toContain("outline-dashed");
    expect(ghost.className).toContain("-outline-offset-1");
    expect(ghost.className).not.toContain("opacity-60");
    expect(ghost.hasAttribute("data-draft-event")).toBe(true);
    expect(ghost.style.borderLeftColor).toBe("rgb(51, 102, 153)");
    // A synced event beside it is drawn and named as ever.
    const synced = screen.getByRole("button", { name: /^Synced, / });
    expect(synced.className).not.toContain("outline-dashed");
    expect(synced.className).toContain("opacity-60");
    expect(synced.hasAttribute("data-draft-event")).toBe(false);
    cleanup();
    // Once a save may have made it, never "not saved".
    draw([
      {
        ...ev({
          id: DRAFT_EVENT_ID,
          calendar_id: "work",
          summary: "Away",
          start: "2026-10-13",
          end: "2026-10-14",
          all_day: true,
        }),
        draft_state: "unsure",
      } as CalendarEvent,
    ]);
    const band = screen.getByRole("button", {
      name: "New event, PM may already have made it: Away",
    });
    expect(band.className).toContain("outline-dashed");
    expect(band.className).toContain("-outline-offset-1");
    expect(band.className).not.toContain("opacity-60");
    expect(band.hasAttribute("data-draft-event")).toBe(true);
  });

  // #884 C14b: an event the user declined (the calendar marks it, `markDeclined`) is struck
  // through, as Google draws it, and its name says so; any other is drawn as ever.
  it("strikes through an event the user declined, timed or all day, and says so", () => {
    const declined = (e: CalendarEvent) => ({ ...e, declined: true }) as CalendarEvent;
    grid([
      declined(ev({ id: "t", summary: "Review", start: "2026-10-12T09:00:00", end: null })),
      ev({ id: "p", summary: "Plain", start: "2026-10-12T11:00:00", end: null }),
      declined(
        ev({ id: "a", summary: "Away", start: "2026-10-12", end: "2026-10-13", all_day: true }),
      ),
    ]);
    const card = screen.getByRole("button", { name: /^Review, .*, declined$/ });
    expect(within(card).getByText("Review").className).toContain("line-through");
    const plain = screen.getByRole("button", { name: /^Plain, / });
    expect(plain.getAttribute("aria-label")).not.toMatch(/declined/);
    expect(within(plain).getByText("Plain").className).not.toContain("line-through");
    const band = screen.getByRole("button", { name: "Away, declined" });
    expect(within(band).getByText("Away").className).toContain("line-through");
  });
});

// Dragging (#884, C16b): with an `interaction`, the grid marks what the drag measures, shows the
// ends that may be dragged, the ghost and the pending look, and keeps the all-day row. Without one
// (Focus's grid), nothing of it is drawn.
describe("TimeGridView — on a grid that drags", () => {
  const all: DragCapability = {
    move: true,
    resizeStart: true,
    resizeEnd: true,
    bandEdges: true,
    reason: null,
  };
  const interaction = (over: Partial<GridInteraction> = {}): GridInteraction => ({
    capability: () => all,
    originId: null,
    ghost: null,
    pendingIds: new Set(),
    ...over,
  });
  function draw(events: CalendarEvent[], with_?: GridInteraction, now = new Date(2026, 9, 1)) {
    return render(
      <TimeGridView
        days={[new Date(2026, 9, 12), new Date(2026, 9, 13)]}
        events={events}
        colorOf={() => "#336699"}
        range="day"
        bounds={{ startHour: 0, endHour: 24 }}
        zones={[]}
        onZonesChange={() => {}}
        allowZones={false}
        now={now}
        onEventClick={vi.fn()}
        interaction={with_}
      />,
    );
  }
  const day = (id: string) =>
    ev({ id, summary: id, start: "2026-10-12T09:00:00", end: "2026-10-12T10:00:00" });

  it("without interaction: no edges, no data attributes, no strip at zero all-day events", () => {
    const { container } = draw([ev({})]);
    expect(container.querySelector("[data-event-id], [data-surface], [data-time-body]")).toBeNull();
    expect(container.querySelector("[data-edge]")).toBeNull();
    expect(screen.queryByText("all-day")).toBeNull();
  });

  it("with interaction: edges only where allowed, on the first and last pieces", () => {
    const { container } = draw([ev({})], interaction());
    const pieces = container.querySelectorAll<HTMLElement>('[data-event-id="e1"]');
    expect([...pieces].map((p) => p.dataset.day)).toEqual(["2026-10-12", "2026-10-13"]);
    // The overnight event's top edge is on its first piece, its bottom on its last.
    const edges = (p: HTMLElement) =>
      [...p.querySelectorAll<HTMLElement>("[data-edge]")].map((e) => e.dataset.edge);
    expect(edges(pieces[0])).toEqual(["start"]);
    expect(edges(pieces[1])).toEqual(["end"]);
    cleanup();
    // What may not be resized has no edge to take.
    const { container: still } = draw(
      [day("d")],
      interaction({ capability: () => ({ ...all, resizeStart: false, resizeEnd: false }) }),
    );
    expect(still.querySelector("[data-edge]")).toBeNull();
    expect(still.querySelector('[data-event-id="d"]')).not.toBeNull();
  });

  it("marks the hours and the columns the drag measures", () => {
    const { container } = draw([], interaction());
    const body = container.querySelector<HTMLElement>("[data-time-body]");
    expect(body?.dataset.rowH).toBe("40");
    expect(body?.className).toContain("select-none");
    const cols = container.querySelectorAll<HTMLElement>('[data-surface="grid"]');
    expect([...cols].map((c) => c.dataset.day)).toEqual(["2026-10-12", "2026-10-13"]);
  });

  it("the strip keeps one row with no all-day events", () => {
    const { container } = draw([], interaction());
    const band = container.querySelector<HTMLElement>('[data-surface="band"]');
    expect(band?.dataset.days).toBe("2026-10-12,2026-10-13");
    expect(band?.style.height).toBe("20px");
    expect(screen.getByText("all-day")).toBeTruthy();
  });

  it("an all-day event's ends may be dragged where it really ends", () => {
    const { container } = draw(
      [ev({ id: "a", summary: "Away", start: "2026-10-11", end: "2026-10-14", all_day: true })],
      interaction(),
    );
    const bar = container.querySelector<HTMLElement>('[data-event-id="a"]');
    // It began before the first day on screen: only its right end is its own.
    expect(
      [...(bar?.querySelectorAll<HTMLElement>("[data-edge]") ?? [])].map((e) => e.dataset.edge),
    ).toEqual(["right"]);
  });

  it("an all-day bar's stripe takes no room, so its left end starts at its very edge", () => {
    const { container } = draw(
      [ev({ id: "a", summary: "Away", start: "2026-10-12", end: "2026-10-14", all_day: true })],
      interaction(),
    );
    const bar = container.querySelector<HTMLElement>('[data-event-id="a"]');
    // A press on the end's strip takes the end, and a border would sit outside the strip as the
    // body's: with no border, the strip starts at the bar's very edge.
    expect(bar?.querySelector('[data-edge="left"]')).not.toBeNull();
    expect(bar?.style.borderLeftWidth).toBe("");
    // The calendar's stripe is still drawn there, inside the bar.
    expect(bar?.style.boxShadow).toMatch(/inset 3px 0/);
  });

  it("the ghost's pieces sit at startMin/60 × rowH in their columns", () => {
    const moved = ev({ start: "2026-10-12T23:00:00", end: "2026-10-13T01:30:00" });
    const { container } = draw(
      [ev({})],
      interaction({
        originId: "e1",
        ghost: { event: moved, kind: "move", invalid: null, label: "12-10 23:00 – 13-10 01:30" },
      }),
    );
    const ghosts = container.querySelectorAll<HTMLElement>("[data-drag-ghost]");
    expect(ghosts).toHaveLength(2);
    expect(ghosts[0].closest<HTMLElement>("[data-day]")?.dataset.day).toBe("2026-10-12");
    expect(ghosts[0].style.top).toBe(`${23 * 40}px`);
    expect(ghosts[1].closest<HTMLElement>("[data-day]")?.dataset.day).toBe("2026-10-13");
    expect(ghosts[1].style.top).toBe("0px");
    expect(ghosts[1].style.height).toBe(`${1.5 * 40 - 3}px`);
    expect(ghosts[0].getAttribute("aria-hidden")).toBe("true");
    expect(ghosts[0].className).toContain("pointer-events-none");
    // The event itself dims where it was.
    const origin = container.querySelectorAll<HTMLElement>('[data-event-id="e1"]');
    expect([...origin].every((p) => p.className.includes("opacity-40"))).toBe(true);
  });

  it("a ghost in the hour the clocks skip says why, on the blocked edge", () => {
    const { container } = draw(
      [],
      interaction({
        ghost: {
          event: day("g"),
          kind: "move",
          invalid: "01:30 doesn't happen on 28-03-2027: the clocks go forward.",
          label: "x",
        },
      }),
    );
    const ghost = container.querySelector<HTMLElement>("[data-drag-ghost]");
    expect(ghost?.style.outline).toContain("var(--st-blocked)");
    expect(ghost?.textContent).toContain("the clocks go forward");
  });

  it("an all-day ghost is drawn in the strip's first row", () => {
    const { container } = draw(
      [],
      interaction({
        ghost: {
          event: ev({ start: "2026-10-13", end: "2026-10-14", all_day: true }),
          kind: "move",
          invalid: null,
          label: "All day · 13-10",
        },
      }),
    );
    const ghost = container.querySelector<HTMLElement>("[data-drag-ghost]");
    expect(ghost?.closest('[data-surface="band"]')).not.toBeNull();
    expect(ghost?.style.left).toBe("50%");
    expect(ghost?.style.top).toBe("0px");
  });

  it("pending rows have the pending look, and only one opacity, past or not", () => {
    // Drawn on a later day: both are past, and the pending look replaces the past one.
    const { container } = draw(
      [
        day("p"),
        ev({ id: "q", summary: "Q", start: "2026-10-12", end: "2026-10-13", all_day: true }),
      ],
      interaction({ pendingIds: new Set(["p", "q"]) }),
      new Date(2026, 9, 20),
    );
    for (const id of ["p", "q"]) {
      const el = container.querySelector<HTMLElement>(`[data-event-id="${id}"]`);
      expect(el?.className).toContain("opacity-70");
      expect(el?.className).toContain("outline-dashed");
      expect(el?.className.match(/opacity-/g)).toHaveLength(1);
    }
  });
});

// A sideways swipe moves the window a day at a time, and every step used to snap the hours back to
// the window's start: scroll down to the evening, swipe, and you were at 08:00 again.
describe("TimeGridView — where the hours are scrolled to", () => {
  const BODY_H = 480;
  let restore: (() => void) | null = null;
  beforeAll(() => {
    // jsdom lays nothing out, so give the body a height for the grid to frame its window in. jsdom
    // defines clientHeight on Element, so that is where it is stubbed and put back.
    const d = Object.getOwnPropertyDescriptor(Element.prototype, "clientHeight");
    Object.defineProperty(Element.prototype, "clientHeight", {
      configurable: true,
      get: () => BODY_H,
    });
    restore = () => {
      if (d) Object.defineProperty(Element.prototype, "clientHeight", d);
    };
  });
  afterAll(() => restore?.());

  /** The grid on 12-10 + `first`, framing `startHour` to `endHour`; `custom` when the user set it. */
  const view = (
    first: number,
    range: "work" | "day",
    {
      startHour = 8,
      endHour = startHour + 10,
      custom = false,
    }: { startHour?: number; endHour?: number; custom?: boolean } = {},
  ) => (
    <TimeGridView
      days={[new Date(2026, 9, first), new Date(2026, 9, first + 1)]}
      events={[]}
      colorOf={() => "#336699"}
      range={range}
      bounds={{ startHour, endHour }}
      customBounds={custom ? { startHour, endHour } : undefined}
      zones={[]}
      onZonesChange={() => {}}
      allowZones={false}
      now={new Date(2026, 9, 1)}
    />
  );

  /** The rendered grid's scroll body. */
  const bodyOf = (container: HTMLElement) =>
    container.querySelector<HTMLElement>(".overflow-y-auto")!;

  /** Scroll as the user does: the offset, then the scroll event an engine fires for it. */
  const scrollTo = (body: HTMLElement, top: number) => {
    body.scrollTop = top;
    fireEvent.scroll(body);
  };

  /** Make `body` keep its offset as an engine does: clamped to the grid's height when written, and
   *  again when read after the grid has shrunk. */
  function clampLikeAnEngine(body: HTMLElement) {
    const max = () => parseFloat((body.firstElementChild as HTMLElement).style.height) - BODY_H;
    const clamp = (v: number) => Math.max(0, Math.min(v, max()));
    let top = body.scrollTop;
    Object.defineProperty(body, "scrollTop", {
      configurable: true,
      get: () => clamp(top),
      set: (v: number) => {
        top = clamp(v);
      },
    });
  }

  it("frames the window once, then keeps your place as the days move", () => {
    const { container, rerender } = render(view(12, "work"));
    const body = bodyOf(container);
    // Framed on its start: 8 hours of 48px rows (480px over a 10-hour window).
    expect(body.scrollTop).toBe(8 * 48);
    scrollTo(body, 900); // the user scrolls down to the evening
    rerender(view(13, "work")); // a swipe: one day on
    expect(body.scrollTop).toBe(900);
    rerender(view(5, "work")); // ‹ a week back
    expect(body.scrollTop).toBe(900);
  });

  it("keeps your place when the Day range's sunrise hour moves with the date", () => {
    const { container, rerender } = render(view(12, "day", { startHour: 7 }));
    const body = bodyOf(container);
    scrollTo(body, 900);
    rerender(view(13, "day", { startHour: 8 })); // a later sunrise, the same 10-hour day
    expect(body.scrollTop).toBe(900);
  });

  it("keeps the hour at the top when the rows grow", () => {
    // A shorter day (an earlier sunset) makes taller rows: 18:45 stays at the top.
    const { container, rerender } = render(view(12, "day"));
    const body = bodyOf(container);
    scrollTo(body, 900); // 18.75 hours of 48px rows
    rerender(view(13, "day", { endHour: 16 })); // 480px over 8 hours: 60px rows
    expect(body.scrollTop).toBe(18.75 * 60);
  });

  it("keeps the hour at the top when the rows shrink, though the engine clamps the offset", () => {
    // 60px rows (8 hours in 480px), scrolled to 15:00. A longer day makes 40px rows, a 960px grid
    // that can scroll only to 12:00; by the time the grid knows, the offset already reads 480px.
    // Rescaling that would land on 08:00; the hour the user was at gives 15:00, held at 12:00.
    const { container, rerender } = render(view(12, "day", { endHour: 16 }));
    const body = bodyOf(container);
    clampLikeAnEngine(body);
    scrollTo(body, 15 * 60);
    rerender(view(13, "day", { endHour: 20 }));
    expect(body.scrollTop).toBe(12 * 40);
    // The engine reports the clamp as a scroll; it's the grid's, so 15:00 is still the hour meant,
    // and back on the shorter day it's at the top again.
    fireEvent.scroll(body);
    rerender(view(12, "day", { endHour: 16 }));
    expect(body.scrollTop).toBe(15 * 60);
  });

  it("re-frames when the range, or the hours you set for it, change", () => {
    const { container, rerender } = render(view(12, "work"));
    const body = bodyOf(container);
    scrollTo(body, 900);
    rerender(view(12, "day"));
    expect(body.scrollTop).toBe(8 * 48);
    scrollTo(body, 900);
    rerender(view(12, "day", { startHour: 6, custom: true }));
    expect(body.scrollTop).toBe(6 * 48);
    scrollTo(body, 900);
    rerender(view(12, "day", { startHour: 6, endHour: 14, custom: true })); // only the end
    expect(body.scrollTop).toBe(6 * 60);
  });
});
