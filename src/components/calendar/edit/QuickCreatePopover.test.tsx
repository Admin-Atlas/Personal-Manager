// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// Quick create (#884, C10c): Enter saves (never on a repeat or while composing text), More options
// hands the draft to the full editor without sending anything, a click outside or Escape closes only
// an empty draft (and Escape only with focus in the box), a click in a picker it opened or on its
// ghost isn't outside, it registers as a dialog layer without being modal, while a save is out
// nothing sends, closes or hands over again, a token that ended unproven is never dropped silently,
// a date it can't read refuses until the field is given one (the field named, marked, and where a
// save it stops puts focus; and it goes on refusing with More options), a titled draft that goes
// with the tab is handed back to open on again, the keyboard comes back into the box even while it's
// locked, the ghost is told how far the event has got, and the calendar list the keyboard can reach
// stays inside the box. Repeating (C11): a ready-made rule is sent with the create, one the backend
// refuses stops every save (a start the clocks skip said once), one from before the months PM syncs
// is said to show inside them, a rule's own day follows a moved date, the Repeat list stays inside
// the box (Escape there closing only it), and Custom…, clicked or reached by the keyboard, carries
// on in the full editor with its dialog open, sending nothing.

import { createRef, StrictMode } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  Calendar,
  CalendarAccount,
  CalendarOverview,
  NewEventDraft,
  RepeatPreset,
  RepeatRule,
  RepeatStart,
  RepeatView,
  WriteOutcome,
} from "../../../lib/types";
import { CREATE_LOST } from "../../../lib/calendarEdit/editReasons";
import type { EditorFields } from "../../../lib/calendarEdit/eventDraft";
import { createTargets } from "../../../lib/calendarEdit/createTargets";
import { isAnyDialogOpen } from "../../../lib/useDialogLayer";
import { Popover } from "../../ui";

const createEvent =
  vi.fn<
    (calendarId: string, draft: NewEventDraft, token: string | null) => Promise<WriteOutcome>
  >();
const noteCreateResult = vi.fn();
const noteHeldCreate = vi.fn();
const recurrencePresets = vi.fn<(day: string) => Promise<RepeatPreset[]>>();
const describeRecurrence = vi.fn<(rule: RepeatRule, start: RepeatStart) => Promise<RepeatView>>();
let deviceZone: string | null = "Europe/London";

// The repeat menu's two reads, answered as the test says.
vi.mock("../../../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  recurrencePresets: (day: string) => recurrencePresets(day),
  describeRecurrence: (rule: RepeatRule, start: RepeatStart) => describeRecurrence(rule, start),
}));

vi.mock("./useEventWrites", async (importOriginal) => ({
  // The rest as it is, for the write notices drawn beside the box.
  ...(await importOriginal<object>()),
  // Every argument passes through, so an extra one, or a dropped token, shows in the calls.
  createEvent: (...a: [string, NewEventDraft, string | null]) => createEvent(...a),
  noteCreateResult: (...a: unknown[]) => noteCreateResult(...a),
  noteHeldCreate: (...a: unknown[]) => noteHeldCreate(...a),
}));

vi.mock("../../../theme/timezones", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  deviceTimeZoneOrNull: () => deviceZone,
}));

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { QuickCreatePopover, type LeftDraft } from "./QuickCreatePopover";
import { WriteNotices } from "./WriteNotices";

const ME = "gcal:me@x.com";
const WORK = "gcal:work@x.com";
const PERSONAL = `${ME}:me@x.com`;
const WORK_CAL = `${WORK}:work@x.com`;

function account(id: string): CalendarAccount {
  const email = id.slice(id.indexOf(":") + 1);
  return { id, provider: "google", email, label: email, state: "ok", last_synced_at: null };
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
  accounts: [account(ME), account(WORK)],
  calendars: [
    calendar(ME, "me@x.com", { name: "Personal", is_primary: true }),
    calendar(WORK, "work@x.com", { name: "Work", is_primary: true }),
  ],
  last_sync: null,
  window_days: 7,
  mirror_start: "2026-09-01T00:00:00Z",
  mirror_end: "2027-11-01T00:00:00Z",
  editing: { [ME]: "on", [WORK]: "on" },
  // Work's account makes half-hour events; Personal's has none read, so an hour.
  default_length: { [WORK]: 30 },
  auto_meet: {},
};

const seed: EditorFields = {
  summary: "",
  location: "",
  description: "",
  time: {
    kind: "timed",
    start_date: "2026-10-12",
    start_time: "09:00",
    start_zone: "Europe/London",
    end_date: "2026-10-12",
    end_time: "10:00",
    end_zone: "Europe/London",
  },
  recurrence: { kind: "once" },
  guests: [],
  guest_perms: { modify: false, invite_others: true, see_others: true },
  meet: false,
  reminders: { use_default: true, overrides: [] },
  colour: { kind: "calendar" },
  show_as: "busy",
  visibility: "default",
};

/** What the box sends for the seed titled `summary`, on Personal. */
const sentFor = (summary: string) => ({
  calendarId: PERSONAL,
  // Every field the seed has: no guests (quick create has no guest box), and Google's own guest
  // permissions, unsaid (C12d); the calendar's notifications and colour, as null (quick create has
  // neither row, C13).
  draft: {
    summary,
    time: seed.time,
    location: seed.location,
    description: seed.description,
    show_as: seed.show_as,
    visibility: seed.visibility,
    recurrence: null,
    guests: [],
    reminders: null,
    colour: null,
    meet: false,
  } as NewEventDraft,
});

const saved: WriteOutcome = { outcome: "saved", warnings: [], left_behind: [] };
const ambiguous: WriteOutcome = { outcome: "ambiguous", retry_token: "ref-1" };

/** The header button's rect, as the calendar hands it over, or another box (a slot on the grid). */
const rect = (left = 40, top = 10, right = 140, bottom = 38): DOMRect =>
  ({
    left,
    top,
    right,
    bottom,
    width: right - left,
    height: bottom - top,
    x: left,
    y: top,
    toJSON: () => ({}),
  }) as DOMRect;

/** A promise whose answer (or failure) the test gives when it chooses. */
function later() {
  let answer: (o: WriteOutcome) => void = () => {};
  let fail: (e: Error) => void = () => {};
  const promise = new Promise<WriteOutcome>((resolve, reject) => {
    answer = resolve;
    fail = reject;
  });
  return { promise, answer, fail };
}

const lost: WriteOutcome = { outcome: "failed", message: CREATE_LOST };
const unreachable: WriteOutcome = { outcome: "failed", message: "PM couldn't reach Google." };
const LIST = { name: "Calendars that can take a new event" };
const WHERE = { name: "Personal", hidden: false };

// jsdom ships no ResizeObserver; the box and the pickers' popovers build one.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

const weeklyOn = (day: "mon" | "tue"): RepeatRule => ({
  pattern: { kind: "weekly", days: [day] },
  interval: 1,
  ends: { kind: "never" },
});

beforeEach(() => {
  deviceZone = "Europe/London";
  // The ready-made rules for the start date (the first two are enough), and a rule that starts where
  // asked.
  recurrencePresets.mockImplementation(async (day) => [
    { label: "Daily", rule: { ...weeklyOn("mon"), pattern: { kind: "daily" } } },
    day === "2026-10-13"
      ? { label: "Weekly on Tuesday", rule: weeklyOn("tue") }
      : { label: "Weekly on Monday", rule: weeklyOn("mon") },
  ]);
  describeRecurrence.mockImplementation(async (_rule, start) => ({
    outcome: "ready",
    summary: "Weekly on Monday",
    starts: start.date,
    moved: false,
  }));
});

afterEach(() => {
  cleanup();
  // Reset, not clear: an answer one test queued and never used must not reach the next.
  vi.resetAllMocks();
});

/** The box under a stand-in header button, beside a stand-in grid with the ghost on it, and, when
 *  asked, another popover open elsewhere on the page, a draft to carry on with, or <StrictMode>
 *  around it all (as the app's development build runs: every effect set up twice); or opened
 *  beside a slot on the grid (C17), with a time the user dragged out. */
function open(
  over: {
    otherPopover?: boolean;
    resume?: LeftDraft;
    calendarId?: string;
    strict?: boolean;
    anchor?: DOMRect;
    placement?: "below" | "beside";
    timeTouched?: boolean;
    overview?: CalendarOverview;
  } = {},
) {
  const shown = over.overview ?? overview;
  const onClose = vi.fn();
  const onMore = vi.fn();
  const onDraft = vi.fn();
  const onLeave = vi.fn();
  const header = createRef<HTMLButtonElement>();
  const focusRef: { current: (() => void) | null } = { current: null };
  const page = (
    <>
      <button ref={header} type="button">
        + New event
      </button>
      <div data-testid="grid">
        <div data-draft-event="" data-testid="ghost">
          (no title)
        </div>
        <button type="button">Synced</button>
      </div>
      {over.otherPopover && (
        <Popover
          ariaLabel="Another popover"
          trigger={({ toggle }) => (
            <button type="button" onClick={toggle}>
              Other
            </button>
          )}
        >
          <button type="button">Inside the other</button>
        </Popover>
      )}
      <QuickCreatePopover
        anchor={over.anchor ?? rect()}
        placement={over.placement}
        timeTouched={over.timeTouched}
        seed={seed}
        targets={createTargets(shown)}
        calendarId={over.calendarId ?? PERSONAL}
        overview={shown}
        hidden={new Set()}
        colorOf={() => "var(--accent)"}
        shapeOf={() => undefined}
        returnFocusTo={header}
        focusRef={focusRef}
        resume={over.resume}
        onDraft={onDraft}
        onMore={onMore}
        onClose={onClose}
        onLeave={onLeave}
      />
    </>
  );
  const view = render(over.strict ? <StrictMode>{page}</StrictMode> : page);
  return { onClose, onMore, onDraft, onLeave, header, focusRef, unmount: view.unmount };
}

/** Unmount, and let what the box leaves as it really goes (after StrictMode's rehearsal of an
 *  unmount can no longer be the one) be said. */
async function leave(unmount: () => void) {
  unmount();
  await act(async () => {});
}

const title = () => screen.getByLabelText<HTMLInputElement>("Title");
const outside = () => fireEvent.mouseDown(screen.getByTestId("grid"));
const box = () => screen.getByRole("dialog", { name: "New event" });

/** An element's one z-index utility, as a number (`z-50`, `z-[45]`). jsdom paints nothing, so the
 *  stacking is read from the classes. */
function zOf(el: Element | null): number {
  const z = Array.from(el?.classList ?? [])
    .map((c) => /^z-\[?(\d+)\]?$/.exec(c))
    .filter((m) => m !== null);
  expect(z).toHaveLength(1);
  return Number(z[0]![1]);
}

describe("QuickCreatePopover", () => {
  it("Enter in the title creates with no notify and closes on saved", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose, header } = open();
    expect(document.activeElement).toBe(title());
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    // The calendar, every field of the draft, and no token or answer: no repeat, and no guests, so
    // no one to notify (C12d: the guest list goes, empty); the calendar's notifications and colour,
    // as null (C13: quick create has neither row); no video call asked for (C18b).
    expect(createEvent.mock.calls).toEqual([[PERSONAL, sentFor("Dentist").draft, null]]);
    expect(Object.keys(createEvent.mock.calls[0][1]).sort()).toEqual([
      "colour",
      "description",
      "guests",
      "location",
      "meet",
      "recurrence",
      "reminders",
      "show_as",
      "summary",
      "time",
      "visibility",
    ]);
    expect(noteCreateResult).toHaveBeenCalledWith(sentFor("Dentist"), null, saved, {
      name: "Personal",
      hidden: false,
    });
    // Back to "+ New event", which Shift+C never focused.
    expect(document.activeElement).toBe(header.current);
  });

  it("Shift+Enter, Ctrl+Enter and Enter while composing don't save", async () => {
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter", shiftKey: true });
    fireEvent.keyDown(title(), { key: "Enter", ctrlKey: true });
    fireEvent.keyDown(title(), { key: "Enter", isComposing: true });
    // WebKit's Enter that ends a composition comes after it, marked only by its key code.
    fireEvent.keyDown(title(), { key: "Enter", keyCode: 229 });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("More options hands over the draft without saving", () => {
    const { onMore, onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "11:00" } });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    expect(createEvent).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(onMore).toHaveBeenCalledWith({
      draft: {
        ...seed,
        summary: "Dentist",
        time: { ...seed.time, start_time: "11:00", end_time: "12:00" },
      },
      calendarId: PERSONAL,
      held: null,
      unproven: null,
      showAsTouched: false,
      timeTouched: true,
      meetTouched: false,
      badDate: null,
      openCustom: false,
    });
  });

  it("an outside click discards only an empty title", () => {
    const first = open();
    outside();
    expect(first.onClose).toHaveBeenCalledWith(false);
    // An outside click put focus where it was aimed: it isn't pulled back to the button.
    expect(document.activeElement).not.toBe(first.header.current);
    cleanup();
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    outside();
    expect(onClose).not.toHaveBeenCalled();
    // Spaces alone aren't a title.
    fireEvent.change(title(), { target: { value: "   " } });
    outside();
    expect(onClose).toHaveBeenCalledWith(false);
  });

  it("a click on its ghost on the grid isn't outside", () => {
    const { onClose } = open();
    fireEvent.mouseDown(screen.getByTestId("ghost"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a click inside the date picker is not outside, and a picked date keeps the box", () => {
    const { onClose, onDraft } = open();
    fireEvent.click(screen.getByLabelText("Date"));
    const day = screen.getByRole("button", { name: /\b14-10(-2026)?$/ });
    fireEvent.mouseDown(day);
    fireEvent.click(day);
    expect(onClose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
    // The event moved whole, and the ghost with it.
    const [draft] = onDraft.mock.lastCall!;
    expect(draft.time).toEqual({ ...seed.time, start_date: "2026-10-14", end_date: "2026-10-14" });
  });

  it("Escape keeps a titled draft and closes an empty one; ✕ discards either", () => {
    const first = open();
    fireEvent.keyDown(title(), { key: "Escape" });
    expect(first.onClose).toHaveBeenCalledWith(false);
    expect(document.activeElement).toBe(first.header.current);
    cleanup();
    const second = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Escape" });
    expect(second.onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(second.onClose).toHaveBeenCalledWith(false);
    expect(document.activeElement).toBe(second.header.current);
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("Escape in another popover doesn't discard the draft", () => {
    const { onClose } = open({ otherPopover: true });
    // Opened with a click (not a press, which would be a click outside the box).
    fireEvent.click(screen.getByRole("button", { name: "Other" }));
    // Even an empty draft stays: the key is that popover's, which closes.
    const inside = screen.getByRole("button", { name: "Inside the other" });
    inside.focus();
    fireEvent.keyDown(inside, { key: "Escape" });
    expect(screen.queryByRole("group", { name: "Another popover" })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    // Nor is Escape the box's with focus somewhere else on the page.
    const elsewhere = screen.getByRole("button", { name: "+ New event" });
    elsewhere.focus();
    fireEvent.keyDown(elsewhere, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
  });

  it("stays non-modal but registers as a dialog layer", () => {
    const { unmount } = open();
    const box = screen.getByRole("dialog", { name: "New event" });
    expect(box.getAttribute("aria-modal")).toBeNull();
    // So the calendar's keys and swipe, and Ctrl+K, stand down while it's open.
    expect(isAnyDialogOpen()).toBe(true);
    unmount();
    expect(isAnyDialogOpen()).toBe(false);
  });

  it("an ambiguous create locks the fields and offers Check again with the same token", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    expect(
      await screen.findByText(/^PM lost touch with Google while creating “Dentist”/),
    ).toBeTruthy();
    expect(title().disabled).toBe(true);
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(
      screen.getByRole<HTMLButtonElement>("button", { name: "Calendar: Personal" }).disabled,
    ).toBe(true);
    // A click outside leaves it, title and all.
    outside();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, sentFor("Dentist").draft, null],
      [PERSONAL, sentFor("Dentist").draft, "ref-1"],
    ]);
  });

  it("a held token goes to the full editor with More options, or stays as a notice on ✕", async () => {
    createEvent.mockResolvedValue(ambiguous);
    const first = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByRole("button", { name: "Check again" });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    expect(first.onMore).toHaveBeenCalledWith(
      expect.objectContaining({ held: { token: "ref-1", sent: sentFor("Dentist") } }),
    );
    // The full editor speaks for it now: no notice as the box goes, and nothing handed back.
    await leave(first.unmount);
    expect(noteHeldCreate).not.toHaveBeenCalled();
    expect(first.onLeave).not.toHaveBeenCalled();

    const second = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByRole("button", { name: "Check again" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(second.onClose).toHaveBeenCalledWith(false);
    await leave(second.unmount);
    expect(noteHeldCreate.mock.calls).toEqual([[sentFor("Dentist"), "ref-1"]]);
  });

  it("a second Enter while saving sends nothing, and nothing closes or hands over", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { onClose, onMore } = open();
    // No title: a click outside or Escape would close it, if a save weren't out.
    fireEvent.keyDown(title(), { key: "Enter" });
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Saving…" }).disabled).toBe(true);
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(screen.getByRole("button", { name: "Saving…" }));
    const more = screen.getByRole<HTMLButtonElement>("button", { name: "More options" });
    const discard = screen.getByRole<HTMLButtonElement>("button", { name: "Discard" });
    expect(more.disabled).toBe(true);
    expect(discard.disabled).toBe(true);
    fireEvent.click(more);
    fireEvent.click(discard);
    outside();
    fireEvent.keyDown(title(), { key: "Escape" });
    expect(createEvent).toHaveBeenCalledTimes(1);
    expect(onClose).not.toHaveBeenCalled();
    expect(onMore).not.toHaveBeenCalled();
    await act(async () => answer.answer(saved));
    expect(onClose).toHaveBeenCalledWith(true);
  });

  it("a no-zone device disables the times and Save, and says nothing can be saved", async () => {
    deviceZone = null;
    open();
    expect(screen.getByLabelText<HTMLSelectElement>("Start time").disabled).toBe(true);
    expect(screen.getByRole("switch", { name: "All day" }).hasAttribute("disabled")).toBe(true);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    expect(
      screen.getByText(
        "PM couldn't tell which time zone this computer is in, so it can't save a new event. Set the time zone in your system settings.",
      ),
    ).toBeTruthy();
    // The title can still be typed: only the save waits for a zone.
    expect(title().disabled).toBe(false);
    // Enter in the title sends nothing, and leaves the caret where it was.
    fireEvent.change(title(), { target: { value: "Dentist" } });
    title().focus();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
  });

  it("the ghost's draft follows the times and the calendar", () => {
    const { onDraft } = open();
    expect(onDraft).toHaveBeenLastCalledWith(seed, PERSONAL, "unsaved");
    fireEvent.change(title(), { target: { value: "Dentist" } });
    expect(onDraft).toHaveBeenLastCalledWith({ ...seed, summary: "Dentist" }, PERSONAL, "unsaved");
    // Another calendar: its account's half-hour length, while the time is still the seed's.
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    fireEvent.click(screen.getByRole("radio", { name: "Work" }), { detail: 1 });
    expect(onDraft).toHaveBeenLastCalledWith(
      { ...seed, summary: "Dentist", time: { ...seed.time, end_time: "09:30" } },
      WORK_CAL,
      "unsaved",
    );
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "14:00" } });
    expect(onDraft.mock.lastCall![0].time).toEqual({
      ...seed.time,
      start_time: "14:00",
      end_time: "14:30",
    });
  });

  // C17: a range dragged out on the grid keeps its own length through All day and back, never the
  // account's (Work's half hour here).
  it("a dragged range keeps its length through All day and back", () => {
    const { onDraft } = open({ placement: "beside", timeTouched: true, calendarId: WORK_CAL });
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(onDraft.mock.lastCall![0].time.kind).toBe("all_day");
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(onDraft.mock.lastCall![0].time).toMatchObject({
      kind: "timed",
      start_date: "2026-10-12",
      start_time: "09:00",
      end_date: "2026-10-12",
      end_time: "10:00",
    });
  });

  // C17: a range dragged out on the grid is the user's choice, so another calendar never re-seeds
  // it; a slot clicked takes the new calendar's length, as the button's does (above).
  it("a dragged range isn't re-seeded on a calendar change; a click's is", () => {
    for (const timeTouched of [true, false]) {
      const { onDraft, unmount } = open({ placement: "beside", timeTouched });
      fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
      fireEvent.click(screen.getByRole("radio", { name: "Work" }), { detail: 1 });
      expect(onDraft).toHaveBeenLastCalledWith(
        timeTouched ? seed : { ...seed, time: { ...seed.time, end_time: "09:30" } },
        WORK_CAL,
        "unsaved",
      );
      unmount();
    }
  });

  // C17: from a slot on the grid, the box opens beside it, level with its top: to its right, or to
  // its left when the window has no room there, as Google's does.
  it("opens beside a slot: to its right, else its left, level with its top", () => {
    const at = (left: number, right: number) => {
      const { unmount } = open({ anchor: rect(left, 200, right, 240), placement: "beside" });
      const placed = [box().style.left, box().style.top];
      unmount();
      return placed;
    };
    const width = 300;
    const offset = vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(width);
    try {
      // Room on both sides: on the right.
      expect(at(500, 600)).toEqual(["606px", "200px"]);
      // No room on its left either: on its right, kept inside the window.
      expect(at(100, 200)).toEqual(["206px", "200px"]);
      // No room on the right of the window: on the slot's left.
      const right = window.innerWidth - 50;
      expect(at(right - 100, right)).toEqual([`${right - 100 - 6 - width}px`, "200px"]);
    } finally {
      offset.mockRestore();
    }
    // Under a button, as ever.
    open();
    expect([box().style.left, box().style.top]).toEqual(["40px", "44px"]);
  });

  it("an end on another day is words, and all day keeps the date", () => {
    open();
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "23:30" } });
    expect(screen.queryByLabelText("End time")).toBeNull();
    expect(screen.getByText(/^ends 13-10(-2026)? · 00:30$/)).toBeTruthy();
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.getByLabelText<HTMLInputElement>("Day").value).toBe("12-10-2026");
    expect(screen.queryByLabelText("Start time")).toBeNull();
  });

  // A token that ended without Google showing the event made (lost after hours held, or made and
  // since deleted): PM may have made it, so its words outlive the box, whichever way it goes.
  it("after a token ends unproven, ✕ leaves its words as a notice", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(lost);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    const box = screen.getByRole("dialog", { name: "New event" });
    expect(
      within(box).getByText(
        "PM lost touch with Google and can't tell whether “Dentist” was made. Look for it in Google Calendar before creating it again, so it isn't made twice.",
      ),
    ).toBeTruthy();
    expect(box.textContent).not.toContain("Nothing has been saved");
    // Escape keeps a titled draft; ✕ throws it away, the words staying.
    fireEvent.keyDown(title(), { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalledWith(false);
    expect(noteCreateResult).not.toHaveBeenCalled();
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([[sentFor("Dentist"), null, lost, WHERE]]);
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  it("an untitled box a click outside closes after a token ends on gone still leaves the notice", async () => {
    const gone: WriteOutcome = { outcome: "gone" };
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(gone);
    const { onClose, unmount } = open();
    // No title: a click outside closes it.
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    outside();
    expect(onClose).toHaveBeenCalledWith(false);
    await leave(unmount);
    expect(noteCreateResult.mock.calls).toEqual([[sentFor(""), null, gone, WHERE]]);
  });

  it("made again after the look-first words, it says Created and nothing more", async () => {
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockResolvedValueOnce(lost)
      .mockResolvedValueOnce(saved);
    const { onClose, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    fireEvent.click(await screen.findByRole("button", { name: "Save" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    await leave(unmount);
    // A new create, under no token, and only its Created.
    expect(createEvent.mock.calls.map((c) => c[2])).toEqual([null, "ref-1", null]);
    expect(noteCreateResult.mock.calls).toEqual([[sentFor("Dentist"), null, saved, WHERE]]);
  });

  it("More options after a token ends unproven hands its words to the full editor", async () => {
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(lost);
    const { onMore, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    expect(onMore).toHaveBeenCalledWith(
      expect.objectContaining({
        held: null,
        unproven: { sent: sentFor("Dentist"), outcome: lost },
      }),
    );
    // The full editor speaks for it now: no notice as the box goes.
    await leave(unmount);
    expect(noteCreateResult).not.toHaveBeenCalled();
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  // Check again out when the box goes with its tab: the answer, or its absence, keeps the token.
  it("a Check again answered once the box has gone passes its token on", async () => {
    const busy: WriteOutcome = { outcome: "busy" };
    const check = later();
    createEvent.mockResolvedValueOnce(ambiguous).mockReturnValueOnce(check.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await leave(unmount);
    // The save still out says how it ended, not the close.
    expect(noteHeldCreate).not.toHaveBeenCalled();
    await act(async () => check.answer(busy));
    expect(noteCreateResult.mock.calls).toEqual([[sentFor("Dentist"), "ref-1", busy, WHERE]]);
  });

  it("a Check again with no answer at all once the box has gone keeps Check again", async () => {
    const check = later();
    createEvent.mockResolvedValueOnce(ambiguous).mockReturnValueOnce(check.promise);
    const { unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await leave(unmount);
    await act(async () => check.fail(new Error("The app's channel closed.")));
    expect(noteHeldCreate.mock.calls).toEqual([[sentFor("Dentist"), "ref-1"]]);
    expect(noteCreateResult).not.toHaveBeenCalled();
  });

  // A first save out when the box goes with its tab, turned down before it was sent (this window,
  // the owner or the vault): no token to keep, so the notice says it wasn't created.
  it("a first save with no answer at all once the box has gone says it wasn't created", async () => {
    const answer = later();
    createEvent.mockReturnValue(answer.promise);
    const { unmount, onLeave } = open();
    // No title, as Enter in an empty box sends.
    fireEvent.keyDown(title(), { key: "Enter" });
    await leave(unmount);
    await act(async () => answer.fail(new Error("The vault is locked.")));
    expect(noteCreateResult.mock.calls).toEqual([
      [sentFor(""), null, { outcome: "failed", message: "The vault is locked." }, WHERE],
    ]);
    expect(noteHeldCreate).not.toHaveBeenCalled();
    expect(onLeave).not.toHaveBeenCalled();
  });

  it("a Check again with no answer at all keeps the draft locked, with Check again", async () => {
    createEvent
      .mockResolvedValueOnce(ambiguous)
      .mockRejectedValueOnce(new Error("The app's channel closed."));
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    expect(await screen.findByText("The app's channel closed.")).toBeTruthy();
    expect(title().disabled).toBe(true);
    expect(screen.getByRole("button", { name: "Check again" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("a held key's repeats send nothing more once an answer unlocks Save", async () => {
    createEvent.mockResolvedValue(unreachable);
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    fireEvent.keyDown(title(), { key: "Enter", repeat: true });
    await act(async () => {});
    expect(createEvent).toHaveBeenCalledTimes(1);
    // A held Enter on Save (where the answer puts focus) would click it on each repeat: the
    // repeat's default is stopped, a first press's isn't. jsdom clicks nothing for Enter itself.
    const save = screen.getByRole("button", { name: "Save" });
    expect(fireEvent.keyDown(save, { key: "Enter", repeat: true })).toBe(false);
    expect(fireEvent.keyDown(save, { key: "Enter" })).toBe(true);
  });

  // The line names the field, which is marked wrong and described by it, and a save it stops puts
  // focus there, where a date typed or picked ends it.
  it("a date it can't read is named, marked and focused by a save", async () => {
    open();
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: "99-99-2026" } });
    // Leaving the field commits it, and it's refused.
    act(() => title().focus());
    const line = screen.getByText(
      "Date: “99-99-2026” isn't a date PM can read. Type the date again as dd-mm-yyyy, or pick it from the calendar.",
    );
    expect(line.getAttribute("role")).toBe("alert");
    expect(date.getAttribute("aria-invalid")).toBe("true");
    expect(date.getAttribute("aria-describedby")).toBe(line.id);
    expect(line.id).not.toBe("");
    expect(document.activeElement).toBe(title());
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(date);
    // All day: the same field is the Day, and the line says so.
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(screen.getByText(/^Day: “99-99-2026” isn't a date PM can read/)).toBeTruthy();
    expect(screen.getByLabelText("Day").getAttribute("aria-invalid")).toBe("true");
    // Given a date, the line and the mark go.
    fireEvent.click(screen.getByLabelText("Day"));
    fireEvent.click(screen.getByRole("button", { name: /\b14-10(-2026)?$/ }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByLabelText("Day").getAttribute("aria-invalid")).toBeNull();
    expect(screen.getByLabelText("Day").getAttribute("aria-describedby")).toBeNull();
  });

  it("a save refused for a time the clocks skip keeps focus where it was", async () => {
    open();
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: "28-03-2027" } });
    fireEvent.blur(date);
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    expect(screen.getByText(/01:30 doesn't happen in Europe\/London that night/)).toBeTruthy();
    act(() => title().focus());
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
  });

  // The calendar list stays inside the box, as in the full editor's dialog: opening it puts the
  // keyboard on the calendar picked, Tab hands focus back to its button, and Escape there closes the
  // list, never the box (even an empty one, which Escape in the box itself would close).
  it("the calendar list is inside the box, and the keyboard gets in and out", async () => {
    const { onClose } = open();
    const trigger = screen.getByRole("button", { name: "Calendar: Personal" });
    trigger.focus();
    // Enter or Space on a button is its click, with no pointer behind it.
    fireEvent.click(trigger, { detail: 0 });
    const list = screen.getByRole("group", LIST);
    expect(list.parentElement).toBe(screen.getByRole("dialog", { name: "New event" }));
    const picked = within(list).getByRole("radio", { name: "Personal" });
    await waitFor(() => expect(document.activeElement).toBe(picked));
    fireEvent.keyDown(picked, { key: "Tab" });
    expect(screen.queryByRole("group", LIST)).toBeNull();
    expect(document.activeElement).toBe(trigger);
    fireEvent.click(trigger, { detail: 0 });
    const again = within(screen.getByRole("group", LIST)).getByRole("radio", { name: "Personal" });
    await waitFor(() => expect(document.activeElement).toBe(again));
    fireEvent.keyDown(again, { key: "Escape" });
    expect(screen.queryByRole("group", LIST)).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("a click on a calendar's name picks it and closes the list, the box staying", () => {
    const { onClose, onDraft } = open();
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    const list = screen.getByRole("group", LIST);
    const name = within(list).getByText("Work");
    fireEvent.pointerDown(name);
    fireEvent.mouseDown(name);
    // Its label forwards the click to the radio with `detail` 0, as an arrow key's would be.
    fireEvent.click(within(list).getByRole("radio", { name: "Work" }), { detail: 0 });
    expect(screen.queryByRole("group", LIST)).toBeNull();
    expect(onDraft.mock.lastCall![1]).toBe(WORK_CAL);
    expect(onClose).not.toHaveBeenCalled();
  });

  it("the calendar list closes for a save and stays closed after its answer", async () => {
    createEvent.mockResolvedValue(unreachable);
    open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    expect(screen.queryByRole("group", LIST)).not.toBeNull();
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    expect(title().disabled).toBe(false);
    expect(screen.queryByRole("group", LIST)).toBeNull();
  });

  // A notice that never goes (Check again) sits at the foot of the window, where the box can open
  // on a short one: the box stacks over the write notices, as a modal's scrim does.
  it("sits over the calendar's write notices", () => {
    open();
    render(
      <WriteNotices
        notices={[{ id: 1, tone: "warn", text: "PM may already have made “Gym”." }]}
        onUndo={vi.fn()}
        onDismiss={vi.fn()}
      />,
    );
    const stack = screen.getByText("PM may already have made “Gym”.").closest(".fixed");
    expect(zOf(box())).toBeGreaterThan(zOf(stack));
  });

  // The date field puts back the date it had when it can't read what was typed: that date was given
  // by nobody, so the refusal stands until the field is given one, typed or picked.
  it("a date it can't read stands through a blur, a time and All day, until the field is given one", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    const date = () => screen.getByLabelText<HTMLInputElement>(/^(Date|Day)$/);
    const refused = () => screen.queryByText(/^Date: “31-11-2026” isn't a date PM can read/);
    date().focus();
    fireEvent.change(date(), { target: { value: "31-11-2026" } });
    fireEvent.blur(date());
    expect(refused()).not.toBeNull();
    expect(date().value).toBe("12-10-2026");
    // Back in the field and out again with nothing typed: the date it put back is no date given.
    date().focus();
    fireEvent.blur(date());
    expect(refused()).not.toBeNull();
    // A time, and All day there and back, leave it standing.
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "10:00" } });
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    fireEvent.click(screen.getByRole("switch", { name: "All day" }));
    expect(refused()).not.toBeNull();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    // A date picked ends it, and it's the date sent.
    fireEvent.click(date());
    fireEvent.click(screen.getByRole("button", { name: /\b14-10(-2026)?$/ }));
    expect(refused()).toBeNull();
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls[0][1].time).toMatchObject({ start_date: "2026-10-14" });
  });

  // More options goes on to the full editor, where the date the field put back would otherwise be
  // saved without a word: the refusal goes with it, even for text typed and not yet left.
  it("More options carries a date it couldn't read on to the full editor", () => {
    const { onMore } = open();
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: "31-02-2026" } });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    expect(onMore).toHaveBeenCalledWith(
      expect.objectContaining({ draft: seed, badDate: "31-02-2026" }),
    );
  });

  // A click in the sidebar switches tabs (a titled box stays through a click), and the tab takes the
  // box with it: with nothing sent, the draft is handed back to open on again.
  it("a titled draft that goes with the tab is handed back once, as it was", async () => {
    const { onLeave, unmount } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "11:00" } });
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: "31-02-2026" } });
    fireEvent.blur(date);
    await act(async () => {});
    expect(onLeave).not.toHaveBeenCalled();
    await leave(unmount);
    expect(onLeave.mock.calls).toEqual([
      [
        {
          seed,
          draft: {
            ...seed,
            summary: "Dentist",
            time: { ...seed.time, start_time: "11:00", end_time: "12:00" },
          },
          calendarId: PERSONAL,
          timeTouched: true,
          meetTouched: false,
          badDate: "31-02-2026",
        },
      ],
    ]);
    expect(noteCreateResult).not.toHaveBeenCalled();
    expect(noteHeldCreate).not.toHaveBeenCalled();
  });

  // A box opened again on that draft holds a title from its first render. The app's development
  // build runs every new mount's effects as setup, cleanup, setup: that rehearsal of an unmount must
  // hand nothing back, or the draft would open yet again after the box was closed.
  it("under StrictMode, a box opened on a draft hands nothing back until it really goes", async () => {
    const draft: EditorFields = { ...seed, summary: "Dentist" };
    const { onLeave, unmount } = open({
      strict: true,
      resume: {
        seed,
        draft,
        calendarId: PERSONAL,
        timeTouched: false,
        meetTouched: false,
        badDate: null,
      },
    });
    await act(async () => {});
    expect(onLeave).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await leave(unmount);
    expect(onLeave).not.toHaveBeenCalled();
  });

  it("nothing is handed back for a box closed from inside, an empty one, or one handed over", async () => {
    // ✕ throws the draft away.
    let opened = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    await leave(opened.unmount);
    expect(opened.onLeave).not.toHaveBeenCalled();
    // Spaces alone aren't a title.
    opened = open();
    fireEvent.change(title(), { target: { value: "  " } });
    await leave(opened.unmount);
    expect(opened.onLeave).not.toHaveBeenCalled();
    // The full editor carries it on.
    opened = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    await leave(opened.unmount);
    expect(opened.onLeave).not.toHaveBeenCalled();
  });

  it("nothing is handed back for a draft sent: a notice or the answer speaks for it", async () => {
    // Held: Check again stays as a notice.
    createEvent.mockResolvedValueOnce(ambiguous);
    let opened = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByRole("button", { name: "Check again" });
    await leave(opened.unmount);
    expect(noteHeldCreate).toHaveBeenCalledTimes(1);
    expect(opened.onLeave).not.toHaveBeenCalled();
    // A save still out says how it ended.
    const answer = later();
    createEvent.mockReturnValueOnce(answer.promise);
    opened = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await leave(opened.unmount);
    await act(async () => answer.answer(saved));
    expect(opened.onLeave).not.toHaveBeenCalled();
    // A token that ended unproven leaves its words.
    createEvent.mockResolvedValueOnce(ambiguous).mockResolvedValueOnce(lost);
    opened = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    await leave(opened.unmount);
    expect(opened.onLeave).not.toHaveBeenCalled();
  });

  it("opens on a draft that went with the tab, its time and refusal as they were", async () => {
    const draft: EditorFields = {
      ...seed,
      summary: "Dentist",
      time: { ...seed.time, start_time: "11:00", end_time: "12:00" } as EditorFields["time"],
    };
    const { onDraft } = open({
      resume: {
        seed,
        draft,
        calendarId: PERSONAL,
        timeTouched: true,
        meetTouched: false,
        badDate: "31-02-2026",
      },
    });
    expect(title().value).toBe("Dentist");
    expect(document.activeElement).toBe(title());
    expect(onDraft).toHaveBeenLastCalledWith(draft, PERSONAL, "unsaved");
    expect(screen.getByText(/^Date: “31-02-2026” isn't a date PM can read/)).toBeTruthy();
    expect(screen.getByLabelText("Date").getAttribute("aria-invalid")).toBe("true");
    // The time was the user's: another calendar leaves it alone.
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    fireEvent.click(screen.getByRole("radio", { name: "Work" }), { detail: 1 });
    expect(onDraft.mock.lastCall![0].time).toEqual(draft.time);
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
  });

  // A click on the ghost, an event or "+ New event" comes back to the box. While it's locked, the
  // title can't take focus, so the keyboard lands on Check again, or the box itself while saving.
  it("brought back while locked, the keyboard lands in the box", async () => {
    const answer = later();
    createEvent.mockReturnValueOnce(answer.promise);
    const { focusRef } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    const synced = screen.getByRole("button", { name: "Synced" });
    synced.focus();
    act(() => focusRef.current!());
    expect(document.activeElement).toBe(box());
    await act(async () => answer.answer(ambiguous));
    const again = screen.getByRole("button", { name: "Check again" });
    await waitFor(() => expect(document.activeElement).toBe(again));
    synced.focus();
    act(() => focusRef.current!());
    expect(title().disabled).toBe(true);
    expect(document.activeElement).toBe(again);
  });

  // What the ghost is announced with: an event PM may already have made is never "not saved".
  it("tells the ghost how far the event has got", async () => {
    const answer = later();
    createEvent.mockReturnValueOnce(answer.promise).mockResolvedValueOnce(lost);
    const { onDraft } = open();
    const state = () => onDraft.mock.lastCall![2];
    fireEvent.change(title(), { target: { value: "Dentist" } });
    expect(state()).toBe("unsaved");
    fireEvent.keyDown(title(), { key: "Enter" });
    expect(state()).toBe("saving");
    await act(async () => answer.answer(ambiguous));
    expect(state()).toBe("unsure");
    // A token that ended unproven: it may still be in Google.
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await screen.findByRole("button", { name: "Save" });
    // As below: the ghost hears of it from an effect that may run after Save is on screen.
    await waitFor(() => expect(state()).toBe("unsure"));
  });

  it("a fresh save that fails outright leaves the ghost unsaved", async () => {
    createEvent.mockResolvedValue(unreachable);
    const { onDraft } = open();
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await screen.findByText("“Dentist” wasn't created: PM couldn't reach Google.");
    // The ghost hears of it from an effect that may run after the words are on screen.
    await waitFor(() => expect(onDraft.mock.lastCall![2]).toBe("unsaved"));
  });

  // --- repeating (C11) ---

  const repeat = () => screen.getByRole<HTMLButtonElement>("button", { name: /^Repeat: / });
  /** The rule the Repeat button names. */
  const shownRepeat = () =>
    repeat()
      .getAttribute("aria-label")
      ?.replace(/^Repeat: /, "");
  const REPEATS = { name: "Repeat" };

  /** Open the Repeat list, once the menu has its ready-made rules. */
  async function openRepeat() {
    await act(async () => {});
    fireEvent.click(repeat());
    return screen.getByRole("group", REPEATS);
  }

  /** Click the Repeat row whose words are `label`. */
  async function pickRepeat(label: string) {
    const list = await openRepeat();
    fireEvent.click(within(list).getByRole("radio", { name: label }), { detail: 1 });
    await act(async () => {});
  }

  it("a preset is sent with the create", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    await pickRepeat("Weekly on Monday");
    expect(shownRepeat()).toBe("Weekly on Monday");
    // It starts on the day picked, so nothing says it moved.
    expect(screen.queryByText(/the first day this repeat allows/)).toBeNull();
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Standup").draft, recurrence: weeklyOn("mon") }, null],
    ]);
  });

  it("a repeat the backend refuses stops every save, and says why", async () => {
    const GAP =
      "The first occurrence would be on 29-03-2027, when the clocks skip that time. Pick a time outside that hour.";
    describeRecurrence.mockResolvedValue({ outcome: "refused", message: GAP });
    open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    await pickRepeat("Weekly on Monday");
    expect(screen.getByText(GAP)).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
    title().focus();
    fireEvent.keyDown(title(), { key: "Enter" });
    await act(async () => {});
    expect(createEvent).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(title());
  });

  /** Give the Date field `text`, as typed and left. */
  function typeDate(text: string) {
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: text } });
    fireEvent.blur(date);
  }

  // The time line already refuses a start the clocks skip, and the backend refuses a first
  // occurrence on it in other words: said once.
  it("a start the clocks skip is refused once, not again under Repeat", async () => {
    const SKIPPED =
      "The first occurrence would be on 28-03-2027, when the clocks skip that time. Pick a time outside that hour.";
    describeRecurrence.mockResolvedValue({ outcome: "refused", message: SKIPPED });
    open();
    fireEvent.change(title(), { target: { value: "Swim" } });
    typeDate("28-03-2027");
    fireEvent.change(screen.getByLabelText("Start time"), { target: { value: "01:30" } });
    await pickRepeat("Daily");
    expect(screen.getAllByText(/the clocks skip/)).toHaveLength(1);
    expect(screen.getByText(/^01:30 doesn't happen in Europe\/London that night/)).toBeTruthy();
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Save" }).disabled).toBe(true);
  });

  // The backend lists and lands its occurrences inside the months PM syncs, wherever it starts.
  it("a repeat that starts before the months PM syncs says only its occurrences there show", async () => {
    open();
    fireEvent.change(title(), { target: { value: "Gym" } });
    typeDate("06-07-2026");
    await pickRepeat("Weekly on Monday");
    expect(
      screen.getByText(
        /^This starts before the months PM syncs \(.+\), so only its occurrences within them will show here\.$/,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/won't show here/)).toBeNull();
  });

  it("moving the date moves a one-day weekly rule", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    await pickRepeat("Weekly on Monday");
    const date = screen.getByLabelText<HTMLInputElement>("Date");
    date.focus();
    fireEvent.change(date, { target: { value: "13-10-2026" } });
    fireEvent.blur(date);
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Tuesday");
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls[0][1].recurrence).toEqual(weeklyOn("tue"));
  });

  // The Repeat list stays inside the box, as in the full editor's dialog, and Escape there closes
  // the list, never the box (even an empty one, which Escape in the box itself would close).
  it("the Repeat list is inside the box, and Escape closes only it", async () => {
    const { onClose } = open();
    const list = await openRepeat();
    expect(list.parentElement).toBe(screen.getByRole("dialog", { name: "New event" }));
    const picked = within(list).getByRole("radio", { name: "Does not repeat" });
    await waitFor(() => expect(document.activeElement).toBe(picked));
    fireEvent.keyDown(picked, { key: "Escape" });
    expect(screen.queryByRole("group", REPEATS)).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(repeat());
  });

  it("Custom… reached by the keyboard hands over on Enter, and an arrow key never does", async () => {
    const { onMore } = open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    const list = await openRepeat();
    const picked = within(list).getByRole("radio", { name: "Does not repeat" });
    await waitFor(() => expect(document.activeElement).toBe(picked));
    // The arrow keys go round the rules only: up from the first is the last.
    fireEvent.keyDown(picked, { key: "ArrowUp" });
    const last = within(list).getByRole("radio", { name: "Weekly on Monday" });
    last.focus();
    fireEvent.click(last, { detail: 0 });
    await act(async () => {});
    expect(shownRepeat()).toBe("Weekly on Monday");
    expect(onMore).not.toHaveBeenCalled();
    // Tab reaches Custom…, and Enter (its click) hands over.
    fireEvent.keyDown(last, { key: "Tab" });
    const custom = within(list).getByRole("button", { name: "Custom…" });
    expect(document.activeElement).toBe(custom);
    expect(fireEvent.keyDown(custom, { key: "Enter" })).toBe(true);
    fireEvent.click(custom, { detail: 0 });
    expect(onMore).toHaveBeenCalledTimes(1);
    expect(onMore.mock.lastCall![0]).toMatchObject({
      draft: { recurrence: { kind: "rule", rule: weeklyOn("mon") } },
      openCustom: true,
    });
    expect(createEvent).not.toHaveBeenCalled();
  });

  it("Custom… hands over to the full editor with the custom dialog open, sending nothing", async () => {
    const { onMore, onClose } = open();
    fireEvent.change(title(), { target: { value: "Standup" } });
    await pickRepeat("Weekly on Monday");
    const list = await openRepeat();
    fireEvent.click(within(list).getByRole("button", { name: "Custom…" }), { detail: 1 });
    expect(createEvent).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    expect(onMore).toHaveBeenCalledWith({
      draft: { ...seed, summary: "Standup", recurrence: { kind: "rule", rule: weeklyOn("mon") } },
      calendarId: PERSONAL,
      held: null,
      unproven: null,
      showAsTouched: false,
      timeTouched: false,
      meetTouched: false,
      badDate: null,
      openCustom: true,
    });
  });
});

// --- a video call on new events (C18b) ---

/** Personal offers Google Meet and its account adds Meet to the events it creates; Work offers
 *  none. */
const meetOverview: CalendarOverview = {
  ...overview,
  calendars: overview.calendars.map((c) =>
    c.id === PERSONAL ? { ...c, conference_types: ["hangoutsMeet"] } : c,
  ),
  auto_meet: { [ME]: true },
};

const meetButton = () => screen.queryByRole("button", { name: "Google Meet" });

describe("QuickCreatePopover video call (C18b)", () => {
  // Quick create has no guests, so the setting alone turns nothing on (critique C2-H2).
  it("the Meet button follows the setting and the calendar", async () => {
    createEvent.mockResolvedValue(saved);
    const { onClose } = open({ overview: meetOverview });
    expect(meetButton()!.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(meetButton()!, { detail: 1 });
    expect(meetButton()!.getAttribute("aria-pressed")).toBe("true");
    // Work offers no Meet: the button goes, the call is off, and the box says so.
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Personal" }));
    fireEvent.click(screen.getByRole("radio", { name: "Work" }), { detail: 1 });
    expect(meetButton()).toBeNull();
    expect(
      screen.getByText("Work doesn't offer Google Meet, so the video call is off."),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Calendar: Work" }));
    fireEvent.click(screen.getByRole("radio", { name: "Personal" }), { detail: 1 });
    expect(meetButton()!.getAttribute("aria-pressed")).toBe("false");
    expect(screen.queryByText(/doesn't offer Google Meet/)).toBeNull();
    // Pressed again and saved: the create asks for the call.
    fireEvent.click(meetButton()!, { detail: 1 });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls).toEqual([
      [PERSONAL, { ...sentFor("Dentist").draft, meet: true }, null],
    ]);
  });

  // Critique C2-M3: a click hands focus back to the title, so Enter saves as it does there; a key
  // press leaves focus on the button, whose Enter presses it.
  it("a click on Google Meet hands focus back to the title", () => {
    open({ overview: meetOverview });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    meetButton()!.focus();
    fireEvent.click(meetButton()!, { detail: 1 });
    expect(document.activeElement).toBe(title());
    meetButton()!.focus();
    fireEvent.click(meetButton()!, { detail: 0 });
    expect(document.activeElement).toBe(meetButton());
    expect(meetButton()!.getAttribute("aria-pressed")).toBe("false");
  });

  it("More options carries the call, and that it was pressed", () => {
    const { onMore } = open({ overview: meetOverview });
    fireEvent.click(meetButton()!, { detail: 1 });
    fireEvent.click(screen.getByRole("button", { name: "More options" }));
    expect(onMore).toHaveBeenCalledWith(
      expect.objectContaining({ draft: { ...seed, meet: true }, meetTouched: true }),
    );
  });

  it("is absent where the calendar offers no Meet", () => {
    open();
    expect(meetButton()).toBeNull();
  });

  it("a draft that goes with the tab keeps the call, and whether it was pressed", async () => {
    const { onLeave, unmount } = open({ overview: meetOverview });
    fireEvent.change(title(), { target: { value: "Dentist" } });
    fireEvent.click(meetButton()!, { detail: 1 });
    await leave(unmount);
    expect(onLeave).toHaveBeenCalledWith(
      expect.objectContaining({
        draft: expect.objectContaining({ meet: true }) as unknown,
        meetTouched: true,
      }),
    );
  });

  // The view may open it again on another calendar than the one it left: one without Meet asks for
  // no call, which the backend would refuse.
  it("a draft back on a calendar without Meet asks for no call", async () => {
    createEvent.mockResolvedValue(saved);
    const draft: EditorFields = { ...seed, summary: "Dentist", meet: true };
    const { onClose } = open({
      overview: meetOverview,
      calendarId: WORK_CAL,
      resume: {
        seed,
        draft,
        calendarId: WORK_CAL,
        timeTouched: true,
        meetTouched: true,
        badDate: null,
      },
    });
    fireEvent.keyDown(title(), { key: "Enter" });
    await waitFor(() => expect(onClose).toHaveBeenCalledWith(true));
    expect(createEvent.mock.calls[0][1].meet).toBe(false);
  });
});
