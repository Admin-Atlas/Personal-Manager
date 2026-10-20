// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Agenda draws the event being made (#884, C10c) as its ghost: dashed, in its calendar's colour,
// never greyed as past, named as the new event, and marked so a click on it isn't a click outside
// quick create; a synced row beside it is drawn and named as ever.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent } from "../../../lib/types";
import { DRAFT_EVENT_ID } from "../../../lib/calendar-layout";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { AgendaView } from "./AgendaView";

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

function agenda(events: CalendarEvent[]) {
  render(
    <AgendaView
      events={events}
      fromDay={new Date(2026, 9, 12)}
      colorOf={(id) => (id === "work" ? "#336699" : "#993366")}
      // Both rows are over by now: a synced one greys, the ghost doesn't.
      now={new Date(2026, 9, 20)}
      onEventClick={vi.fn()}
    />,
  );
}

describe("AgendaView", () => {
  it("draws the new event's ghost dashed and named as new, beside a synced row as ever", () => {
    agenda([
      ev({}),
      ev({
        id: DRAFT_EVENT_ID,
        calendar_id: "work",
        summary: "Dentist",
        start: "2026-10-12T11:00:00",
      }),
    ]);
    const ghost = screen.getByRole("button", { name: "New event, not saved yet: 11:00, Dentist" });
    expect(ghost.className).toContain("outline-dashed");
    expect(ghost.className).toContain("-outline-offset-1");
    expect(ghost.className).not.toContain("opacity-60");
    expect(ghost.hasAttribute("data-draft-event")).toBe(true);
    expect(ghost.style.borderLeftColor).toBe("rgb(51, 102, 153)");
    const synced = screen.getByRole("button", { name: /^09:00\s*Synced$/ });
    expect(synced.className).not.toContain("outline-dashed");
    expect(synced.className).toContain("opacity-60");
    expect(synced.hasAttribute("data-draft-event")).toBe(false);
    expect(synced.style.borderLeftColor).toBe("rgb(153, 51, 102)");
  });

  it("names a ghost a save may have made without saying it isn't saved", () => {
    agenda([
      {
        ...ev({ id: DRAFT_EVENT_ID, summary: "Dentist" }),
        draft_state: "unsure",
      } as CalendarEvent,
    ]);
    expect(
      screen.getByRole("button", {
        name: "New event, PM may already have made it: 09:00, Dentist",
      }),
    ).toBeTruthy();
  });
});
