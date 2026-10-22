// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Focus tab's "Upcoming" card strikes through an event the user declined, as the Calendar tab
// and Google do (#884, C14b): in the list, as the backend's agenda says (its rows carry no guest
// list); in the days grid, read from the mirror's copies like the Calendar tab, so an occurrence
// mirrored on two calendars is struck through only when every copy with the user's entry declined.

import { act, cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgendaEvent, Attendee, Calendar, CalendarEvent } from "../lib/types";

const ipc = vi.hoisted(() => ({
  listAllCalendarEvents: vi.fn(),
  onCalendarSynced: vi.fn(),
  eventFlags: vi.fn(),
  openUrl: vi.fn(),
}));
vi.mock("../lib/ipc", () => ipc);

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard", accent: "mono", colorblind: false }),
}));

vi.mock("../theme/UserTimeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useUserTime: () => ({ timeZone: "Europe/London", coords: null }),
}));

import { writeFocusUpcomingMode } from "../lib/focusPrefs";
import { FocusUpcoming } from "./FocusUpcoming";

beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

beforeEach(() => {
  localStorage.clear();
  ipc.onCalendarSynced.mockResolvedValue(() => {});
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

const calendar = (id: string, remote: string): Calendar => ({
  id,
  source_id: "gcal:me@x.org",
  provider: "google",
  remote_id: remote,
  name: id,
  color: null,
  selected: true,
  is_primary: false,
  quiet: false,
  kind: null,
  access_role: "owner",
  time_zone: null,
  default_reminders: [],
  conference_types: [],
  data_owner: null,
});
const CALENDARS = [calendar("mine", "me@x.org"), calendar("theirs", "them@x.org")];

const own = (email: string, response: string): Attendee => ({
  name: null,
  email,
  response,
  optional: false,
  organizer: false,
  self: true,
});

/** Today at `hour`, local, as the mirror would hold it. */
function at(hour: number): string {
  const d = new Date();
  d.setHours(hour, 0, 0, 0);
  return d.toISOString().replace(/\.\d{3}Z$/, "Z");
}

const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "mine",
  summary: "Review",
  description: null,
  location: null,
  start: at(23),
  end: null,
  all_day: false,
  html_link: null,
  uid: "u1",
  ...over,
});

describe("FocusUpcoming: declined events", () => {
  it("strikes a row through in the list as the agenda says", () => {
    const listEvents: AgendaEvent[] = [
      { ...row({}), ended: false, declined: true },
      { ...row({ id: "e2", uid: "u2", summary: "Plain" }), ended: false, declined: false },
    ];
    render(<FocusUpcoming listEvents={listEvents} calendars={CALENDARS} />);
    const struck = screen.getByRole("button", { name: /Review, declined/ });
    expect(within(struck).getByText("Review").className).toContain("line-through");
    const plain = screen.getByRole("button", { name: /Plain/ });
    expect(plain.textContent).not.toContain("declined");
    expect(within(plain).getByText("Plain").className).not.toContain("line-through");
  });

  it("strikes a row through in the days grid only when every copy with the user's entry declined", async () => {
    writeFocusUpcomingMode("week");
    ipc.listAllCalendarEvents.mockResolvedValue([
      row({ attendees: [own("me@x.org", "declined")] }),
      row({ id: "e2", calendar_id: "theirs", attendees: [own("them@x.org", "accepted")] }),
      row({ id: "e3", uid: "u3", summary: "Dentist", attendees: [own("me@x.org", "declined")] }),
    ]);
    render(<FocusUpcoming listEvents={[]} calendars={CALENDARS} />);
    await act(async () => {});
    expect(screen.getByRole("button", { name: /^Dentist, .*, declined$/ })).toBeTruthy();
    const merged = screen.getByRole("button", { name: /^Review, / });
    expect(merged.getAttribute("aria-label")).not.toMatch(/declined/);
  });
});
