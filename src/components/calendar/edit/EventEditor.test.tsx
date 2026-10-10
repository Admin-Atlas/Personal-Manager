// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The event editor's promises (#884): only what changed is sent, a typed date counts even if Save is
// pressed before the field loses focus, a conflict never loses the draft silently, a dirty draft asks
// before it goes, and what can't change is greyed out with a reason (no device zone included).

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CalendarEvent,
  EditLoad,
  EventForEdit,
  FieldPermissions,
  WriteOutcome,
} from "../../../lib/types";

const openForEdit = vi.fn<(id: string) => Promise<EditLoad>>();
const saveEdit = vi.fn<(session: string, changes: unknown) => Promise<WriteOutcome>>();
const noteSaved = vi.fn();
let deviceZone: string | null = "Europe/London";

vi.mock("./useEventWrites", () => ({
  openForEdit: (id: string) => openForEdit(id),
  saveEdit: (s: string, c: unknown) => saveEdit(s, c),
  noteSaved: (...a: unknown[]) => noteSaved(...a),
}));

vi.mock("../../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => deviceZone,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

vi.mock("../../../lib/markdown", () => ({
  Markdown: ({ children }: { children: string }) => <div>{children}</div>,
}));

import { EventEditor } from "./EventEditor";

const row: CalendarEvent = {
  id: "gcal:me@x.com:me@x.com:abc",
  calendar_id: "gcal:me@x.com:me@x.com",
  summary: "Dentist",
  description: null,
  location: "High St",
  start: "2026-10-12T08:00:00Z",
  end: "2026-10-12T09:00:00Z",
  all_day: false,
  html_link: null,
  uid: "abc@google.com",
};

const ALL: FieldPermissions = {
  summary: true,
  time: true,
  location: true,
  description: true,
  show_as: true,
  visibility: true,
  delete: true,
  reasons: [],
};

function event(over: Partial<EventForEdit> = {}): EventForEdit {
  return {
    summary: "Dentist",
    location: "High St",
    description: "",
    description_html: false,
    time: {
      kind: "timed",
      start_date: "2026-10-12",
      start_time: "09:00",
      start_zone: "Europe/London",
      end_date: "2026-10-12",
      end_time: "10:00",
      end_zone: "Europe/London",
    },
    start_at: "2026-10-12T08:00:00Z",
    end_at: "2026-10-12T09:00:00Z",
    show_as: "busy",
    visibility: "default",
    attachments: [],
    html_link: null,
    ...over,
  };
}

const ready = (ev: EventForEdit, permissions = ALL, session = "s1"): EditLoad => ({
  outcome: "ready",
  session,
  event: ev,
  permissions,
  seen: {
    summary: ev.summary,
    start: "2026-10-12T08:00:00Z",
    end: "2026-10-12T09:00:00Z",
    all_day: false,
    location: ev.location,
  },
});

async function open(onClose = vi.fn(), onDelete?: (r: CalendarEvent) => void) {
  render(
    <EventEditor
      row={row}
      calendar={null}
      account="me@x.com"
      milestone={null}
      onClose={onClose}
      onDelete={onDelete}
    />,
  );
  await screen.findByLabelText("Title");
  return onClose;
}

beforeEach(() => {
  deviceZone = "Europe/London";
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("EventEditor", () => {
  // Google's formatted description shows read-only, as its text: lines where Google breaks them,
  // escapes decoded once, and nothing of the markup (a script's text included) on screen.
  it("shows a formatted description as its text", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          description:
            "<b>Agenda</b><br><ul><li>One &amp; two</li></ul><script>alert(1)</script>&lt;i&gt;",
          description_html: true,
        }),
      ),
    );
    await open();
    const text = screen.getByText(
      (_, el) => el?.tagName === "P" && /Agenda/.test(el.textContent ?? ""),
    );
    expect(text.textContent).toBe("Agenda\nOne & two\n<i>");
    expect(text.querySelector("b, i, script")).toBeNull();
  });

  it("sends only what changed, then closes and says so", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", { summary: "Dentist (moved)" });
    expect(noteSaved).toHaveBeenCalledWith("Dentist (moved)", {
      outcome: "saved",
      warnings: [],
    });
  });

  it("sends a date typed but not yet committed when Save is pressed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "13-10-2026" } });
    // Still focused: the field hasn't committed it.
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    // The end moved with it, keeping the hour.
    expect(saveEdit).toHaveBeenCalledWith("s1", {
      time: {
        kind: "timed",
        start_date: "2026-10-13",
        start_time: "09:00",
        start_zone: "Europe/London",
        end_date: "2026-10-13",
        end_time: "10:00",
        end_zone: "Europe/London",
      },
    });
  });

  it("closes without sending anything when nothing changed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("keeps the draft through a conflict, and names what Google's version replaced", async () => {
    openForEdit
      .mockResolvedValueOnce(ready(event()))
      .mockResolvedValueOnce(ready(event({ summary: "Dentist (Google)" }), ALL, "s2"));
    saveEdit.mockResolvedValue({ outcome: "conflict", fields: ["summary"] });
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (PM)" } });
    fireEvent.change(screen.getByLabelText("Location"), { target: { value: "Room 2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByText(/Google's version of the title is shown now/);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByLabelText<HTMLInputElement>("Title").value).toBe("Dentist (Google)");
    expect(screen.getByLabelText<HTMLInputElement>("Location").value).toBe("Room 2");
    // The next save goes out on the new session, with only the user's own change.
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenLastCalledWith("s2", { location: "Room 2" });
  });

  it("asks before throwing a dirty draft away", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.keyDown(window, { key: "Escape" });
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("greys out what a locked event can't change, and says why", async () => {
    openForEdit.mockResolvedValue(
      ready(event(), {
        ...ALL,
        summary: false,
        time: false,
        location: false,
        description: false,
        delete: false,
        reasons: ["locked"],
      }),
    );
    await open(vi.fn(), vi.fn());
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByText(/Google has locked this event's title/)).toBeTruthy();
    // Delete is wired (onDelete given), and still not offered.
    expect(screen.queryByRole("button", { name: "Delete" })).toBeNull();
  });

  it("deletes what the editor showed, asking first when there are unsaved changes", async () => {
    openForEdit.mockResolvedValue(ready(event({ summary: "Dentist (Google)" })));
    const onDelete = vi.fn();
    await open(vi.fn(), onDelete);
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "Dentist (moved)" } });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Discard your changes and delete?")).toBeTruthy();
    expect(onDelete).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard and delete" }));
    // The row handed on carries Google's fresh copy (what the editor showed), not the mirror's.
    expect(onDelete).toHaveBeenCalledWith(
      expect.objectContaining({ id: row.id, summary: "Dentist (Google)" }),
    );
  });

  it("refuses to save a date it couldn't read, rather than dropping it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const start = screen.getByLabelText<HTMLInputElement>("Start date");
    start.focus();
    fireEvent.change(start, { target: { value: "31-13-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText(/“31-13-2026” isn't a date PM can read/)).toBeTruthy();
    expect(saveEdit).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("asks before closing over a date typed but not yet committed", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const end = screen.getByLabelText<HTMLInputElement>("End date");
    end.focus();
    fireEvent.change(end, { target: { value: "13-10-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(await screen.findByText("Discard your changes?")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("moves a whole all-day event when its first day moves", async () => {
    openForEdit.mockResolvedValue(
      ready(
        event({
          time: { kind: "all_day", first_day: "2026-10-15", last_day: "2026-10-16" },
          start_at: null,
          end_at: null,
        }),
      ),
    );
    saveEdit.mockResolvedValue({ outcome: "saved", warnings: [] });
    const onClose = await open();
    const first = screen.getByLabelText<HTMLInputElement>("First day");
    first.focus();
    fireEvent.change(first, { target: { value: "12-10-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).toHaveBeenCalledWith("s1", {
      time: { kind: "all_day", first_day: "2026-10-12", last_day: "2026-10-13" },
    });
  });

  it("turning All day on and off again leaves the time as Google holds it", async () => {
    openForEdit.mockResolvedValue(ready(event()));
    const onClose = await open();
    const allDay = screen.getByRole("switch", { name: "All day" });
    fireEvent.click(allDay);
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(saveEdit).not.toHaveBeenCalled();
  });

  it("turns the time controls off when this computer's zone is unknown", async () => {
    deviceZone = null;
    openForEdit.mockResolvedValue(ready(event()));
    await open();
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByText(/couldn't tell which time zone this computer is in/)).toBeTruthy();
    // Everything else still works.
    expect(screen.getByLabelText<HTMLInputElement>("Title").disabled).toBe(false);
  });

  it("says why it couldn't open", async () => {
    openForEdit.mockResolvedValue({ outcome: "reauth" });
    render(
      <EventEditor row={row} calendar={null} account={null} milestone={null} onClose={() => {}} />,
    );
    expect(await screen.findByText(/Google needs you to sign in again/)).toBeTruthy();
  });
});
