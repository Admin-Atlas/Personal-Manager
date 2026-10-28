// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The gesture on the calendar grid (#884, C15) as a state machine: a press, the frames while the
// pointer moves, then a release or a cancel, each giving the next state and what to do about it as
// data (`GestureEffect`). The hook (C16) measures the boxes, plans each frame with
// `calendarDrag.planGesture`, calls `gestureStep` straight from each listener and runs the effects
// there, once; this only decides. A step that changes nothing returns the same state object, so a
// frame inside the same 15-minute step re-renders nothing.
//
// Only letting go on a valid plan that changes something moves an event. A pointercancel, a blur, a
// hidden window, a lost capture, a dialog opening, Escape or an unmount puts it back, and the grid,
// still until then, scrolling under a press by its right edge before it's a drag ends that press.

import {
  exceedsThreshold,
  gapAnnouncement,
  grabAllowed,
  samePlan,
  type Plan,
  type Press,
  type PressOn,
  type Timing,
} from "./calendarDrag";
import type { CalendarEvent } from "./types";

export type GesturePhase =
  | { phase: "idle" }
  | { phase: "pressed"; press: Press }
  /** `plan` is the last frame's; null until one plans something, and always for a press on an
   *  event that may do nothing (its reason shows by the pointer). */
  | { phase: "dragging"; press: Press; plan: Plan | null };

type Pressed = Extract<GesturePhase, { phase: "pressed" }>;
type Dragging = Extract<GesturePhase, { phase: "dragging" }>;

export type CancelWhy =
  | "escape"
  | "pointercancel"
  | "blur"
  | "hidden"
  | "lostcapture"
  | "unmount"
  /** A dialog opened mid-drag (a shortcut, a delete asking again): it owns the pointer now. */
  | "dialog"
  /** The grid, still at the press, scrolled under a press by its right edge that wasn't a drag
   *  yet: it was on a scrollbar that floats there (or a wheel turned), and just ends. */
  | "scrolled";

export type GestureAction =
  | { type: "press"; press: Press }
  /** `plan` is `planGesture` at the pointer; null keeps the last frame's. */
  | { type: "frame"; x: number; y: number; plan: Plan | null }
  | { type: "release"; x: number; y: number; plan: Plan | null }
  | { type: "cancel"; why: CancelWhy };

export type GestureEffect =
  /** The press became a drag: pointer capture on the calendar, the cursor, no text selection. */
  | { effect: "capture" }
  | { effect: "uncapture" }
  /** The click that follows this gesture is eaten, once. */
  | { effect: "swallowClick" }
  /** Move `row`, as it was drawn when pressed, from `from` to `to`; `resize` when an edge was
   *  dragged. */
  | { effect: "commit"; row: CalendarEvent; from: Timing; to: Timing; resize: boolean }
  /** Open quick create for `timing`, from a click or a drag over empty space. */
  | { effect: "create"; timing: Timing; drag: boolean }
  /** Say `text` in the polite live region. */
  | { effect: "announce"; text: string };

export interface GestureStep {
  state: GesturePhase;
  effects: GestureEffect[];
}

export const IDLE: GesturePhase = { phase: "idle" };

/** Said when a drag of an event ends with nothing moved. */
export const PUT_BACK = "Put back.";

/** Said when an event is picked up. */
export function movingText(title: string): string {
  return `Moving “${title.trim() || "(no title)"}”.`;
}

const stay = (state: GesturePhase): GestureStep => ({ state, effects: [] });
const idle = (effects: GestureEffect[] = []): GestureStep => ({ state: IDLE, effects });
const announce = (text: string): GestureEffect => ({ effect: "announce", text });

/** Whether a press on an event may change anything. */
function canAct(on: Extract<PressOn, { kind: "event" }>): boolean {
  return on.origin !== null && grabAllowed(on.grab, on.cap);
}

const pastThreshold = (press: Press, x: number, y: number) =>
  exceedsThreshold(x - press.x0, y - press.y0, press.pointerType);

/** The next state and what to do, for `a` in `s`. */
export function gestureStep(s: GesturePhase, a: GestureAction): GestureStep {
  switch (s.phase) {
    case "idle":
      return a.type === "press" ? stay({ phase: "pressed", press: a.press }) : stay(s);
    case "pressed":
      return fromPressed(s, a);
    case "dragging":
      return fromDragging(s, a);
  }
}

function fromPressed(s: Pressed, a: GestureAction): GestureStep {
  const press = s.press;
  switch (a.type) {
    case "press":
      return stay(s); // another pointer: this press goes on
    case "cancel":
      return idle();
    case "frame":
      return pastThreshold(press, a.x, a.y) ? pickUp(press, a.plan) : stay(s);
    case "release": {
      if (!pastThreshold(press, a.x, a.y)) return click(press);
      // A flick: past the threshold between two frames. Picked up and let go at once, so there's
      // nothing to capture or let go of.
      const up = pickUp(press, a.plan);
      if (up.state.phase !== "dragging") return up;
      const down = fromDragging(up.state, a);
      return {
        state: down.state,
        effects: [...up.effects, ...down.effects].filter(
          (e) => e.effect !== "capture" && e.effect !== "uncapture",
        ),
      };
    }
  }
}

/** The press moved past the threshold. */
function pickUp(press: Press, plan: Plan | null): GestureStep {
  // One rule for every press: one that went down while a popover was open only closes it.
  if (press.dismissOnly) return idle();
  const on = press.on;
  if (on.kind === "empty") {
    if (on.create === null) return idle();
    return { state: { phase: "dragging", press, plan }, effects: [{ effect: "capture" }] };
  }
  if (!canAct(on)) {
    // An overlay or the draft says nothing, and its click goes ahead as ever; anything else that
    // can't change says why by the pointer while the button is down.
    if (on.cap.reason === null) return idle();
    return {
      state: { phase: "dragging", press, plan: null },
      effects: [{ effect: "capture" }, announce(on.cap.reason)],
    };
  }
  return {
    state: { phase: "dragging", press, plan },
    effects: [{ effect: "capture" }, announce(movingText(on.row.summary))],
  };
}

/** A release that never left the threshold: an event's own click opens its details as it always
 *  has; empty space creates where it may, unless the press only closed a popover. */
function click(press: Press): GestureStep {
  const on = press.on;
  if (on.kind === "event" || press.dismissOnly || on.create === null) return idle();
  const plan = on.create;
  if (plan.ok) return idle([{ effect: "create", timing: plan.timing, drag: false }]);
  if (plan.why === "gap") return idle([announce(gapAnnouncement(plan.at))]);
  return idle();
}

function fromDragging(s: Dragging, a: GestureAction): GestureStep {
  const on = s.press.on;
  const acting = on.kind === "empty" || canAct(on);
  switch (a.type) {
    case "press":
      return stay(s);
    case "frame": {
      if (!acting) return stay(s); // nothing to plan; the reason stays by the pointer
      const plan = a.plan ?? s.plan;
      return samePlan(plan, s.plan) ? stay(s) : stay({ ...s, plan });
    }
    case "cancel":
      return idle([
        { effect: "uncapture" },
        { effect: "swallowClick" },
        // A drag over empty space just ends: nothing was there to put back.
        ...(on.kind === "event" && acting ? [announce(PUT_BACK)] : []),
      ]);
    case "release":
      return letGo(s, a.plan ?? s.plan);
  }
}

function letGo(s: Dragging, plan: Plan | null): GestureStep {
  const done: GestureEffect[] = [{ effect: "uncapture" }, { effect: "swallowClick" }];
  const on = s.press.on;
  if (on.kind === "event") {
    const from = on.origin;
    // Nothing could change: the pointer already said why.
    if (from === null || !grabAllowed(on.grab, on.cap)) return idle(done);
    if (plan === null || (!plan.ok && plan.why === "outside"))
      return idle([...done, announce(PUT_BACK)]);
    if (!plan.ok) return idle([...done, announce(gapAnnouncement(plan.at))]);
    if (plan.noop) return idle(done);
    // No announcement: the notice that follows is the live region.
    return idle([
      ...done,
      { effect: "commit", row: on.row, from, to: plan.timing, resize: on.grab !== "body" },
    ]);
  }
  if (plan === null || (!plan.ok && plan.why === "outside")) return idle(done);
  if (!plan.ok) return idle([...done, announce(gapAnnouncement(plan.at))]);
  // Quick create takes focus and says what it is.
  return idle([...done, { effect: "create", timing: plan.timing, drag: true }]);
}
