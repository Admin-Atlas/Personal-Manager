// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom

// Opening a copy of an event (#884, C18c): the original is read from Google once, never the mirror,
// and its copy handed on; a read that fails says why and closes; nothing is ever sent.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { CalendarEvent, EditLoad, EventForEdit } from "../../../lib/types";

const openForEdit = vi.hoisted(() => vi.fn<(id: string) => Promise<EditLoad>>());
vi.mock("./useEventWrites", () => ({ openForEdit: (id: string) => openForEdit(id) }));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { COPY_CALL_NOTE, seedDuplicate } from "../../../lib/calendarEdit/eventDraft";
import { CopyOpening } from "./CopyOpening";

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

const CAL = "gcal:me@x.org:me@x.org";

const row = (over: Partial<CalendarEvent> = {}): CalendarEvent => ({
  id: `${CAL}:e1`,
  calendar_id: CAL,
  summary: "Retro",
  description: null,
  location: null,
  start: "2026-10-20T09:00:00Z",
  end: "2026-10-20T10:00:00Z",
  all_day: false,
  html_link: null,
  uid: "e1@google.com",
  ...over,
});

const event: EventForEdit = {
  summary: "Retro",
  location: "Room 2",
  description: "Notes",
  description_html: false,
  time: {
    kind: "timed",
    start_date: "2026-10-20",
    start_time: "10:00",
    start_zone: "Europe/London",
    end_date: "2026-10-20",
    end_time: "11:00",
    end_zone: "Europe/London",
  },
  start_at: "2026-10-20T09:00:00Z",
  end_at: "2026-10-20T10:00:00Z",
  show_as: "busy",
  visibility: "default",
  attachments: [],
  html_link: null,
  guests: [],
  guests_can_modify: false,
  guests_can_invite_others: true,
  guests_can_see_other_guests: true,
  guests_omitted: false,
  organizer_email: "me@x.org",
  reminders: { use_default: true, overrides: [] },
  color_id: null,
  label_id: null,
  reply: null,
  organizer_self: true,
  call: { kind: "meet", name: "Google Meet", join: "https://meet.google.com/x", state: "ready" },
  meet_offered: true,
};

const ready = (ev: EventForEdit): EditLoad => ({
  outcome: "ready",
  session: "s1",
  event: ev,
  permissions: {
    summary: true,
    time: true,
    location: true,
    description: true,
    show_as: true,
    visibility: true,
    guests: true,
    reminders: true,
    colour: true,
    repeat: true,
    delete: true,
    respond: false,
    meet: true,
    calendar: true,
    reasons: [],
  },
  seen: null,
  series: null,
});

describe("CopyOpening", () => {
  it("reads the original once and hands its copy on", async () => {
    openForEdit.mockResolvedValue(ready(event));
    const onReady = vi.fn();
    const { rerender } = render(<CopyOpening row={row()} onReady={onReady} onClose={() => {}} />);
    expect(screen.getByRole("dialog", { name: "Duplicate event" })).toBeTruthy();
    expect(screen.getByText("Copying “Retro” from Google…")).toBeTruthy();
    await act(async () => {});
    expect(openForEdit.mock.calls).toEqual([[`${CAL}:e1`]]);
    expect(onReady).toHaveBeenCalledOnce();
    expect(onReady).toHaveBeenCalledWith(seedDuplicate(event, null, false));
    expect(onReady.mock.calls[0][0].callNote).toBe(COPY_CALL_NOTE);
    // A new render from the calendar reads nothing more.
    rerender(<CopyOpening row={row()} onReady={vi.fn()} onClose={() => {}} />);
    await act(async () => {});
    expect(openForEdit).toHaveBeenCalledOnce();
  });

  it("an occurrence's copy says the original repeats", async () => {
    openForEdit.mockResolvedValue(ready(event));
    const onReady = vi.fn();
    render(<CopyOpening row={row({ recurring: true })} onReady={onReady} onClose={() => {}} />);
    await act(async () => {});
    expect(onReady.mock.calls[0][0].notes).toEqual([
      "The original repeats; the copy doesn't. Choose Repeat to set it.",
    ]);
  });

  it("a gone original says so and closes", async () => {
    openForEdit.mockResolvedValue({ outcome: "gone" });
    const onReady = vi.fn();
    const onClose = vi.fn();
    render(<CopyOpening row={row()} onReady={onReady} onClose={onClose} />);
    await act(async () => {});
    expect(onReady).not.toHaveBeenCalled();
    expect(screen.getByText(/deleted/i)).toBeTruthy();
    // The footer's Close, beside the bar's.
    const closes = screen.getAllByRole("button", { name: "Close" });
    expect(closes).toHaveLength(2);
    fireEvent.click(closes[1]);
    expect(onClose).toHaveBeenCalled();
  });

  it("a read that fails says why", async () => {
    openForEdit.mockRejectedValue(new Error("PM couldn't reach Google."));
    render(<CopyOpening row={row()} onReady={vi.fn()} onClose={() => {}} />);
    await act(async () => {});
    expect(screen.getByText("PM couldn't reach Google.")).toBeTruthy();
  });

  it("Escape closes, and an answer after it hands nothing on", async () => {
    let answer: (load: EditLoad) => void = () => {};
    openForEdit.mockReturnValue(new Promise((resolve) => (answer = resolve)));
    const onReady = vi.fn();
    const onClose = vi.fn();
    const { unmount } = render(<CopyOpening row={row()} onReady={onReady} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
    unmount();
    await act(async () => answer(ready(event)));
    expect(onReady).not.toHaveBeenCalled();
  });
});
