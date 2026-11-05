// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The palette's dialog semantics, which it had been hand-rolling and getting wrong in one specific
// way: Escape was a React `onKeyDown` on the `<input>`, and the result rows are `tabIndex={-1}`, so
// the FIRST Tab moved focus out of the palette entirely — and from there a keydown on a background
// control never bubbled back to the input. Escape was dead, with the palette still on screen over a
// scrim, and the only way out was the mouse.
//
// Wearing `Modal` fixes that (its Escape is a window listener) and brings the rest of the shell with
// it: a `role="dialog"` with a name, and focus handed back to whatever opened the palette. Those are
// what these tests pin — not the fuzzy matcher, which is pure and belongs to its own unit.

import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listConversations = vi.fn();
const listDocuments = vi.fn();
const listProjectOverviews = vi.fn();
const searchCalendarEvents = vi.fn();
// The Calendar tab's hidden calendars live in the encrypted store, which no test hydrates.
const hiddenCalendars = vi.hoisted(() => ({ ids: new Set<string>() }));
vi.mock("../lib/calendarPrefs", () => ({ readHidden: () => hiddenCalendars.ids }));

vi.mock("../lib/ipc", () => ({
  listConversations: () => listConversations(),
  listDocuments: () => listDocuments(),
  listProjectOverviews: () => listProjectOverviews(),
  searchCalendarEvents: (query: string, hidden: string[]) => searchCalendarEvents(query, hidden),
}));

// The same stub the other component tests use: the palette's primitives reach for `useTheme`, and
// the real ThemeProvider pulls in IPC.
vi.mock("../theme/ThemeContext", async (importOriginal) => ({
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

import { CommandPalette } from "./CommandPalette";
import type { CalendarEvent } from "../lib/types";

const noop = () => {};

function renderPalette(onClose: () => void, onOpenEvent: (event: CalendarEvent) => void = noop) {
  return render(
    <CommandPalette
      onClose={onClose}
      onOpenProject={noop}
      onOpenConversation={noop}
      onOpenEvent={onOpenEvent}
      onNavigate={noop}
      onOpenSettings={noop}
    />,
  );
}

/** Something outside the palette to hold focus — the position a single Tab used to land on. */
function outsideButton(label: string): HTMLButtonElement {
  const el = document.createElement("button");
  el.textContent = label;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  listConversations.mockResolvedValue([]);
  listDocuments.mockResolvedValue([]);
  listProjectOverviews.mockResolvedValue([]);
  searchCalendarEvents.mockResolvedValue([]);
  // jsdom implements no layout, so it has no `scrollIntoView` at all; the palette keeps the
  // highlighted row in view on every arrow press. Same stub as ChatView.test.tsx.
  Element.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.clearAllMocks();
  hiddenCalendars.ids = new Set();
});

function event(id: string, summary: string, calendarId: string): CalendarEvent {
  return {
    id,
    calendar_id: calendarId,
    summary,
    start: "2026-11-09T09:00:00Z",
    all_day: false,
    recurring: true,
  } as CalendarEvent;
}

describe("CommandPalette", () => {
  it("closes on Escape from a focus position that is NOT the input", async () => {
    // The regression this conversion exists for. Focus is deliberately moved off the input first:
    // with the old input-bound handler this Escape reached nothing at all.
    const onClose = vi.fn();
    renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const elsewhere = outsideButton("behind the palette");
    elsewhere.focus();
    expect(document.activeElement).toBe(elsewhere);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("still closes on Escape while the input holds focus", async () => {
    // The path that already worked must keep working — the fix removed the input's own branch.
    const onClose = vi.fn();
    const { getByRole } = renderPalette(onClose);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    expect(document.activeElement).toBe(input);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps Tab on the query field instead of letting it leave", async () => {
    // The rows are `tabIndex={-1}` and selected via `aria-activedescendant`, so the query field is
    // the palette's ONLY tab stop — Tab should return to it, not walk out into the app behind the
    // scrim. (`useFocusTrap` had to learn that a tabindex="-1" button is not a stop; see its test.)
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    const input = getByRole("combobox");
    fireEvent.keyDown(input, { key: "Tab" });
    expect(document.activeElement).toBe(input);
  });

  it("is a named dialog, and the input keeps its own field name", async () => {
    // Two different names, both required: `aria-label` on a combobox names the FIELD, so it could
    // never have been the dialog's name. The palette had no dialog role at all before this.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    expect(
      getByRole("combobox", { name: "Search projects, files, conversations and events" }),
    ).toBeTruthy();
  });

  it("hands focus back to whatever opened it", async () => {
    // Closing used to drop focus on <body>: keyboard users landed at the top of the document.
    const opener = outsideButton("Open the palette");
    opener.focus();

    const { unmount } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    expect(document.activeElement).not.toBe(opener);

    unmount();
    expect(document.activeElement).toBe(opener);
  });

  it("keeps the help anchor on the panel itself", async () => {
    // Two things depend on where this sits. HelpOverlay resolves a hovered element with
    // `closest("[data-help]")`, so it must be an ANCESTOR of the rows and the input — and
    // `.help-mode [data-help]:hover` draws the outline that says help is available here, which
    // needs an element that actually paints a box. The dialog element is both.
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    expect(getByRole("dialog").getAttribute("data-help")).toBe("command-palette");
    expect(getByRole("combobox").closest("[data-help]")).toBe(getByRole("dialog"));
    expect(getByRole("listbox").closest("[data-help]")).toBe(getByRole("dialog"));
  });

  it("finds calendar events, leaving hidden calendars to Rust, and opens one", async () => {
    // The Search button found nothing in the calendar (#1047): events are searched in Rust as you
    // type, the Calendar tab's hidden calendars left out there (before the limit), and a hit opens
    // the Calendar on it.
    hiddenCalendars.ids = new Set(["cal-hidden"]);
    const standup = event("standup-09", "Team standup", "cal-work");
    searchCalendarEvents.mockResolvedValue([standup]);
    const onClose = vi.fn();
    const onOpenEvent = vi.fn();
    const { getByRole, findByRole } = renderPalette(onClose, onOpenEvent);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());

    fireEvent.change(getByRole("combobox"), { target: { value: "  Team " } });
    const hit = await findByRole("option", { name: /Team standup/ });
    expect(searchCalendarEvents).toHaveBeenCalledTimes(1);
    expect(searchCalendarEvents).toHaveBeenCalledWith("team", ["cal-hidden"]);
    expect(getByRole("group", { name: "Events" })).toBeTruthy();
    expect(hit.textContent).toContain("repeats");

    fireEvent.click(hit);
    expect(onOpenEvent).toHaveBeenCalledWith(standup);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("never shows an older search's answer under a newer query", async () => {
    const answers = new Map<string, (events: CalendarEvent[]) => void>();
    searchCalendarEvents.mockImplementation(
      (query: string) => new Promise<CalendarEvent[]>((resolve) => answers.set(query, resolve)),
    );
    const { getByRole, findByRole, queryByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    const box = getByRole("combobox");

    fireEvent.change(box, { target: { value: "te" } });
    await waitFor(() => expect(answers.has("te")).toBe(true));
    fireEvent.change(box, { target: { value: "team" } });
    await waitFor(() => expect(answers.has("team")).toBe(true));
    answers.get("team")?.([event("standup", "Team standup", "cal-work")]);
    await findByRole("option", { name: /Team standup/ });
    answers.get("te")?.([event("tea", "Tea with Mats", "cal-work")]);
    await new Promise((r) => setTimeout(r, 50));
    expect(queryByRole("option", { name: /Tea with Mats/ })).toBeNull();
  });

  it("keeps the arrowed row highlighted when the calendar's hits land, and keeps hits meanwhile", async () => {
    // An index used to snap back to the top when the hits landed, under a row already picked; and
    // the Events group blinked out on every keystroke until the next answer.
    listDocuments.mockResolvedValue([
      { id: 1, title: "Team notes", project: "Atlas", linked_projects: [], tags: [] },
      { id: 2, title: "Team plan", project: "Atlas", linked_projects: [], tags: [] },
    ]);
    const answers = new Map<string, (events: CalendarEvent[]) => void>();
    searchCalendarEvents.mockImplementation(
      (query: string) => new Promise<CalendarEvent[]>((resolve) => answers.set(query, resolve)),
    );
    const { getByRole, findByRole } = renderPalette(noop);
    await findByRole("option", { name: /Team notes/ });
    const box = getByRole("combobox");

    fireEvent.change(box, { target: { value: "team" } });
    fireEvent.keyDown(box, { key: "ArrowDown" });
    const plan = getByRole("option", { name: /Team plan/ });
    expect(plan.getAttribute("aria-selected")).toBe("true");
    await waitFor(() => expect(answers.has("team")).toBe(true));
    answers.get("team")?.([event("standup", "Team standup", "cal-work")]);
    await findByRole("option", { name: /Team standup/ });
    expect(getByRole("option", { name: /Team plan/ }).getAttribute("aria-selected")).toBe("true");

    // A longer query: the hit that still holds every word stays while its own search is on the way.
    fireEvent.change(box, { target: { value: "team stand" } });
    expect(getByRole("option", { name: /Team standup/ })).toBeTruthy();
  });

  it("doesn't search the calendar for a single letter", async () => {
    const { getByRole } = renderPalette(noop);
    await waitFor(() => expect(listDocuments).toHaveBeenCalled());
    fireEvent.change(getByRole("combobox"), { target: { value: "t" } });
    await new Promise((r) => setTimeout(r, 300));
    expect(searchCalendarEvents).not.toHaveBeenCalled();
  });
});
