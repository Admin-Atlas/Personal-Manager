// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Day/Week grid draws a timed event that runs past midnight in the hours of every day it
// touches (#884), not in the all-day strip: one piece per day, each opening the same event, and only
// the first a tab stop. All-day events still go to the strip.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { TimeGridView } from "./TimeGridView";

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
});
