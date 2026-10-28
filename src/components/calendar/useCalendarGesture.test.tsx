// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The drag on the calendar grid (#884, C16b), over a stand-in grid with measured boxes (jsdom lays
// nothing out, so each element's box is set by hand, and pointer capture is stubbed): the pointer is
// held on the calendar's box, never a card; the press never prevents the default, so the details
// still close; a drop is made from the row as pressed and sent once, StrictMode included; every way
// out but a release on a valid place puts it back; and the keys, clicks, selection and scrolling it
// touches are only the ones it should.

import { StrictMode, useRef } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import type { CalendarEvent } from "../../lib/types";
import { dragCapability, NO_END, type DragCapability } from "../../lib/calendarDrag";
import { PUT_BACK } from "../../lib/calendarGesture";

const env = vi.hoisted(() => ({ dialog: false }));
vi.mock("../../lib/useDialogLayer", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  isAnyDialogOpen: () => env.dialog,
}));

import {
  sayAgain,
  useCalendarGesture,
  type CalendarGesture,
  type GestureHost,
} from "./useCalendarGesture";

const D1 = "2026-10-12";
const D2 = "2026-10-13";

const row = (over: Partial<CalendarEvent>): CalendarEvent => ({
  id: "e1",
  calendar_id: "c1",
  summary: "Gym",
  description: null,
  location: null,
  start: `${D1}T09:00:00`,
  end: `${D1}T10:00:00`,
  all_day: false,
  html_link: null,
  uid: null,
  ...over,
});
const GYM = row({});
const HOLIDAY = row({ id: "a1", summary: "Holiday", start: D1, end: D2, all_day: true });

const canMove = (ev: CalendarEvent): DragCapability =>
  dragCapability(ev, {
    rights: { canRetime: true, reason: null },
    pending: false,
    zoneKnown: true,
  });

interface Spies {
  onCommit: Mock<GestureHost["onCommit"]>;
  onCreate: Mock<GestureHost["onCreate"]>;
  announce: Mock<GestureHost["announce"]>;
}

/** A Day/Week grid as the views mark it: the strip with an all-day bar, the hours (40 px an hour)
 *  with two day columns and a 09:00-10:00 card in the first, the hour gutter, and the add-zone
 *  button. */
function Grid({
  host,
  rows,
  onView,
}: {
  host: Partial<GestureHost>;
  rows: Map<string, CalendarEvent>;
  onView?: (v: CalendarGesture) => void;
}) {
  const wrapper = useRef<HTMLDivElement>(null);
  const view = useCalendarGesture({
    wrapper: () => wrapper.current,
    surfaceOk: () => true,
    rowById: (id) => rows.get(id),
    capability: canMove,
    canCreate: () => false,
    overlayOpen: () => false,
    lengthFor: () => 60,
    zone: () => "Europe/London",
    onCommit: () => {},
    onCreate: () => {},
    announce: () => {},
    ...host,
  });
  onView?.(view);
  return (
    <div ref={wrapper} data-testid="wrapper">
      <button type="button" data-testid="add-zone">
        +
      </button>
      <div data-surface="band" data-days={`${D1},${D2}`} data-testid="band">
        <div data-event-id="a1" data-testid="bar" />
      </div>
      <div data-time-body="" data-row-h="40" data-testid="body">
        <div data-testid="gutter" />
        <div data-surface="grid" data-day={D1} data-testid="col1">
          <div data-event-id="e1" data-day={D1} data-testid="card">
            <div data-edge="start" />
            <div data-edge="end" />
          </div>
        </div>
        <div data-surface="grid" data-day={D2} data-testid="col2" />
      </div>
      {view.chip && (
        <div ref={view.chipRef} data-testid="chip">
          {view.chip}
        </div>
      )}
      {view.ghost && <div data-testid="ghost">{view.ghost.label}</div>}
    </div>
  );
}

function place(el: Element, left: number, top: number, right: number, bottom: number) {
  (el as HTMLElement).getBoundingClientRect = () =>
    ({
      left,
      top,
      right,
      bottom,
      x: left,
      y: top,
      width: right - left,
      height: bottom - top,
      toJSON: () => ({}),
    }) as DOMRect;
}

/** Lay the grid out: hours from y 100 (09:00 at y 460), the columns side by side from x 54. */
function layOut() {
  place(screen.getByTestId("wrapper"), 0, 0, 800, 600);
  place(screen.getByTestId("band"), 54, 60, 800, 80);
  place(screen.getByTestId("bar"), 54, 60, 427, 78);
  place(screen.getByTestId("body"), 0, 100, 800, 600);
  place(screen.getByTestId("gutter"), 0, 100, 54, 600);
  place(screen.getByTestId("col1"), 54, 100, 427, 600);
  place(screen.getByTestId("col2"), 427, 100, 800, 600);
  place(screen.getByTestId("card"), 55, 460, 426, 500);
}

function mount(host: Partial<GestureHost> = {}, opts: { strict?: boolean } = {}) {
  const spies: Spies = {
    onCommit: vi.fn<GestureHost["onCommit"]>(),
    onCreate: vi.fn<GestureHost["onCreate"]>(),
    announce: vi.fn<GestureHost["announce"]>(),
  };
  const rows = new Map([
    [GYM.id, GYM],
    [HOLIDAY.id, HOLIDAY],
  ]);
  const views: CalendarGesture[] = [];
  const grid = <Grid host={{ ...spies, ...host }} rows={rows} onView={(v) => views.push(v)} />;
  const utils = render(opts.strict ? <StrictMode>{grid}</StrictMode> : grid);
  layOut();
  return { ...utils, ...spies, rows, views };
}

const at = (x: number, y: number, more: object = {}) => ({
  pointerId: 1,
  pointerType: "mouse",
  button: 0,
  buttons: 1,
  clientX: x,
  clientY: y,
  ...more,
});
const frame = () =>
  act(() => {
    vi.advanceTimersByTime(20);
  });
function press(target: Element, x = 200, y = 480, more: object = {}) {
  return fireEvent.pointerDown(target, at(x, y, more));
}
function moveTo(x: number, y: number) {
  fireEvent.pointerMove(document.body, at(x, y));
  frame();
}
function release(x: number, y: number) {
  fireEvent.pointerUp(document.body, at(x, y, { buttons: 0 }));
}
/** Press Gym at 09:30 and drag it to 10:30 on the next day. */
function dragGymOver() {
  press(screen.getByTestId("card"));
  moveTo(600, 520);
}

let capture: ReturnType<typeof vi.fn>;
let releaseCapture: ReturnType<typeof vi.fn>;
let held = false;

beforeEach(() => {
  env.dialog = false;
  vi.useFakeTimers({
    toFake: ["setTimeout", "clearTimeout", "requestAnimationFrame", "cancelAnimationFrame"],
  });
  held = false;
  capture = vi.fn(() => {
    held = true;
  });
  releaseCapture = vi.fn(() => {
    held = false;
  });
  Object.assign(HTMLElement.prototype, {
    setPointerCapture: capture,
    releasePointerCapture: releaseCapture,
    hasPointerCapture: () => held,
  });
});
afterEach(() => {
  cleanup();
  // A click swallow lapses on the next turn: let it, so it can't eat the next test's first click.
  vi.runOnlyPendingTimers();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("useCalendarGesture: a drop", () => {
  it("moves Gym where it's let go, from the row as pressed", () => {
    const { onCommit, announce } = mount();
    dragGymOver();
    expect(announce).toHaveBeenCalledWith("Moving “Gym”.");
    expect(screen.getByTestId("ghost").textContent).toMatch(/10:00.11:00/);
    release(600, 520);
    expect(onCommit).toHaveBeenCalledTimes(1);
    const commit = onCommit.mock.calls[0][0];
    expect(commit.row).toBe(GYM);
    expect(commit.from).toEqual({
      kind: "timed",
      start: { day: D1, min: 540 },
      end: { day: D1, min: 600 },
    });
    expect(commit.to).toEqual({
      kind: "timed",
      start: { day: D2, min: 600 },
      end: { day: D2, min: 660 },
    });
    expect(commit.resize).toBe(false);
    expect(screen.queryByTestId("ghost")).toBeNull();
  });

  it("capture goes on the wrapper, never the card, and is let go after", () => {
    mount();
    dragGymOver();
    expect(capture).toHaveBeenCalledTimes(1);
    expect(capture.mock.contexts[0]).toBe(screen.getByTestId("wrapper"));
    expect(document.body.style.cursor).toBe("grabbing");
    release(600, 520);
    expect(releaseCapture).toHaveBeenCalledTimes(1);
    expect(document.body.style.cursor).toBe("");
  });

  it("pointerdown is never default-prevented", () => {
    mount();
    const notPrevented = press(screen.getByTestId("card"));
    expect(notPrevented).toBe(true);
  });

  it("resizes from the bottom edge, in its own column", () => {
    const { onCommit } = mount();
    // The bottom 6 px of the 40 px card are its edge.
    press(screen.getByTestId("card"), 200, 497);
    moveTo(600, 540);
    release(600, 540);
    const commit = onCommit.mock.calls[0][0];
    expect(commit.resize).toBe(true);
    expect(commit.to).toEqual({
      kind: "timed",
      start: { day: D1, min: 540 },
      end: { day: D1, min: 660 },
    });
  });

  it("a row re-read mid-drag doesn't change what was seen", () => {
    const { onCommit, rows } = mount();
    dragGymOver();
    rows.set("e1", { ...GYM, summary: "Gym (renamed in Google)" });
    release(600, 520);
    expect(onCommit.mock.calls[0][0].row).toBe(GYM);
  });

  it("a row gone mid-drag puts it back with no commit", () => {
    const { onCommit, announce, rows } = mount();
    dragGymOver();
    rows.delete("e1");
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
  });

  it("a column re-keyed mid-drag: the drop lands on the live column", () => {
    const { onCommit } = mount();
    dragGymOver();
    // A swipe moved the days on: the column under the pointer is now the 14th.
    screen.getByTestId("col2").setAttribute("data-day", "2026-10-14");
    moveTo(600, 521);
    release(600, 521);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: "2026-10-14", min: 600 },
      end: { day: "2026-10-14", min: 660 },
    });
  });

  it("an edge the card doesn't draw isn't taken: a press there moves it", () => {
    const { onCommit } = mount();
    screen.getByTestId("card").querySelector('[data-edge="end"]')?.remove();
    press(screen.getByTestId("card"), 200, 497);
    moveTo(600, 537);
    release(600, 537);
    expect(onCommit.mock.calls[0][0].resize).toBe(false);
  });

  it("only its own pointer moves or lets go of it", () => {
    const { onCommit } = mount();
    dragGymOver();
    fireEvent.pointerUp(document.body, at(600, 520, { pointerId: 2, buttons: 0 }));
    expect(onCommit).not.toHaveBeenCalled();
    release(600, 520);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("a capture lost before the drag began, which was never its own, changes nothing", () => {
    const { onCommit } = mount();
    press(screen.getByTestId("card"));
    fireEvent(screen.getByTestId("wrapper"), new Event("lostpointercapture"));
    moveTo(600, 520);
    release(600, 520);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("one drop sends once under StrictMode, and a capture lost after the release changes nothing", () => {
    const { onCommit, announce } = mount({}, { strict: true });
    dragGymOver();
    release(600, 520);
    fireEvent(screen.getByTestId("wrapper"), new Event("lostpointercapture"));
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(announce).not.toHaveBeenCalledWith(PUT_BACK);
  });
});

describe("useCalendarGesture: putting it back", () => {
  it("Escape mid-drag puts it back and never reaches a window bubble listener", () => {
    const { onCommit, announce } = mount();
    const bubble = vi.fn();
    window.addEventListener("keydown", bubble);
    dragGymOver();
    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(bubble).not.toHaveBeenCalled();
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    window.removeEventListener("keydown", bubble);
  });

  it("after Escape, letting go over the event opens nothing", () => {
    mount();
    const click = vi.fn();
    screen.getByTestId("card").addEventListener("click", click);
    dragGymOver();
    fireEvent.keyDown(document.body, { key: "Escape" });
    // The button is held a while before it's let go: the swallow waits for that release.
    frame();
    release(200, 480);
    fireEvent.click(screen.getByTestId("card"));
    expect(click).not.toHaveBeenCalled();
    // Only that one: the next click is the user's.
    fireEvent.click(screen.getByTestId("card"));
    expect(click).toHaveBeenCalledTimes(1);
  });

  it("after a drag the next click is swallowed exactly once", () => {
    mount();
    const click = vi.fn();
    screen.getByTestId("card").addEventListener("click", click);
    dragGymOver();
    release(600, 520);
    fireEvent.click(screen.getByTestId("card"));
    expect(click).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("card"));
    expect(click).toHaveBeenCalledTimes(1);
  });

  it("a click with no drag reaches the event", () => {
    const { onCommit } = mount();
    const click = vi.fn();
    screen.getByTestId("card").addEventListener("click", click);
    press(screen.getByTestId("card"));
    // A wobble under the threshold.
    moveTo(202, 481);
    release(202, 481);
    fireEvent.click(screen.getByTestId("card"));
    expect(click).toHaveBeenCalledTimes(1);
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("Ctrl+Z mid-drag is swallowed; arrow keys still reach the window", () => {
    mount();
    const bubble = vi.fn();
    window.addEventListener("keydown", bubble);
    dragGymOver();
    const z = fireEvent.keyDown(document.body, { key: "z", ctrlKey: true });
    expect(z).toBe(false);
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    expect(bubble.mock.calls.map(([e]) => (e as KeyboardEvent).key)).toEqual(["ArrowRight"]);
    window.removeEventListener("keydown", bubble);
  });

  it("pointercancel, blur, a hidden document and a lost capture put it back", () => {
    const ways: [string, () => void][] = [
      ["pointercancel", () => fireEvent.pointerCancel(document.body, at(600, 520))],
      ["blur", () => fireEvent.blur(window)],
      [
        "hidden",
        () => {
          vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
          fireEvent(document, new Event("visibilitychange"));
        },
      ],
      [
        "lost capture",
        () => fireEvent(screen.getByTestId("wrapper"), new Event("lostpointercapture")),
      ],
    ];
    for (const [why, cancel] of ways) {
      const { onCommit, announce, unmount } = mount();
      dragGymOver();
      cancel();
      release(600, 520);
      expect(onCommit, why).not.toHaveBeenCalled();
      expect(announce, why).toHaveBeenLastCalledWith(PUT_BACK);
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("a pointer that can't be held puts it back", () => {
    capture.mockImplementation(() => {
      throw new Error("no capture here");
    });
    const { onCommit, announce } = mount();
    dragGymOver();
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
    expect(screen.queryByTestId("ghost")).toBeNull();
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(document.body.style.cursor).toBe("");
  });

  it("the same pointer pressed again (its release was lost) puts it back", () => {
    const { onCommit, announce } = mount();
    dragGymOver();
    press(screen.getByTestId("card"), 600, 520);
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("after Escape, a new press before any release means that release was lost: its click goes ahead", () => {
    mount();
    const click = vi.fn();
    screen.getByTestId("card").addEventListener("click", click);
    dragGymOver();
    fireEvent.keyDown(document.body, { key: "Escape" });
    press(screen.getByTestId("card"));
    release(200, 480);
    fireEvent.click(screen.getByTestId("card"));
    expect(click).toHaveBeenCalledTimes(1);
  });

  it("a dialog opening mid-drag puts it back", () => {
    const { onCommit } = mount();
    dragGymOver();
    env.dialog = true;
    frame();
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("letting go above the calendar puts it back; below it doesn't", () => {
    const { onCommit } = mount();
    dragGymOver();
    release(600, -5);
    expect(onCommit).not.toHaveBeenCalled();
    dragGymOver();
    moveTo(600, 640);
    release(600, 640);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("every listener goes on unmount mid-drag", () => {
    const { onCommit, announce, unmount } = mount();
    const added = vi.spyOn(window, "addEventListener");
    const removed = vi.spyOn(window, "removeEventListener");
    dragGymOver();
    unmount();
    const count = (spy: typeof added) => spy.mock.calls.filter(([type]) => type !== "click").length;
    // Each listener the press put on the window came off, and the press listener itself.
    expect(count(removed)).toBeGreaterThanOrEqual(count(added) + 1);
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
  });
});

describe("useCalendarGesture: presses it leaves alone", () => {
  it("touch is ignored; a press under a dialog is ignored", () => {
    const { onCommit } = mount();
    // Neither is even taken: selecting text still works.
    const selects = () =>
      fireEvent(
        screen.getByTestId("card"),
        new Event("selectstart", { bubbles: true, cancelable: true }),
      );
    press(screen.getByTestId("card"), 200, 480, { pointerType: "touch" });
    expect(selects()).toBe(true);
    moveTo(600, 520);
    fireEvent.pointerUp(document.body, at(600, 520, { buttons: 0, pointerType: "touch" }));
    env.dialog = true;
    press(screen.getByTestId("card"));
    expect(selects()).toBe(true);
    moveTo(600, 520);
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a press with Shift, Ctrl, Alt or Meta is ignored, and so is any other button", () => {
    const { onCommit } = mount();
    for (const mod of [
      { shiftKey: true },
      { ctrlKey: true },
      { altKey: true },
      { metaKey: true },
      { button: 2 },
    ]) {
      press(screen.getByTestId("card"), 200, 480, mod);
      moveTo(600, 520);
      release(600, 520);
    }
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("a view that doesn't drag takes nothing", () => {
    const { onCommit } = mount({ surfaceOk: () => false });
    dragGymOver();
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a press outside the calendar's box is not its own, even over the grid", () => {
    const { onCommit } = mount();
    // Something drawn over the grid from elsewhere in the page, marked like the grid.
    const over = document.createElement("div");
    over.dataset.surface = "grid";
    over.dataset.day = D1;
    const card = document.createElement("div");
    card.dataset.eventId = "e1";
    card.dataset.day = D1;
    over.appendChild(card);
    document.body.appendChild(over);
    place(over, 54, 100, 427, 600);
    place(card, 55, 460, 426, 500);
    press(card);
    moveTo(600, 520);
    release(600, 520);
    over.remove();
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("an event that isn't in the hours or the all-day row isn't taken", () => {
    const { onCommit } = mount();
    const stray = document.createElement("div");
    stray.dataset.eventId = "e1";
    screen.getByTestId("wrapper").appendChild(stray);
    place(stray, 55, 460, 426, 500);
    press(stray);
    moveTo(600, 520);
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a press on the hour gutter or the add-zone control does nothing", () => {
    const { announce } = mount();
    press(screen.getByTestId("gutter"), 20, 480);
    moveTo(600, 520);
    release(600, 520);
    press(screen.getByTestId("add-zone"), 20, 30);
    moveTo(600, 520);
    release(600, 520);
    expect(capture).not.toHaveBeenCalled();
    expect(announce).not.toHaveBeenCalled();
  });

  it("a press on empty time takes nothing while creating there isn't on", () => {
    const { onCreate } = mount();
    press(screen.getByTestId("col2"), 600, 300);
    release(600, 300);
    press(screen.getByTestId("col2"), 600, 300);
    moveTo(600, 400);
    release(600, 400);
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a press while the details are open only closes them", () => {
    const { onCommit } = mount({ overlayOpen: () => true });
    dragGymOver();
    release(600, 520);
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a read-only event shows its reason by the pointer and sends nothing", () => {
    const reason = "Someone else organises this event.";
    const { onCommit, announce } = mount({
      capability: (ev) =>
        dragCapability(ev, {
          rights: { canRetime: false, reason },
          pending: false,
          zoneKnown: true,
        }),
    });
    dragGymOver();
    const chip = screen.getByTestId("chip");
    expect(chip.textContent).toBe(reason);
    expect(chip.style.left).toBe("612px");
    expect(screen.queryByTestId("ghost")).toBeNull();
    expect(announce).toHaveBeenCalledWith(reason);
    expect(document.body.style.cursor).toBe("not-allowed");
    // It follows the pointer; and a drag that can change nothing never scrolls the hours.
    moveTo(650, 595);
    expect(chip.style.left).toBe("662px");
    expect(chip.style.top).toBe("607px");
    expect(screen.getByTestId("body").scrollTop).toBe(0);
    release(650, 595);
    expect(onCommit).not.toHaveBeenCalled();
    expect(screen.queryByTestId("chip")).toBeNull();
  });

  it("an event with no end says so by the pointer", () => {
    const { rows } = mount();
    rows.set("e1", { ...GYM, end: null });
    dragGymOver();
    expect(screen.getByTestId("chip").textContent).toBe(NO_END);
  });

  it("selection and native drags are stopped from the press, and only then", () => {
    mount();
    const card = screen.getByTestId("card");
    const starts = (type: string) =>
      fireEvent(card, new Event(type, { bubbles: true, cancelable: true }));
    press(card);
    // Before the threshold: a selection starts at the press.
    expect(starts("selectstart")).toBe(false);
    expect(starts("dragstart")).toBe(false);
    release(200, 480);
    expect(starts("selectstart")).toBe(true);
    expect(starts("dragstart")).toBe(true);
  });
});

describe("useCalendarGesture: scrolling the hours", () => {
  it("scrolls while dragging near the bottom, never while only pressed or over the all-day row", () => {
    mount();
    const body = screen.getByTestId("body");
    // Held at the bottom edge without moving past the threshold: no scroll.
    press(screen.getByTestId("card"), 200, 590);
    frame();
    frame();
    expect(body.scrollTop).toBe(0);
    release(200, 590);
    // Dragged over the all-day row: no scroll.
    press(screen.getByTestId("card"));
    moveTo(200, 70);
    frame();
    expect(body.scrollTop).toBe(0);
    // Dragged to the bottom edge: it scrolls, frame after frame.
    moveTo(200, 595);
    const first = body.scrollTop;
    expect(first).toBeGreaterThan(0);
    frame();
    expect(body.scrollTop).toBeGreaterThan(first);
    release(200, 595);
  });
});

describe("sayAgain", () => {
  it("makes the same words differ each time, so they're said again", () => {
    const once = sayAgain("", PUT_BACK);
    expect(once).toBe(PUT_BACK);
    const twice = sayAgain(once, PUT_BACK);
    expect(twice).not.toBe(once);
    expect(twice.trim()).toBe(PUT_BACK);
    expect(sayAgain(twice, PUT_BACK)).toBe(PUT_BACK);
    expect(sayAgain(PUT_BACK, "Moving “Gym”.")).toBe("Moving “Gym”.");
  });
});

describe("useCalendarGesture: the all-day row", () => {
  it("moves an all-day event along the row by its body", () => {
    const { onCommit } = mount();
    press(screen.getByTestId("bar"), 200, 70);
    moveTo(600, 70);
    release(600, 70);
    expect(onCommit.mock.calls[0][0].to).toEqual({ kind: "all_day", first: D2, last: D2 });
  });

  it("an all-day event dropped in the hours takes its account's length", () => {
    const { onCommit } = mount({ lengthFor: (ev) => (ev?.id === "a1" ? 30 : 60) });
    press(screen.getByTestId("bar"), 200, 70);
    moveTo(600, 480);
    release(600, 480);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D2, min: 570 },
      end: { day: D2, min: 600 },
    });
  });
});
