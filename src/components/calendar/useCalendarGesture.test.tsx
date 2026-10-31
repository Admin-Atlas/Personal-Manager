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
import { formatClock } from "../../lib/format";

const env = vi.hoisted(() => ({ dialog: false }));
vi.mock("../../lib/useDialogLayer", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  isAnyDialogOpen: () => env.dialog,
}));

import {
  overlayShown,
  sayAgain,
  useCalendarGesture,
  type CalendarGesture,
  type GestureHost,
} from "./useCalendarGesture";

const D1 = "2026-10-12";
const D2 = "2026-10-13";
// The ghost's times follow the machine's locale (CI's is 12-hour), so they're built the same way.
const clock = (h: number) => formatClock(new Date(2026, 9, 13, h));

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
    rights: { canEdit: true, canRetime: true, reason: null },
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
    createIn: () => null,
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
        <div data-event-id="a1" data-testid="bar">
          <div data-edge="right" data-testid="bar-end" />
        </div>
      </div>
      <div data-time-body="" data-row-h="40" data-testid="body">
        <div data-testid="gutter" />
        <div data-surface="grid" data-day={D1} data-testid="col1">
          <div data-event-id="e1" data-day={D1} data-testid="card">
            <div data-edge="start" data-testid="card-start" />
            <div data-edge="end" data-testid="card-end" />
          </div>
        </div>
        <div data-surface="grid" data-day={D2} data-testid="col2" />
      </div>
      {view.chip && (
        <div ref={view.chipRef} data-testid="chip">
          {view.chip}
        </div>
      )}
      {view.ghost && (
        <div
          data-testid="ghost"
          data-kind={view.ghost.kind}
          data-calendar={view.ghost.event.calendar_id}
        >
          {view.ghost.label}
        </div>
      )}
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
  // Its strips, as EventCard draws them inside its 1 px border, its stripe and its corners: the
  // top 6 px and the bottom 10 px of the padding box.
  place(screen.getByTestId("card-start"), 58, 461, 425, 467);
  place(screen.getByTestId("card-end"), 58, 489, 425, 499);
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
    expect(screen.getByTestId("ghost").textContent).toContain(`${clock(10)}–${clock(11)}`);
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

  it("resizes from the bottom edge, in its own column, by how far it's dragged", () => {
    const { onCommit } = mount();
    // Its strip taken 8 px up from the bottom (09:48), and dragged down an hour.
    press(screen.getByTestId("card-end"), 200, 492);
    moveTo(600, 532);
    release(600, 532);
    const commit = onCommit.mock.calls[0][0];
    expect(commit.resize).toBe(true);
    expect(commit.to).toEqual({
      kind: "timed",
      start: { day: D1, min: 540 },
      end: { day: D1, min: 660 },
    });
  });

  it("taking an edge changes nothing until it's dragged: a sideways slip lets go as it was", () => {
    const { onCommit } = mount();
    press(screen.getByTestId("card-end"), 200, 492);
    moveTo(212, 494);
    expect(screen.getByTestId("ghost").dataset.kind).toBe("resize");
    expect(screen.getByTestId("ghost").textContent).toContain(`${clock(9)}–${clock(10)}`);
    release(212, 494);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("the card's top strip takes its start; the card's own box between its strips moves it", () => {
    const { onCommit } = mount();
    press(screen.getByTestId("card-start"), 200, 462);
    moveTo(200, 422);
    release(200, 422);
    expect(onCommit.mock.calls[0][0].resize).toBe(true);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D1, min: 480 },
      end: { day: D1, min: 600 },
    });
    onCommit.mockClear();
    // A press on the card's own box between its strips is a move.
    press(screen.getByTestId("card"), 200, 480);
    moveTo(200, 520);
    release(200, 520);
    expect(onCommit.mock.calls[0][0].resize).toBe(false);
  });

  it("the card's own border level with a strip (the line drawn as its edge) takes that edge", () => {
    const { onCommit } = mount();
    const card = screen.getByTestId("card");
    // On the line drawn as its bottom, below the strip the card clips inside its border.
    press(card, 200, 499.5);
    moveTo(200, 539.5);
    release(200, 539.5);
    expect(onCommit.mock.calls[0][0].resize).toBe(true);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D1, min: 540 },
      end: { day: D1, min: 660 },
    });
    onCommit.mockClear();
    // And its top border the start.
    press(card, 200, 460.5);
    moveTo(200, 420.5);
    release(200, 420.5);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D1, min: 480 },
      end: { day: D1, min: 600 },
    });
    onCommit.mockClear();
    // On the calendar's stripe down its left side, level with the end's strip, and in its
    // bottom-left corner, outside the strip's rounded clip.
    for (const [x, y] of [
      [56, 494],
      [59, 497],
    ]) {
      press(card, x, y);
      moveTo(x, y + 40);
      release(x, y + 40);
      expect(onCommit.mock.calls[0][0].resize).toBe(true);
      onCommit.mockClear();
    }
    // A border with no strip beside it (an end that may not move) is the body's.
    card.querySelector('[data-edge="end"]')?.remove();
    press(card, 200, 499.5);
    moveTo(200, 539.5);
    release(200, 539.5);
    expect(onCommit.mock.calls[0][0].resize).toBe(false);
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

  it("letting go above the calendar puts it back; below it drops at the hours' bottom edge", () => {
    const { onCommit } = mount();
    // The hours don't scroll here, so their bottom edge (y 600) stays 12:30.
    Object.defineProperty(screen.getByTestId("body"), "scrollTop", { get: () => 0, set: () => {} });
    dragGymOver();
    release(600, -5);
    expect(onCommit).not.toHaveBeenCalled();
    dragGymOver();
    moveTo(600, 640);
    release(600, 640);
    expect(onCommit).toHaveBeenCalledTimes(1);
    // Pressed at 09:30, let go 40 px below the hours: the pressed point lands on the edge on screen
    // (12:30), never on the 13:30 out of sight under the pointer.
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D2, min: 720 },
      end: { day: D2, min: 780 },
    });
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
          rights: { canEdit: true, canRetime: false, reason },
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
  it("its right end's strip changes its days", () => {
    const { onCommit } = mount();
    press(screen.getByTestId("bar-end"), 420, 70);
    moveTo(600, 70);
    release(600, 70);
    expect(onCommit.mock.calls[0][0].resize).toBe(true);
    expect(onCommit.mock.calls[0][0].to).toEqual({ kind: "all_day", first: D1, last: D2 });
  });

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

/** Month as the view marks it (C16c): the weekday row, then three weeks of 100 px from Monday
 *  12-10, seven 100 px days across, Gym's chip on Monday and the holiday's band on Monday, a
 *  "+1 more" on Tuesday, and Sunday. Its days cover the stream, so a press on the stream itself is
 *  on its scrollbar. */
function MonthGrid({
  host,
  rows,
}: {
  host: Partial<GestureHost>;
  rows: Map<string, CalendarEvent>;
}) {
  const wrapper = useRef<HTMLDivElement>(null);
  const view = useCalendarGesture({
    wrapper: () => wrapper.current,
    surfaceOk: () => true,
    rowById: (id) => rows.get(id),
    capability: canMove,
    createIn: () => null,
    overlayOpen: () => false,
    lengthFor: () => 60,
    zone: () => "Europe/London",
    onCommit: () => {},
    onCreate: () => {},
    announce: () => {},
    ...host,
  });
  return (
    <div ref={wrapper} data-testid="m-wrapper">
      <div data-testid="weekdays" />
      <div
        data-surface="month"
        data-grid-start={D1}
        data-week-h="100"
        data-weeks="3"
        data-testid="stream"
      >
        <div data-day={D1} data-testid="mon">
          <span data-no-create="" data-testid="number">
            12
          </span>
          <div data-event-id="e1" data-testid="chip" />
        </div>
        <div data-day={D2} data-testid="tue">
          <span data-no-create="" data-testid="more">
            +1 more
          </span>
        </div>
        <div data-day="2026-10-18" data-testid="sun" />
        <div data-event-id="a1" data-testid="m-bar" />
      </div>
      {view.ghost && <div data-testid="ghost">{view.ghost.label}</div>}
    </div>
  );
}

function mountMonth(host: Partial<GestureHost> = {}) {
  const spies: Spies = {
    onCommit: vi.fn<GestureHost["onCommit"]>(),
    onCreate: vi.fn<GestureHost["onCreate"]>(),
    announce: vi.fn<GestureHost["announce"]>(),
  };
  const rows = new Map([
    [GYM.id, GYM],
    [HOLIDAY.id, HOLIDAY],
  ]);
  render(<MonthGrid host={{ ...spies, ...host }} rows={rows} />);
  place(screen.getByTestId("m-wrapper"), 0, 0, 700, 340);
  place(screen.getByTestId("weekdays"), 0, 0, 700, 40);
  place(screen.getByTestId("stream"), 0, 40, 700, 340);
  place(screen.getByTestId("mon"), 0, 40, 100, 140);
  place(screen.getByTestId("tue"), 100, 40, 200, 140);
  place(screen.getByTestId("sun"), 600, 40, 700, 140);
  place(screen.getByTestId("chip"), 2, 70, 98, 84);
  place(screen.getByTestId("more"), 104, 120, 160, 134);
  place(screen.getByTestId("number"), 2, 42, 20, 60);
  place(screen.getByTestId("m-bar"), 0, 50, 100, 64);
  return { ...spies, rows };
}

describe("useCalendarGesture: Month", () => {
  it("moves a chip to another day, keeping its time", () => {
    const { onCommit, announce } = mountMonth();
    press(screen.getByTestId("chip"), 50, 77);
    // Thursday of the next week.
    moveTo(350, 177);
    expect(announce).toHaveBeenCalledWith("Moving “Gym”.");
    expect(screen.getByTestId("ghost").textContent).toMatch(/22-10/);
    release(350, 177);
    expect(onCommit).toHaveBeenCalledTimes(1);
    const commit = onCommit.mock.calls[0][0];
    expect(commit.row).toBe(GYM);
    expect(commit.resize).toBe(false);
    expect(commit.to).toEqual({
      kind: "timed",
      start: { day: "2026-10-22", min: 540 },
      end: { day: "2026-10-22", min: 600 },
    });
  });

  it("moves an all-day band by whole days, with no ends to drag", () => {
    const { onCommit } = mountMonth();
    // Its very left end: in Month that's still the body.
    press(screen.getByTestId("m-bar"), 1, 57);
    moveTo(251, 57);
    release(251, 57);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "all_day",
      first: "2026-10-14",
      last: "2026-10-14",
    });
    expect(onCommit.mock.calls[0][0].resize).toBe(false);
  });

  it("scrolls the weeks while dragging near the bottom, and up from the weekday row", () => {
    mountMonth();
    const s = screen.getByTestId("stream");
    // Held at the bottom without passing the threshold: no scroll.
    press(screen.getByTestId("chip"), 50, 77);
    fireEvent.pointerMove(document.body, at(51, 77));
    frame();
    expect(s.scrollTop).toBe(0);
    // Picked up on this frame; it scrolls from the next.
    moveTo(50, 335);
    expect(s.scrollTop).toBe(0);
    frame();
    const down = s.scrollTop;
    expect(down).toBeGreaterThan(0);
    frame();
    expect(s.scrollTop).toBeGreaterThan(down);
    // Over the weekday row, which reads as the top week: back up.
    moveTo(50, 20);
    frame();
    expect(s.scrollTop).toBeLessThan(down);
    release(50, 20);
  });

  it("letting go beside the calendar puts it back", () => {
    const { onCommit, announce } = mountMonth();
    press(screen.getByTestId("chip"), 50, 77);
    moveTo(350, 177);
    release(750, 177);
    expect(onCommit).not.toHaveBeenCalled();
    expect(announce).toHaveBeenLastCalledWith(PUT_BACK);
  });
});

// C17: a click or drag on empty space opens quick create for that slot, through `onCreate`, with
// where the slot is on screen; never while something was open at the press, nowhere marked as no
// place to create, and nowhere when no calendar takes the new event.
describe("useCalendarGesture: creating", () => {
  const creates = { createIn: () => "c1", lengthFor: (ev: CalendarEvent | null) => (ev ? 60 : 30) };
  const box = (r: DOMRect | null) => r && [r.left, r.top, r.right, r.bottom];

  it("a click on empty time creates the slot with the default length", () => {
    const { onCreate, announce } = mount(creates);
    press(screen.getByTestId("col2"), 600, 300);
    release(600, 300);
    expect(onCreate).toHaveBeenCalledTimes(1);
    const [effect, anchor] = onCreate.mock.calls[0];
    // 05:00, floored to its slot, and the create calendar's half hour.
    expect(effect).toEqual({
      effect: "create",
      timing: { kind: "timed", start: { day: D2, min: 300 }, end: { day: D2, min: 330 } },
      drag: false,
    });
    expect(box(anchor)).toEqual([427, 300, 800, 320]);
    // Quick create takes focus and says what it is: nothing is announced, and nothing captured.
    expect(announce).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("a drag down creates the stepped range, drawn as the new event, and hands over its rect", () => {
    const { onCreate } = mount(creates);
    press(screen.getByTestId("col2"), 600, 300);
    moveTo(600, 400);
    const ghost = screen.getByTestId("ghost");
    expect(ghost.dataset.kind).toBe("create");
    expect(ghost.dataset.calendar).toBe("c1");
    expect(document.body.style.cursor).toBe("ns-resize");
    release(600, 400);
    const [effect, anchor] = onCreate.mock.calls[0];
    expect(effect).toEqual({
      effect: "create",
      timing: { kind: "timed", start: { day: D2, min: 300 }, end: { day: D2, min: 450 } },
      drag: true,
    });
    expect(box(anchor)).toEqual([427, 300, 800, 400]);
    expect(screen.queryByTestId("ghost")).toBeNull();
  });

  it("a click while the event popover is open only closes it; a drag then creates nothing", () => {
    const { onCreate } = mount({ ...creates, overlayOpen: () => true });
    press(screen.getByTestId("col2"), 600, 300);
    release(600, 300);
    press(screen.getByTestId("col2"), 600, 300);
    moveTo(600, 400);
    release(600, 400);
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("no calendar to create in: nothing happens", () => {
    const { onCreate } = mount({ ...creates, createIn: () => null });
    press(screen.getByTestId("col2"), 600, 300);
    release(600, 300);
    expect(onCreate).not.toHaveBeenCalled();
  });

  it("a tap creates on touch; a touch on an event is left to it", () => {
    const { onCreate, onCommit } = mount(creates);
    press(screen.getByTestId("col2"), 600, 300, { pointerType: "touch" });
    fireEvent.pointerUp(document.body, at(600, 300, { buttons: 0, pointerType: "touch" }));
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(onCreate.mock.calls[0][0].drag).toBe(false);
    // Touch never drags.
    press(screen.getByTestId("card"), 200, 480, { pointerType: "touch" });
    fireEvent.pointerMove(document.body, at(600, 520, { pointerType: "touch" }));
    frame();
    fireEvent.pointerUp(document.body, at(600, 520, { buttons: 0, pointerType: "touch" }));
    expect(onCommit).not.toHaveBeenCalled();
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(capture).not.toHaveBeenCalled();
  });

  it("the mouse events a tap that created is followed by never reach quick create", () => {
    const { onCreate } = mount(creates);
    const col = screen.getByTestId("col2");
    // Quick create, open once a tap has created: a mousedown outside it closes it while untitled.
    const outside = vi.fn();
    const clicked = vi.fn();
    col.addEventListener("click", clicked);
    const tap = () => {
      press(col, 600, 300, { pointerType: "touch" });
      fireEvent.pointerUp(document.body, at(600, 300, { buttons: 0, pointerType: "touch" }));
      window.addEventListener("mousedown", outside);
    };
    // The browser's mouse events for the tap, at the same spot.
    const followUp = () => {
      fireEvent.mouseDown(col, { clientX: 600, clientY: 300 });
      fireEvent.mouseUp(col, { clientX: 600, clientY: 300 });
      fireEvent.click(col, { clientX: 600, clientY: 300 });
    };
    try {
      tap();
      expect(onCreate).toHaveBeenCalledTimes(1);
      followUp();
      expect(outside).not.toHaveBeenCalled();
      expect(clicked).not.toHaveBeenCalled();
      // The next press is the user's own, and its mousedown closes it as ever.
      press(screen.getByTestId("col1"), 200, 300);
      fireEvent.mouseDown(screen.getByTestId("col1"), { clientX: 200, clientY: 300 });
      expect(outside).toHaveBeenCalledTimes(1);
      release(200, 300);
      // So does any mousedown a moment after a tap.
      tap();
      act(() => {
        vi.advanceTimersByTime(800);
      });
      fireEvent.mouseDown(col, { clientX: 600, clientY: 300 });
      expect(outside).toHaveBeenCalledTimes(2);
      // A mouse click that creates eats nothing.
      press(col, 600, 300);
      release(600, 300);
      followUp();
      expect(outside).toHaveBeenCalledTimes(3);
      expect(clicked).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener("mousedown", outside);
    }
  });

  it("the all-day row creates all day, on the day clicked", () => {
    const { onCreate } = mount(creates);
    press(screen.getByTestId("band"), 600, 70);
    release(600, 70);
    const [effect, anchor] = onCreate.mock.calls[0];
    expect(effect.timing).toEqual({ kind: "all_day", first: D2, last: D2 });
    expect(box(anchor)).toEqual([427, 60, 800, 80]);
  });

  it("Month's days create all day, a drag across them over several", () => {
    const { onCreate } = mountMonth(creates);
    press(screen.getByTestId("tue"), 150, 60);
    release(150, 60);
    expect(onCreate.mock.calls[0][0]).toEqual({
      effect: "create",
      timing: { kind: "all_day", first: D2, last: D2 },
      drag: false,
    });
    press(screen.getByTestId("tue"), 150, 60);
    moveTo(350, 160);
    expect(screen.getByTestId("ghost").textContent).toMatch(/13-10/);
    release(350, 160);
    expect(onCreate.mock.calls[1][0]).toEqual({
      effect: "create",
      timing: { kind: "all_day", first: D2, last: "2026-10-22" },
      drag: true,
    });
  });

  it("the day number and more never create", () => {
    const { onCreate } = mountMonth(creates);
    for (const id of ["number", "more"]) {
      press(screen.getByTestId(id), 120, 125);
      release(120, 125);
      press(screen.getByTestId(id), 120, 125);
      moveTo(350, 160);
      release(350, 160);
    }
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
  });

  it("Month's days are the weeks' width, less a scrollbar that takes room", () => {
    const { onCreate } = mountMonth(creates);
    // A 14 px scrollbar: the seven days share 686 px, so Sunday starts at 588.
    const stream = screen.getByTestId("stream");
    Object.defineProperty(stream, "clientWidth", { value: 686 });
    const sun = screen.getByTestId("sun");
    place(sun, 588, 40, 686, 140);
    press(sun, 590, 60);
    release(590, 60);
    expect(onCreate.mock.calls[0][0].timing).toEqual({
      kind: "all_day",
      first: "2026-10-18",
      last: "2026-10-18",
    });
    // A drag from Sunday that passes over the scrollbar stays on Sunday, never past it.
    press(sun, 590, 60);
    moveTo(695, 160);
    release(695, 160);
    expect(onCreate.mock.calls[1][0].timing).toEqual({
      kind: "all_day",
      first: "2026-10-18",
      last: "2026-10-25",
    });
  });

  it("a press on Month's scrollbar only scrolls", () => {
    const { onCreate } = mountMonth(creates);
    const stream = screen.getByTestId("stream");
    // A 14 px scrollbar down the right, and another along the bottom.
    Object.defineProperty(stream, "clientWidth", { value: 686 });
    Object.defineProperty(stream, "clientHeight", { value: 286 });
    // The right one's track clicked, then its thumb dragged down; the bottom one's track clicked.
    press(stream, 695, 60);
    release(695, 60);
    press(stream, 695, 60);
    moveTo(695, 200);
    release(695, 200);
    press(stream, 300, 330);
    release(300, 330);
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    // The days beside them still create.
    press(screen.getByTestId("sun"), 590, 60);
    release(590, 60);
    expect(onCreate).toHaveBeenCalledTimes(1);
  });

  it("a press on a scrollbar that floats over Month's days only scrolls too", () => {
    const { onCreate } = mountMonth(creates);
    const stream = screen.getByTestId("stream");
    // GTK's and a Mac trackpad's scrollbar takes no room: the days span the stream's whole width,
    // and the bar floats over Sunday's right edge.
    Object.defineProperty(stream, "clientWidth", { value: 700 });
    Object.defineProperty(stream, "clientHeight", { value: 300 });
    // Its track clicked, then its thumb dragged down.
    press(stream, 695, 60);
    release(695, 60);
    press(stream, 695, 60);
    moveTo(695, 200);
    release(695, 200);
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    // Sunday under it, where the bar isn't, still creates.
    press(screen.getByTestId("sun"), 695, 60);
    release(695, 60);
    expect(onCreate.mock.calls[0][0].timing).toEqual({
      kind: "all_day",
      first: "2026-10-18",
      last: "2026-10-18",
    });
  });

  it("a press the grid scrolls under before it's a drag was a scrollbar's: it creates nothing", () => {
    const { onCreate } = mountMonth(creates);
    const stream = screen.getByTestId("stream");
    const sun = screen.getByTestId("sun");
    // An engine that hands a floating bar's press to the day beneath it: the track clicked pages
    // the weeks down, and the button is let go before the next frame.
    press(sun, 695, 60);
    stream.scrollTop = 100;
    release(695, 60);
    // The thumb dragged down: the weeks scroll as the pointer goes.
    press(sun, 695, 60);
    stream.scrollTop = 150;
    moveTo(695, 200);
    release(695, 200);
    expect(onCreate).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    // With the weeks still, a click on Sunday creates, and a drag from it still scrolls them.
    press(sun, 695, 60);
    release(695, 60);
    expect(onCreate).toHaveBeenCalledTimes(1);
    press(sun, 695, 60);
    moveTo(695, 335);
    frame();
    expect(stream.scrollTop).toBeGreaterThan(150);
    expect(capture).toHaveBeenCalledTimes(1);
    release(695, 335);
    expect(onCreate).toHaveBeenCalledTimes(2);
  });

  it("in the hours too, a press they scroll under creates or moves nothing", () => {
    const { onCreate, onCommit, announce } = mount(creates);
    const body = screen.getByTestId("body");
    // The hours' floating thumb dragged down over the next day's column, which got the press.
    press(screen.getByTestId("col2"), 790, 300);
    body.scrollTop = 200;
    moveTo(790, 400);
    release(790, 400);
    // Or over Gym's card, with Gym's day drawn last so the card meets the bar.
    place(screen.getByTestId("col1"), 427, 100, 800, 600);
    place(screen.getByTestId("col2"), 54, 100, 427, 600);
    place(screen.getByTestId("card"), 428, 460, 799, 500);
    press(screen.getByTestId("card"), 790, 480);
    body.scrollTop = 400;
    moveTo(790, 560);
    release(790, 560);
    expect(onCreate).not.toHaveBeenCalled();
    expect(onCommit).not.toHaveBeenCalled();
    expect(capture).not.toHaveBeenCalled();
    // Quietly: nothing was picked up, so nothing is put back.
    expect(announce).not.toHaveBeenCalled();
  });

  it("by the scrollbar, a press as the weeks still settle from a scroll acts as ever", () => {
    const { onCreate } = mountMonth(creates);
    const stream = screen.getByTestId("stream");
    const sun = screen.getByTestId("sun");
    const clock = vi.spyOn(performance, "now").mockReturnValue(1000);
    // A wheel turned: the weeks scroll past a week's start, and Sunday is clicked by the bar as
    // they snap back onto it, before the next frame and after it.
    stream.scrollTop = 30;
    fireEvent.scroll(stream);
    clock.mockReturnValue(1100);
    press(sun, 695, 60);
    stream.scrollTop = 10;
    fireEvent.scroll(stream);
    frame();
    stream.scrollTop = 0;
    release(695, 60);
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(onCreate.mock.calls[0][0].timing).toEqual({
      kind: "all_day",
      first: "2026-10-18",
      last: "2026-10-18",
    });
    // Pressed as they settle and dragged down a week, it's picked up and creates over both.
    stream.scrollTop = 20;
    fireEvent.scroll(stream);
    press(sun, 695, 60);
    stream.scrollTop = 0;
    fireEvent.scroll(stream);
    moveTo(695, 160);
    release(695, 160);
    expect(onCreate.mock.calls[1][0].timing).toEqual({
      kind: "all_day",
      first: "2026-10-18",
      last: "2026-10-25",
    });
    // Once they've stood still a moment, a press they scroll under by the bar is its own again.
    clock.mockReturnValue(1300);
    press(sun, 695, 60);
    stream.scrollTop = 100;
    release(695, 60);
    expect(onCreate).toHaveBeenCalledTimes(2);
  });

  it("away from Month's scrollbar, a press acts though the weeks move under it", () => {
    const { onCreate, onCommit } = mountMonth(creates);
    const stream = screen.getByTestId("stream");
    // Tuesday clicked as the weeks start to snap onto a week.
    press(screen.getByTestId("tue"), 150, 60);
    stream.scrollTop = 40;
    frame();
    release(150, 60);
    expect(onCreate.mock.calls[0][0].timing).toEqual({ kind: "all_day", first: D2, last: D2 });
    // Gym's chip pressed as they move is picked up and moved.
    press(screen.getByTestId("chip"), 50, 77);
    stream.scrollTop = 0;
    moveTo(350, 177);
    release(350, 177);
    expect(onCommit).toHaveBeenCalledTimes(1);
  });

  it("away from the hours' scrollbar, and on the all-day row, a press acts though the hours move", () => {
    const { onCreate, onCommit } = mount(creates);
    const body = screen.getByTestId("body");
    // Gym pressed as the hours glide on from a fling, and dragged to the next day.
    press(screen.getByTestId("card"));
    body.scrollTop = 40;
    moveTo(600, 520);
    release(600, 520);
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit.mock.calls[0][0].to).toEqual({
      kind: "timed",
      start: { day: D2, min: 660 },
      end: { day: D2, min: 720 },
    });
    // The all-day row at its right end, above where the hours' bar floats, as they glide.
    press(screen.getByTestId("band"), 790, 70);
    body.scrollTop = 80;
    release(790, 70);
    expect(onCreate.mock.calls[0][0].timing).toEqual({ kind: "all_day", first: D2, last: D2 });
  });

  it("only the scrolling element's own press can be on its scrollbar: the all-day row still creates", () => {
    const { onCreate } = mount(creates);
    // The hours scroll below the all-day row, a 14 px scrollbar down their right.
    const body = screen.getByTestId("body");
    Object.defineProperty(body, "clientWidth", { value: 786 });
    Object.defineProperty(body, "clientHeight", { value: 500 });
    press(screen.getByTestId("band"), 600, 70);
    release(600, 70);
    expect(onCreate.mock.calls[0][0].timing).toEqual({ kind: "all_day", first: D2, last: D2 });
  });
});

describe("overlayShown", () => {
  it("counts a dialog, the event's details and any other popover", () => {
    expect(overlayShown(false)).toBe(false);
    expect(overlayShown(true)).toBe(true);
    const panel = document.createElement("div");
    panel.dataset.popoverPanel = "";
    document.body.appendChild(panel);
    expect(overlayShown(false)).toBe(true);
    panel.remove();
    env.dialog = true;
    expect(overlayShown(false)).toBe(true);
  });
});
