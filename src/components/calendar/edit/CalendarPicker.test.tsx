// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The calendar a new event goes to (#884, C10): only calendars PM may create on, grouped by account
// as radios, the accounts that can't named by why, a calendar that can't hold a private event greyed
// out for a private draft, a hidden one still offered and marked, and a pointer's pick told from the
// keyboard's. Inside the editor's dialog it's NewEventEditor.test that reaches it by keyboard.

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Calendar, CalendarAccount, CalendarOverview } from "../../../lib/types";
import { createTargets } from "../../../lib/calendarEdit/createTargets";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { CalendarPicker } from "./CalendarPicker";

const ME = "gcal:me@x.com";
const WORK = "gcal:work@x.com";

function account(id: string, provider = "google"): CalendarAccount {
  const email = id.slice(id.indexOf(":") + 1);
  return { id, provider, email, label: email, state: "ok", last_synced_at: null };
}

function calendar(source: string, remote: string, over: Partial<Calendar> = {}): Calendar {
  return {
    id: `${source}:${remote}`,
    source_id: source,
    provider: "google",
    remote_id: remote,
    name: remote,
    color: null,
    selected: true,
    is_primary: false,
    quiet: false,
    kind: null,
    access_role: "owner",
    time_zone: "Europe/London",
    default_reminders: [],
    conference_types: [],
    data_owner: null,
    ...over,
  };
}

const overview: CalendarOverview = {
  google_client_configured: true,
  microsoft_client_configured: false,
  accounts: [
    account(ME),
    account(WORK),
    account("gcal:off@x.com"),
    account("gcal:paused@x.com"),
    account("gcal:consent@x.com"),
    account("ics:feed", "other"),
  ],
  calendars: [
    calendar(ME, "me@x.com", { name: "Personal", is_primary: true }),
    calendar(ME, "family", { name: "Family", access_role: "writerWithoutPrivateAccess" }),
    calendar(ME, "holidays", { name: "Holidays", access_role: "reader" }),
    calendar(ME, "old", { name: "Old project", selected: false }),
    calendar(WORK, "work@x.com", { name: "Work", is_primary: true }),
    calendar("gcal:off@x.com", "off@x.com", { name: "Off account" }),
    calendar("ics:feed", "feed", { name: "Feed", provider: "other", access_role: null }),
  ],
  last_sync: null,
  window_days: 7,
  mirror_start: "2026-09-01T00:00:00Z",
  mirror_end: "2027-11-01T00:00:00Z",
  editing: {
    [ME]: "on",
    [WORK]: "on",
    "gcal:paused@x.com": "paused",
    "gcal:consent@x.com": "needs_consent",
  },
  default_length: {},
};

// jsdom ships no ResizeObserver, and the list's popover (escapeClipping) builds one to re-place
// itself; placement isn't what these assert.
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

function open(over: Partial<Parameters<typeof CalendarPicker>[0]> = {}) {
  const onChange = vi.fn();
  render(
    <CalendarPicker
      targets={createTargets(overview)}
      value={`${ME}:me@x.com`}
      onChange={onChange}
      privateDraft={false}
      hidden={new Set()}
      colorOf={() => "var(--accent)"}
      shapeOf={() => undefined}
      {...over}
    />,
  );
  const trigger = screen.getByRole("button", { name: /^Calendar: / });
  fireEvent.click(trigger);
  const list = screen.getByRole("group", { name: "Calendars that can take a new event" });
  return { onChange, trigger, list };
}

describe("CalendarPicker", () => {
  it("groups radios by account under a named group", () => {
    const { trigger, list } = open();
    expect(trigger.getAttribute("aria-expanded")).toBe("true");
    const mine = within(list).getByRole("group", { name: "me@x.com" });
    const work = within(list).getByRole("group", { name: "work@x.com" });
    expect(
      within(mine)
        .getAllByRole("radio")
        .map((r) => r.closest("label")?.textContent),
    ).toEqual(["Personal", "Family"]);
    expect(within(work).getByRole("radio", { name: "Work" })).toBeTruthy();
    // One choice across the groups: they share a name.
    const names = new Set(
      within(list)
        .getAllByRole("radio")
        .map((r) => r.getAttribute("name")),
    );
    expect(names.size).toBe(1);
    expect(
      (within(mine).getByRole("radio", { name: "Personal" }) as HTMLInputElement).checked,
    ).toBe(true);
  });

  it("lists only writable, selected calendars", () => {
    const { list } = open();
    const offered = within(list)
      .getAllByRole("radio")
      .map((r) => r.closest("label")?.textContent);
    expect(offered).toEqual(["Personal", "Family", "Work"]);
    // Read-only, unsynced, editing-off and subscription calendars aren't there at all.
    for (const name of ["Holidays", "Old project", "Off account", "Feed"]) {
      expect(within(list).queryByText(name)).toBeNull();
    }
  });

  it("names editing-off accounts in the footer by status", () => {
    const { list } = open();
    expect(
      within(list).getByText("Editing is off for off@x.com. Turn it on in Settings › Connectors."),
    ).toBeTruthy();
    expect(
      within(list).getByText(
        "Editing is paused for paused@x.com. Turn it back on in Settings › Connectors.",
      ),
    ).toBeTruthy();
    expect(
      within(list).getByText(
        "Editing for consent@x.com needs Google's permission again. Ask for it in Settings › Connectors.",
      ),
    ).toBeTruthy();
  });

  it("disables a no-private calendar for a private draft", () => {
    const { list } = open({ privateDraft: true });
    const family = within(list).getByRole("radio", { name: /Family/ }) as HTMLInputElement;
    expect(family.disabled).toBe(true);
    expect(family.closest("label")?.textContent).toContain("Can't hold private events");
    expect(family.closest("label")?.className).not.toContain("text-faint");
    expect(
      (within(list).getByRole("radio", { name: "Personal" }) as HTMLInputElement).disabled,
    ).toBe(false);
    cleanup();
    const again = open({ privateDraft: false });
    expect(
      (within(again.list).getByRole("radio", { name: "Family" }) as HTMLInputElement).disabled,
    ).toBe(false);
  });

  it("a hidden calendar is listed and says so", () => {
    const { list } = open({ hidden: new Set([`${WORK}:work@x.com`]) });
    const work = within(list).getByRole("radio", { name: /Work/ });
    expect(work.closest("label")?.textContent).toBe("Workhidden");
  });

  it("a click picks and closes, focus back on the button; the arrow keys only move the choice", () => {
    const { onChange, trigger, list } = open();
    const work = within(list).getByRole("radio", { name: "Work" });
    // What the arrow keys do: a change and a click with no pointer behind it.
    fireEvent.click(work, { detail: 0 });
    expect(onChange).toHaveBeenLastCalledWith(`${WORK}:work@x.com`);
    expect(
      screen.queryByRole("group", { name: "Calendars that can take a new event" }),
    ).not.toBeNull();
    // Enter takes it.
    fireEvent.keyDown(work, { key: "Enter" });
    expect(screen.queryByRole("group", { name: "Calendars that can take a new event" })).toBeNull();
    expect(document.activeElement).toBe(trigger);
    // A pointer's click picks and closes at once.
    fireEvent.click(trigger);
    const family = within(
      screen.getByRole("group", { name: "Calendars that can take a new event" }),
    ).getByRole("radio", { name: "Family" });
    fireEvent.click(family, { detail: 1 });
    expect(onChange).toHaveBeenLastCalledWith(`${ME}:family`);
    expect(screen.queryByRole("group", { name: "Calendars that can take a new event" })).toBeNull();
  });

  // A click on the name or dot reaches the radio forwarded by its label, with `detail` 0 in every
  // engine PM runs on, as an arrow key's click has: the pointer going down on the row is what tells.
  it("a click on a row's name picks and closes, as one on its radio does", () => {
    const { onChange, trigger, list } = open();
    fireEvent.pointerDown(within(list).getByText("Family"));
    fireEvent.click(within(list).getByRole("radio", { name: "Family" }), { detail: 0 });
    expect(onChange).toHaveBeenLastCalledWith(`${ME}:family`);
    expect(screen.queryByRole("group", { name: "Calendars that can take a new event" })).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  it("a key after a pointer went down on a row is the keyboard's again", () => {
    const { onChange, list } = open();
    // Pressed on a row and let go off it: no click. Then an arrow key moves the choice.
    fireEvent.pointerDown(within(list).getByText("Family"));
    const work = within(list).getByRole("radio", { name: "Work" });
    fireEvent.keyDown(work, { key: "ArrowDown" });
    fireEvent.click(work, { detail: 0 });
    expect(onChange).toHaveBeenLastCalledWith(`${WORK}:work@x.com`);
    expect(
      screen.queryByRole("group", { name: "Calendars that can take a new event" }),
    ).not.toBeNull();
  });

  it("opening puts focus on the calendar picked, else the first that can be", async () => {
    const { list } = open({ value: `${WORK}:work@x.com` });
    await waitFor(() =>
      expect(document.activeElement).toBe(within(list).getByRole("radio", { name: "Work" })),
    );
    cleanup();
    // Picked, but a private draft can't go there: the first calendar that can take it.
    const again = open({ value: `${ME}:family`, privateDraft: true });
    await waitFor(() =>
      expect(document.activeElement).toBe(
        within(again.list).getByRole("radio", { name: "Personal" }),
      ),
    );
  });

  it("closes when it's disabled, and doesn't come back by itself", () => {
    const props = {
      targets: createTargets(overview),
      value: `${ME}:me@x.com`,
      onChange: vi.fn(),
      privateDraft: false,
      hidden: new Set<string>(),
      colorOf: () => "var(--accent)",
      shapeOf: () => undefined,
    };
    const view = render(<CalendarPicker {...props} />);
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    expect(
      screen.queryByRole("group", { name: "Calendars that can take a new event" }),
    ).not.toBeNull();
    view.rerender(<CalendarPicker {...props} disabled />);
    view.rerender(<CalendarPicker {...props} />);
    expect(screen.queryByRole("group", { name: "Calendars that can take a new event" })).toBeNull();
  });

  it("can't open while disabled", () => {
    render(
      <CalendarPicker
        targets={createTargets(overview)}
        value={`${ME}:me@x.com`}
        onChange={vi.fn()}
        privateDraft={false}
        hidden={new Set()}
        colorOf={() => "var(--accent)"}
        shapeOf={() => undefined}
        disabled
      />,
    );
    const trigger = screen.getByRole("button", { name: "Calendar: Personal" }) as HTMLButtonElement;
    expect(trigger.disabled).toBe(true);
  });
});
