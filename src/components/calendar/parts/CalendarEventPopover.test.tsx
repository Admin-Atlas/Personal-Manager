// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The popover's focus contract, and the fact that it is deliberately NOT a modal.
//
// It declared `role="dialog"` and then never moved focus into itself. Event chips are activatable
// from the keyboard, and this panel renders LAST in the calendar's DOM, so opening it with Enter
// left focus on the chip and reaching the panel's own Close / "Join the call" buttons meant tabbing
// forward through every remaining event in the grid. Closing then dropped focus on <body>.
//
// The other half is a guard against a future "fix": adding `aria-modal` or a focus trap here would
// be wrong. There is no scrim, the calendar behind stays live, and clicking a different event is how
// you move between them — Tab must be able to leave.

import { act, cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Calendar, CalendarEvent, WriteOutcome } from "../../../lib/types";

const eventFlags = vi.fn();
const openUrl = vi.fn();

vi.mock("../../../lib/ipc", () => ({
  eventFlags: (...a: unknown[]) => eventFlags(...a),
  openUrl: (...a: unknown[]) => openUrl(...a),
}));

// The description is untrusted provider text and renders through the real sanitising boundary in the
// app; here the Markdown view is stubbed (its own suites drive it), so a test can see which view drew
// a description and hand the pop-up the links Markdown would make. The rest of the module is real:
// the codec's link rule asks its `safeUrl`.
const plainMarkdown = ({ children }: { children: string }) => <div>{children}</div>;
const markdownView = vi.fn(plainMarkdown);
vi.mock("../../../lib/markdown", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  Markdown: (props: { children: string }) => markdownView(props),
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
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

import {
  CalendarEventPopover,
  type PopoverEditing,
  type PopoverRsvp,
} from "./CalendarEventPopover";

function calendarEvent(over: Partial<CalendarEvent> = {}): CalendarEvent {
  return {
    id: "e1",
    calendar_id: "c1",
    summary: "Design review",
    description: null,
    location: null,
    start: "2026-08-03T10:00:00",
    end: "2026-08-03T11:00:00",
    all_day: false,
    html_link: null,
    uid: null,
    ...over,
  };
}

/** jsdom has no layout, so a chip's rect is supplied directly — which is exactly how the real
 *  calendar drives this panel (a `DOMRect` handed up by whichever chip was activated). */
function rect(top: number): DOMRect {
  return {
    x: 10,
    y: top,
    left: 10,
    top,
    right: 200,
    bottom: top + 24,
    width: 190,
    height: 24,
    toJSON: () => ({}),
  } as DOMRect;
}

/** An event chip: focusable, and the thing focus has to come back to. */
function chip(label: string): HTMLElement {
  const el = document.createElement("div");
  el.setAttribute("role", "button");
  el.tabIndex = 0;
  el.textContent = label;
  document.body.appendChild(el);
  return el;
}

beforeEach(() => {
  eventFlags.mockResolvedValue([]);
  openUrl.mockResolvedValue(undefined);
  markdownView.mockImplementation(plainMarkdown);
});

/** One of `provider`'s calendars, for the description's view. */
function calendarOf(provider: string): Calendar {
  return {
    id: "c1",
    source_id: `${provider}:me@example.com`,
    provider,
    remote_id: "me@example.com",
    name: "Me",
    color: null,
    selected: true,
    is_primary: true,
    quiet: false,
  } as Calendar;
}

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

describe("CalendarEventPopover", () => {
  it("moves focus into the panel when it opens", () => {
    const opener = chip("Design review");
    opener.focus();

    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    const panel = getByRole("dialog");
    expect(panel.contains(document.activeElement)).toBe(true);
  });

  it("hands focus back to the chip on Escape", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();

    render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={onClose}
      />,
    );

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
  });

  it("hands focus back to the chip on Close", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();

    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={onClose}
      />,
    );

    fireEvent.click(getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
  });

  // CalendarView's Ctrl+Z closes the panel when it brings an event back (#884, F5): focus comes back
  // to the chip from the panel, but someone already back in the grid is left where they are.
  it("hands focus back when closed from outside, only if it was in the panel", () => {
    const opener = chip("Design review");
    opener.focus();
    const onClose = vi.fn();
    const closeRef = { current: null as (() => void) | null };
    const props = {
      event: calendarEvent(),
      anchor: rect(100),
      calendar: null,
      color: "#000",
      milestone: null,
      onClose,
      closeRef,
    };

    const view = render(<CalendarEventPopover {...props} />);
    expect(view.getByRole("dialog").contains(document.activeElement)).toBe(true);
    closeRef.current!();
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(opener);
    view.unmount();
    expect(closeRef.current).toBeNull();

    render(<CalendarEventPopover {...props} />);
    const elsewhere = chip("Standup");
    elsewhere.focus();
    closeRef.current!();
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(document.activeElement).toBe(elsewhere);
  });

  it("re-points at the NEW chip when the panel is re-used without closing", () => {
    // The keyboard path: activating another chip with Enter swaps `event`/`anchor` on the mounted
    // instance rather than unmounting it (the mouse path closes first, via the outside-mousedown
    // dismissal). A capture-on-mount opener would hand focus back to the FIRST chip of the session.
    const first = chip("Design review");
    const second = chip("Standup");
    first.focus();

    const { rerender, getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    second.focus();
    rerender(
      <CalendarEventPopover
        event={calendarEvent({ id: "e2", summary: "Standup" })}
        anchor={rect(300)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    // Re-pointed, the panel takes focus again, as it did when it opened.
    expect(getByRole("dialog").contains(document.activeElement)).toBe(true);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.activeElement).toBe(second);
  });

  it("is a NON-modal dialog: no aria-modal", () => {
    // A guard, not a description. Turning this into a modal would put a trap on a panel that sits
    // over a live calendar, and a scrim over the grid you click to move between events.
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    expect(getByRole("dialog").getAttribute("aria-modal")).toBeNull();
  });

  it("does not restore focus on an outside click", () => {
    // The outside click has ALREADY moved focus to whatever was clicked; restoring would yank it
    // straight back off the thing the user just aimed at.
    const opener = chip("Design review");
    opener.focus();
    const elsewhere = chip("Somewhere else");

    render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    elsewhere.focus();
    fireEvent.mouseDown(elsewhere);
    expect(document.activeElement).toBe(elsewhere);
  });

  it("still names itself when the event has no title", () => {
    // Nothing pins the "summary is never blank" rule at the type — it is three separate producer-side
    // guarantees (Google's parse, the milestone suffix, the pinboard fallback), one of them in Rust.
    // A dialog announced with no name is a WCAG failure, so the naming lives at the consumer.
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent({ summary: "   " })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    const panel = getByRole("dialog");
    expect(panel.getAttribute("aria-label")).toBe("(no title)");
    // The visible heading says the same thing, rather than rendering as an empty line.
    expect(panel.querySelector("h2")?.textContent).toBe("(no title)");
  });

  it("names itself from the summary when there is one", () => {
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );

    expect(getByRole("dialog").getAttribute("aria-label")).toBe("Design review");
  });

  // #884 C7: an iCal series says how it repeats; a row still holding a raw rule from before says
  // only "Repeats", as does one with no summary. A Google occurrence holds no rule: its words come
  // from Google through `describeSeries` (C9b-2, below), and without it it reads "Repeats" too.
  it.each([
    ["Weekly on Monday", "Weekly on Monday"],
    ["FREQ=WEEKLY;BYDAY=MO", "Repeats"],
    [null, "Repeats"],
  ])("shows the repeat line for a summary of %s", (summary, expected) => {
    const { getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent({ recurring: true, recurrence_summary: summary })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    const text = getByRole("dialog").textContent ?? "";
    expect(text).toContain(expected);
    expect(text).not.toContain("FREQ=");
  });

  describe("how a Google occurrence repeats (C9b-2)", () => {
    const occurrence = (id = "e1") =>
      calendarEvent({ id, recurring: true, series_id: "abc", summary: "Gym" });

    /** A `describeSeries` whose answers the test gives, one per call, in order. */
    function deferred() {
      const answers: Array<{ resolve: (w: string | null) => void; reject: (e: Error) => void }> =
        [];
      const ask = vi.fn<(id: string) => Promise<string | null>>(
        () =>
          new Promise<string | null>((resolve, reject) => {
            answers.push({ resolve, reject });
          }),
      );
      return { ask, answers };
    }

    function popover(
      event: CalendarEvent,
      anchor: DOMRect,
      ask?: (id: string) => Promise<string | null>,
    ) {
      return (
        <CalendarEventPopover
          event={event}
          anchor={anchor}
          calendar={null}
          color="#000"
          milestone={null}
          onClose={() => {}}
          describeSeries={ask}
        />
      );
    }

    it('a Google occurrence reads "Repeats", then how it repeats', async () => {
      const { ask, answers } = deferred();
      const view = render(popover(occurrence(), rect(100), ask));
      expect(view.getByText("Repeats")).toBeTruthy();
      expect(ask).toHaveBeenCalledTimes(1);
      expect(ask).toHaveBeenCalledWith("e1");
      await act(async () => answers[0].resolve("Weekly on Monday"));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      expect(view.queryByText("Repeats")).toBeNull();
    });

    it('a failed or empty answer keeps "Repeats"', async () => {
      for (const answer of [new Error("offline"), null, "  "]) {
        const { ask, answers } = deferred();
        const view = render(popover(occurrence(), rect(100), ask));
        await act(async () =>
          answer instanceof Error ? answers[0].reject(answer) : answers[0].resolve(answer),
        );
        expect(view.getByText("Repeats")).toBeTruthy();
        view.unmount();
      }
    });

    it("a row with its own words, or a one-off, never asks", () => {
      const { ask } = deferred();
      const feed = calendarEvent({ recurring: true, recurrence_summary: "Weekly on Monday" });
      const view = render(popover(feed, rect(100), ask));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      view.unmount();
      const once = render(popover(calendarEvent({ series_id: null }), rect(100), ask));
      expect(once.queryByText("Repeats")).toBeNull();
      expect(ask).not.toHaveBeenCalled();
    });

    it("re-pointed at another event, the old words never show", async () => {
      const { ask, answers } = deferred();
      const view = render(popover(occurrence("e1"), rect(100), ask));
      await act(async () => answers[0].resolve("Weekly on Monday"));
      expect(view.getByText("Weekly on Monday")).toBeTruthy();
      // The keyboard path re-points the mounted panel: the next event reads "Repeats" at once.
      view.rerender(popover(occurrence("e2"), rect(300), ask));
      expect(view.queryByText("Weekly on Monday")).toBeNull();
      expect(view.getByText("Repeats")).toBeTruthy();
      // And an answer for an event no longer shown is dropped, even one that comes after the shown
      // event's own (that one kept by the backend, the older one still out at Google).
      view.rerender(popover(occurrence("e3"), rect(500), ask));
      await act(async () => answers[2].resolve("Daily"));
      expect(view.getByText("Daily")).toBeTruthy();
      await act(async () => answers[1].resolve("Weekly on Tuesday"));
      expect(view.queryByText("Weekly on Tuesday")).toBeNull();
      expect(view.getByText("Daily")).toBeTruthy();
    });

    it("the words arriving never move focus", async () => {
      // jsdom has no layout: the panel is taller once the words are in, and opens flipped above a
      // chip near the bottom, so the words move it.
      const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetHeight");
      Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
        configurable: true,
        get(this: HTMLElement) {
          return this.textContent?.includes("Weekly on Monday") ? 300 : 200;
        },
      });
      try {
        const { ask, answers } = deferred();
        const view = render(popover(occurrence(), rect(window.innerHeight - 60), ask));
        const panel = view.getByRole("dialog");
        const before = panel.style.top;
        const elsewhere = chip("Standup");
        elsewhere.focus();
        await act(async () => answers[0].resolve("Weekly on Monday"));
        expect(panel.style.top).not.toBe(before);
        expect(document.activeElement).toBe(elsewhere);
      } finally {
        if (height) Object.defineProperty(HTMLElement.prototype, "offsetHeight", height);
      }
    });

    it("re-pointed at another event, the old flags go even when its read fails", async () => {
      eventFlags.mockResolvedValueOnce([{ id: 1, type: "prep-needed" }]);
      const view = render(popover(calendarEvent({ uid: "u1" }), rect(100)));
      await act(async () => {});
      expect(view.getByText("prep needed")).toBeTruthy();
      eventFlags.mockRejectedValueOnce(new Error("locked"));
      view.rerender(popover(calendarEvent({ id: "e2", uid: "u2" }), rect(300)));
      await act(async () => {});
      expect(view.queryByText("prep needed")).toBeNull();
    });
  });

  // #884: CalendarView passes `editing`; FocusUpcoming doesn't, and must stay as it was.
  it("offers no editing at all without the editing prop", () => {
    const { queryByRole, getByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(queryByRole("button", { name: "Edit" })).toBeNull();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();
    expect(getByRole("dialog").textContent).not.toContain("PM can");
  });

  it("offers Edit and Delete when the event can change, and says why when it can't", () => {
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    const { getByRole, rerender, queryByRole, getByText } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{
          canEdit: true,
          canDelete: true,
          canDuplicate: false,
          reason: null,
          onEdit,
          onDelete,
          onDuplicate: () => {},
        }}
      />,
    );
    fireEvent.click(getByRole("button", { name: "Edit" }));
    expect(onEdit).toHaveBeenCalledOnce();
    fireEvent.click(getByRole("button", { name: "Delete" }));
    expect(onDelete).toHaveBeenCalledOnce();

    // A locked copy: its busy/free can change, but it can't be deleted, and the line says why.
    rerender(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{
          canEdit: true,
          canDelete: false,
          canDuplicate: false,
          reason: "Google has locked this event's title, time, place and description.",
          onEdit,
          onDelete,
          onDuplicate: () => {},
        }}
      />,
    );
    expect(getByRole("button", { name: "Edit" })).toBeTruthy();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();

    rerender(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={{
          canEdit: false,
          canDelete: false,
          canDuplicate: false,
          reason: "Someone else organises this event.",
          onEdit,
          onDelete,
          onDuplicate: () => {},
        }}
      />,
    );
    expect(queryByRole("button", { name: "Edit" })).toBeNull();
    expect(queryByRole("button", { name: "Delete" })).toBeNull();
    expect(getByText("Someone else organises this event.")).toBeTruthy();
  });

  // C18c: Duplicate sits before Edit, taking the push to the right from whichever came first.
  it("Duplicate shows only when offered and calls onDuplicate", () => {
    const onDuplicate = vi.fn();
    const editing = (canDuplicate: boolean, canEdit: boolean): PopoverEditing => ({
      canEdit,
      canDelete: true,
      canDuplicate,
      reason: null,
      onEdit: () => {},
      onDelete: () => {},
      onDuplicate,
    });
    const shown = (edit: PopoverEditing) => (
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
        editing={edit}
      />
    );
    const { getByRole, queryByRole, rerender } = render(shown(editing(false, true)));
    expect(queryByRole("button", { name: "Duplicate" })).toBeNull();
    expect(getByRole("button", { name: "Edit" }).className).toContain("ml-auto");
    rerender(shown(editing(true, true)));
    const duplicate = getByRole("button", { name: "Duplicate" });
    expect(duplicate.className).toContain("ml-auto");
    expect(getByRole("button", { name: "Edit" }).className).not.toContain("ml-auto");
    // Before Edit and Delete, in the row's order.
    const names = Array.from(getByRole("dialog").querySelectorAll("button")).map(
      (b) => b.textContent,
    );
    expect(names.indexOf("Duplicate")).toBeLessThan(names.indexOf("Edit"));
    fireEvent.click(duplicate);
    expect(onDuplicate).toHaveBeenCalledOnce();
    // Nothing to edit: Delete follows Duplicate, which still pushes right.
    rerender(shown(editing(true, false)));
    expect(getByRole("button", { name: "Delete" }).className).not.toContain("ml-auto");
    // Still a non-modal dialog, as the event's details are.
    expect(getByRole("dialog").getAttribute("aria-modal")).toBeNull();
  });

  // A description is shown line by line: the wrapper's class is what index.css gives `pre-line`
  // (GFM alone would fold single line breaks into spaces).
  it("shows a description inside the wrapper that keeps its lines", () => {
    const { getByText } = render(
      <CalendarEventPopover
        event={calendarEvent({ description: "Agenda\n1. intro\n2. demo" })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    const text = getByText(
      (_, el) =>
        el?.textContent === "Agenda\n1. intro\n2. demo" && el.tagName === "DIV" && !el.className,
    );
    expect(text.closest(".pm-event-desc")).not.toBeNull();
  });

  describe("a description's formatting (C19b)", () => {
    /** The pop-up showing `description` on one of `provider`'s calendars. */
    function shown(
      description: string,
      provider: string | null,
      over: Partial<CalendarEvent> = {},
    ) {
      const view = render(
        <CalendarEventPopover
          event={calendarEvent({ description, ...over })}
          anchor={rect(100)}
          calendar={provider === null ? null : calendarOf(provider)}
          color="#000"
          milestone={null}
          onClose={() => {}}
        />,
      );
      const box = view.container.ownerDocument.querySelector(".pm-event-desc") as HTMLElement;
      return { ...view, box, rich: box.querySelector(".pm-rich") };
    }

    it("a_google_description_shows_its_formatting", () => {
      // Before C19b, Markdown dropped the tags and this was an empty box.
      const { rich } = shown("<ul><li>One</li><li><b>Two</b></li></ul>", "google");
      expect(rich).not.toBeNull();
      expect([...rich!.querySelectorAll("li")].map((li) => li.textContent)).toEqual(["One", "Two"]);
      expect(rich!.querySelector("li > b")?.textContent).toBe("Two");
      expect(markdownView).not.toHaveBeenCalled();
    });

    it("a reference alone is formatting Google wrote", () => {
      const { rich } = shown("Tom &amp; Jerry", "google");
      expect(rich?.textContent).toBe("Tom & Jerry");
    });

    // Outlook's, a feed's, and Google's own plain text keep the Markdown path, whose address links
    // are what they always were (DECISIONS 10-10).
    it.each([
      ["an_outlook_description_keeps_its_address_links", "See <https://zoom.us/j/1>", "microsoft"],
      ["a feed's formatting", "<b>Agenda</b>", "ics"],
      ["a calendar PM can't name", "<b>Agenda</b>", null],
      ["a_plain_google_description_renders_as_before", "Agenda\n1. intro\n**not bold**", "google"],
      ["Teams' address in angle brackets", "Click here to join<https://example.com/j/1>", "google"],
      ["a guest's address in angle brackets", "Ann <ann@example.com>", "google"],
      ["an Outlook element only", "<o:p>Room 4</o:p>", "google"],
    ])("%s draws through Markdown", (_name, description, provider) => {
      const { rich } = shown(description, provider);
      expect(rich).toBeNull();
      expect(markdownView).toHaveBeenCalledWith({ children: description, plainLines: true });
    });

    it("a bare address inside bold is a link", () => {
      const { rich } = shown("<b>Join at https://example.com/j/1</b>", "google");
      const link = rich?.querySelector("b > a");
      expect(link?.getAttribute("href")).toBe("https://example.com/j/1");
      expect(link?.getAttribute("target")).toBe("_blank");
    });

    it("says where the rest of a cut description is", () => {
      const rest = (provider: string | null, cut: boolean) => {
        const { box, unmount } = shown("Long", provider, { description_cut: cut });
        const said = box.textContent;
        unmount();
        return said;
      };
      expect(rest("google", true)).toBe("LongThe rest is in Google Calendar.");
      expect(rest("microsoft", true)).toBe("LongThe rest is in Outlook.");
      expect(rest("ics", true)).toBe("LongThe rest is in the calendar it comes from.");
      expect(rest("google", false)).toBe("Long");
    });

    it("a mail link opens the mail app, and says so with the address when it can't", async () => {
      openUrl.mockRejectedValue("PM couldn't open your mail app.");
      const { rich, getByRole } = shown(
        '<a href="mailto:ann@example.com?subject=Hi">Email us</a>',
        "google",
      );
      expect(getByRole("status").textContent).toBe("");
      await act(async () => {
        expect(fireEvent.click(rich!.querySelector("a")!)).toBe(false);
      });
      // The whole link goes; the backend keeps the address alone.
      expect(openUrl).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com?subject=Hi");
      expect(getByRole("status").textContent).toBe(
        "PM couldn't open your mail app. The address is ann@example.com.",
      );
      // A try that works takes the line away.
      openUrl.mockResolvedValue(undefined);
      await act(async () => {
        fireEvent.click(rich!.querySelector("a")!);
      });
      expect(getByRole("status").textContent).toBe("");
    });

    it("a mail link the backend refuses says the backend's words", async () => {
      openUrl.mockRejectedValue("That doesn't look like an email address.");
      const { rich, getByRole } = shown('<a href="mailto:a%0Ab@example.com">x</a>', "google");
      await act(async () => {
        fireEvent.click(rich!.querySelector("a")!);
      });
      expect(getByRole("status").textContent).toBe("That doesn't look like an email address.");
    });

    it("a line about one event's mail link never shows on the next", async () => {
      openUrl.mockRejectedValue("PM couldn't open your mail app.");
      const props = {
        anchor: rect(100),
        calendar: calendarOf("google"),
        color: "#000",
        milestone: null,
        onClose: () => {},
      };
      const description = '<a href="mailto:ann@example.com">Email us</a>';
      const { container, getByRole, rerender } = render(
        <CalendarEventPopover event={calendarEvent({ id: "e1", description })} {...props} />,
      );
      await act(async () => {
        fireEvent.click(container.ownerDocument.querySelector(".pm-rich a")!);
      });
      expect(getByRole("status").textContent).not.toBe("");
      // The keyboard path re-points the mounted panel at another chip.
      rerender(
        <CalendarEventPopover event={calendarEvent({ id: "e2", description })} {...props} />,
      );
      expect(getByRole("status").textContent).toBe("");
    });

    it("Markdown's mail links open the same way, and no link loads inside PM's window", async () => {
      markdownView.mockImplementation(() => (
        <div>
          <a href="mailto:ann@example.com">Ann</a>
          <a href="/somewhere">relative</a>
          <a href="">emptied</a>
          <a href="https://example.com" target="_blank" rel="noreferrer">
            web
          </a>
        </div>
      ));
      const { box } = shown("Mail Ann <ann@example.com>", "microsoft");
      const link = (words: string) =>
        [...box.querySelectorAll("a")].find((a) => a.textContent === words)!;
      await act(async () => {
        expect(fireEvent.click(link("Ann"))).toBe(false);
      });
      expect(openUrl).toHaveBeenCalledExactlyOnceWith("mailto:ann@example.com");
      expect(fireEvent.click(link("relative"))).toBe(false);
      expect(fireEvent.click(link("emptied"))).toBe(false);
      // A web link is useExternalLinks' to open, so the pop-up leaves its default alone.
      expect(fireEvent.click(link("web"))).toBe(true);
      expect(openUrl).toHaveBeenCalledTimes(1);
    });
  });

  // The call and the event in Google open in the default browser; each link can be copied instead,
  // for another browser.
  it("offers the call's link and the event's link to copy, beside the buttons that open them", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    try {
      const call = "https://meet.google.com/abc-defg-hij";
      const page = "https://www.google.com/calendar/event?eid=ZTE";
      const { getByRole } = render(
        <CalendarEventPopover
          event={calendarEvent({ conference_url: call, html_link: page })}
          anchor={rect(100)}
          calendar={null}
          color="#000"
          milestone={null}
          onClose={() => {}}
        />,
      );
      // Each says what it copies, so the two can be told apart by name alone.
      await act(async () => {
        fireEvent.click(getByRole("button", { name: "Copy link to the call" }));
      });
      expect(writeText).toHaveBeenLastCalledWith(call);
      await act(async () => {
        fireEvent.click(getByRole("button", { name: "Copy link to this event" }));
      });
      expect(writeText).toHaveBeenLastCalledWith(page);
    } finally {
      Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    }
  });

  it("offers no link to copy when the event has none", () => {
    const { queryByRole } = render(
      <CalendarEventPopover
        event={calendarEvent()}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(queryByRole("button", { name: /^Copy link/ })).toBeNull();
  });

  // The colour an event was given in Google is named, not only shown: the fill's hue alone doesn't
  // carry it for everyone.
  it("names the colour an event was given in Google, and says nothing for one without", () => {
    const { getByText, queryByText, rerender } = render(
      <CalendarEventPopover
        event={calendarEvent({ color_id: "1" })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(getByText("Colour")).toBeTruthy();
    expect(getByText("Lavender")).toBeTruthy();
    rerender(
      <CalendarEventPopover
        event={calendarEvent({ color_id: null })}
        anchor={rect(100)}
        calendar={null}
        color="#000"
        milestone={null}
        onClose={() => {}}
      />,
    );
    expect(queryByText("Colour")).toBeNull();
  });

  // #884 C14b: "Going?" at the foot of the card, one per copy the user can reply from.
  describe("the reply strip", () => {
    const SAVED: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };
    const copy = (over: Partial<PopoverRsvp> = {}): PopoverRsvp => ({
      key: "e1",
      current: "awaiting",
      repeating: false,
      forWhom: null,
      onReply: vi.fn(() => Promise.resolve(SAVED)),
      ...over,
    });
    const editing = (rsvp?: PopoverRsvp[]): PopoverEditing => ({
      canEdit: false,
      canDelete: false,
      canDuplicate: false,
      reason: null,
      onEdit: () => {},
      onDelete: () => {},
      onDuplicate: () => {},
      rsvp,
    });
    function popover(edit?: PopoverEditing, event = calendarEvent(), onClose = () => {}) {
      return (
        <CalendarEventPopover
          event={event}
          anchor={rect(100)}
          calendar={null}
          color="#000"
          milestone={null}
          onClose={onClose}
          editing={edit}
        />
      );
    }
    const me = (response: string) => ({
      name: null,
      email: "me@x.org",
      response,
      optional: false,
      organizer: false,
      self: true,
    });

    it("no strip without editing or without copies to reply from", () => {
      for (const edit of [undefined, editing(), editing([])]) {
        const view = render(popover(edit));
        expect(view.queryByText(/^Going\?/)).toBeNull();
        expect(view.queryByRole("button", { name: "Yes" })).toBeNull();
        // Not even an empty section: the header, the body and the actions only.
        expect(view.getByRole("dialog").children).toHaveLength(3);
        view.unmount();
      }
      const view = render(popover(editing([copy()])));
      expect(view.getByRole("dialog").children).toHaveLength(4);
    });

    it("one copy: Going? with no name; two: each names its calendar", () => {
      const view = render(popover(editing([copy()])));
      expect(view.getByRole("group", { name: "Going?" })).toBeTruthy();
      view.rerender(
        popover(editing([copy({ forWhom: "Work" }), copy({ key: "e2", forWhom: "Personal" })])),
      );
      expect(view.getByRole("group", { name: "Going? Replying for Work." })).toBeTruthy();
      expect(view.getByRole("group", { name: "Going? Replying for Personal." })).toBeTruthy();
      // Still a NON-modal dialog with the strip in it.
      expect(view.getByRole("dialog").getAttribute("aria-modal")).toBeNull();
    });

    it("a reply calls that copy's onReply with the scope", async () => {
      const first = copy({ forWhom: "Work" });
      const second = copy({ key: "e2", forWhom: "Personal", repeating: true });
      const view = render(popover(editing([first, second])));
      const strip = view.getByRole("group", { name: "Going? Replying for Personal." });
      fireEvent.click(within(strip).getByRole("button", { name: "No" }));
      await act(async () => fireEvent.click(view.getByRole("button", { name: "All events" })));
      expect(second.onReply).toHaveBeenCalledWith("no", "all");
      expect(first.onReply).not.toHaveBeenCalled();
    });

    it("the occurrence question's Escape stays in the strip", () => {
      const onClose = vi.fn();
      const view = render(popover(editing([copy({ repeating: true })]), calendarEvent(), onClose));
      fireEvent.click(view.getByRole("button", { name: "Yes" }));
      fireEvent.keyDown(view.getByRole("button", { name: "Back" }), { key: "Escape" });
      expect(onClose).not.toHaveBeenCalled();
      // Back at the reply buttons, Escape closes the popover as ever.
      fireEvent.keyDown(view.getByRole("button", { name: "Yes" }), { key: "Escape" });
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    // The calendar renders the popover with its row read afresh (CalendarView), so a reply that
    // landed shows in the guest list without reopening it.
    it("after a reply the guest list shows it", () => {
      const view = render(
        popover(editing([copy()]), calendarEvent({ attendees: [me("needsAction")] })),
      );
      expect(view.getByRole("dialog").textContent).toContain("no reply");
      view.rerender(
        popover(editing([copy({ current: "no" })]), calendarEvent({ attendees: [me("declined")] })),
      );
      expect(view.getByRole("dialog").textContent).toContain("declined");
      expect(view.getByRole("button", { name: "No" }).getAttribute("aria-pressed")).toBe("true");
    });

    // C14 critiques C1-L4, C2-L3: anything that changes the panel's size re-places it, the
    // strip's question and lines included, not only the words and flags it knows of.
    it("re-places the panel whenever its size changes", () => {
      let resized: (() => void) | null = null;
      const real = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
      (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
        constructor(callback: () => void) {
          resized = callback;
        }
        observe() {}
        unobserve() {}
        disconnect() {}
      };
      const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "offsetHeight");
      let tall = 200;
      Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
        configurable: true,
        get: () => tall,
      });
      try {
        const view = render(
          <CalendarEventPopover
            event={calendarEvent()}
            anchor={rect(window.innerHeight - 60)}
            calendar={null}
            color="#000"
            milestone={null}
            onClose={() => {}}
            editing={editing([copy({ repeating: true })])}
          />,
        );
        const panel = view.getByRole("dialog");
        const before = panel.style.top;
        tall = 320;
        act(() => resized?.());
        expect(panel.style.top).not.toBe(before);
      } finally {
        if (height) Object.defineProperty(HTMLElement.prototype, "offsetHeight", height);
        (globalThis as { ResizeObserver?: unknown }).ResizeObserver = real;
      }
    });
  });
});
