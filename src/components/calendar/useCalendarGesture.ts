// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Dragging on the calendar grid (#884, C16b): moving an event in Day and Week, changing when it starts
// or ends by its top or bottom edge, an all-day event's days by its ends, and between the all-day row
// and the hours; in Month, moving it to another day, keeping its times (C16c). The pure half is
// `calendarDrag` (where a drop lands) and `calendarGesture` (what each press, frame, release or
// cancel does); this is the half that touches the page.
//
// THE PRESS IS ONE WINDOW LISTENER FOR THE HOOK'S LIFE, AND OWNERSHIP IS GEOMETRY. The swipe's lesson
// (`useHorizontalWheelShift`, #698): the day columns are keyed by date, so a swipe or ← mid-drag
// unmounts the column a drag began over, and anything bound to it goes with it. So cards carry data
// attributes and nothing else; one capture listener on the window reads what a press took from them,
// and every frame measures the grid afresh (nothing is kept from the press but the row it took), so a
// column re-keyed under the pointer just reads differently on the next frame. The press listener
// never prevents the default: the mousedown after it still closes the event's details and any other
// popover, and whether one was open is read first, so that press only closes it.
//
// The phase lives here, not in React state: each listener steps the machine and runs what it says
// there and then, once, so StrictMode (which runs renders and effects twice) can't send a move twice,
// and a capture lost just after the release can't undo it. React state only mirrors what is drawn
// (the ghost, the dimmed event, the reason by the pointer), and a frame inside the same 15-minute
// step re-renders nothing.
//
// Letting go on a valid place that changes something is the only way a drop happens. Escape, the
// window losing focus or being hidden, the pointer being cancelled or captured away, a dialog
// opening, or the calendar going all put it back. It imports no write and names no command: a drop is
// handed to the calendar (`onCommit`), which asks and moves through the write hook.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  autoScrollDelta,
  bandHitZone,
  cardHitZone,
  exceedsThreshold,
  gapText,
  ghostEvent,
  grabAllowed,
  inCancelZone,
  planClick,
  planGesture,
  resolveMonthTarget,
  resolveTimeGridTarget,
  slotRect,
  timingOf,
  type Box,
  type DragCapability,
  type Grab,
  type MonthGeom,
  type Plan,
  type Press,
  type Surface,
  type Target,
  type TimeGridGeom,
  type Timing,
} from "../../lib/calendarDrag";
import {
  gestureStep,
  IDLE,
  type CancelWhy,
  type GestureAction,
  type GestureEffect,
  type GesturePhase,
} from "../../lib/calendarGesture";
import type { CalendarEvent } from "../../lib/types";
import { isAnyDialogOpen } from "../../lib/useDialogLayer";
import type { GhostView } from "./parts/DragGhost";
import { whenText } from "./parts/whenText";

type Commit = Extract<GestureEffect, { effect: "commit" }>;
type Create = Extract<GestureEffect, { effect: "create" }>;

/** What the calendar gives the drag. Every function is read when it's needed, never kept. */
export interface GestureHost {
  /** The calendar's grid wrapper, which isn't keyed: the pointer is held here, never on a card. */
  wrapper: () => HTMLElement | null;
  /** The view on screen drags now (Day, Week and Month, with a Google account that has editing
   *  on). */
  surfaceOk: () => boolean;
  /** The event as drawn, by id, read at the press: the drop is made from it. */
  rowById: (id: string) => CalendarEvent | undefined;
  capability: (ev: CalendarEvent) => DragCapability;
  /** A press on empty space may create an event (C17); until then nothing is taken there. */
  canCreate: () => boolean;
  /** The event's details or another popover is open now, read before the press closes it. */
  overlayOpen: () => boolean;
  /** How long `ev` becomes when dragged from the all-day row into the hours (its account's default
   *  length), or a new event (null), in minutes. */
  lengthFor: (ev: CalendarEvent | null) => number;
  /** This computer's time zone, or null when it can't be told. */
  zone: () => string | null;
  onCommit: (e: Commit) => void;
  /** Open quick create for a slot, beside it when it's on screen (C17). */
  onCreate: (e: Create, anchor: DOMRect | null) => void;
  /** Say `text` in the calendar's polite live region. */
  announce: (text: string) => void;
}

/** What the grid draws for the drag now. */
export interface GestureView {
  /** A drag is under way. */
  live: boolean;
  /** The event being dragged, dimmed where it was; null when it would go nowhere. */
  originId: string | null;
  ghost: GhostView | null;
  /** Why the event pressed can't be dragged, shown by the pointer while the button is down. */
  chip: string | null;
}

export interface CalendarGesture extends GestureView {
  /** For the reason's element: it follows the pointer without a render per move. */
  chipRef: (el: HTMLElement | null) => void;
}

const NONE: GestureView = { live: false, originId: null, ghost: null, chip: null };

/** Nowhere to drop: letting go puts it back. */
const OUTSIDE: Plan = { ok: false, why: "outside" };

/** How far right of and below the pointer the reason sits, in px. */
const CHIP_OFFSET = 12;

/** What a press elsewhere on the grid leaves alone: controls (the add-zone button among them). */
const CONTROLS = "button, a, input, select, textarea, [data-no-create]";

const boxOf = (r: DOMRect): Box => ({ left: r.left, top: r.top, right: r.right, bottom: r.bottom });

/** An event's surface, from its `data-surface`: the hours, the all-day row or Month; anything else
 *  isn't the drag's. */
const surfaceOf = (s: string | undefined): Surface | null =>
  s === "grid" || s === "band" || s === "month" ? s : null;

/** A no-break space, written as its code so it stays visible in the source. */
const NBSP = String.fromCharCode(0xa0);

/** What a live region shows to say `text` after `was`: the same words twice ("Put back." after
 *  "Put back.") differ by a no-break space, so a screen reader says them again. */
export function sayAgain(was: string, text: string): string {
  return was === text ? `${text}${NBSP}` : text;
}

/** The grid inside `wrapper` as laid out now, with the element that scrolls it: the time grid's
 *  hours, or Month's weeks. Null when neither is drawn there. */
type Measured =
  | { kind: "time"; geom: TimeGridGeom; scroller: HTMLElement }
  | { kind: "month"; geom: MonthGeom; scroller: HTMLElement };

function measure(wrapper: HTMLElement): Measured | null {
  const outer = boxOf(wrapper.getBoundingClientRect());
  const body = wrapper.querySelector<HTMLElement>("[data-time-body]");
  if (body) {
    const cols = Array.from(
      wrapper.querySelectorAll<HTMLElement>('[data-surface="grid"]'),
      (el) => {
        const r = el.getBoundingClientRect();
        return { day: el.dataset.day ?? "", left: r.left, right: r.right };
      },
    );
    const band = wrapper.querySelector<HTMLElement>('[data-surface="band"]');
    return {
      kind: "time",
      scroller: body,
      geom: {
        wrapper: outer,
        body: boxOf(body.getBoundingClientRect()),
        scrollTop: body.scrollTop,
        rowH: Number(body.dataset.rowH),
        cols,
        band: band
          ? { ...boxOf(band.getBoundingClientRect()), days: band.dataset.days?.split(",") ?? [] }
          : null,
      },
    };
  }
  const stream = wrapper.querySelector<HTMLElement>('[data-surface="month"]');
  if (!stream) return null;
  return {
    kind: "month",
    scroller: stream,
    geom: {
      wrapper: outer,
      stream: boxOf(stream.getBoundingClientRect()),
      scrollTop: stream.scrollTop,
      weekH: Number(stream.dataset.weekH),
      gridStart: stream.dataset.gridStart ?? "",
      weeks: Number(stream.dataset.weeks),
    },
  };
}

/** Where the pointer at (x, y) is on the grid `m`. */
const targetIn = (m: Measured, x: number, y: number): Target | null =>
  m.kind === "time" ? resolveTimeGridTarget(m.geom, x, y) : resolveMonthTarget(m.geom, x, y);

/** Whether a press may change anything once dragged: empty space (which only gets this far where it
 *  may create), or an event's part it may move. */
function acting(press: Press): boolean {
  const on = press.on;
  return on.kind === "empty" || (on.origin !== null && grabAllowed(on.grab, on.cap));
}

/** The pointer's look while dragging: what's being changed, or that nothing may be. */
function cursorFor(press: Press): string {
  if (!acting(press)) return "not-allowed";
  const grab: Grab = press.on.kind === "event" ? press.on.grab : "end";
  if (grab === "start" || grab === "end") return "ns-resize";
  if (grab === "left" || grab === "right") return "ew-resize";
  return "grabbing";
}

/** What the grid draws for `phase`. */
function viewOf(phase: GesturePhase): GestureView {
  if (phase.phase !== "dragging") return NONE;
  const on = phase.press.on;
  // A drag over empty space draws the new event's outline from C17.
  if (on.kind === "empty") return { ...NONE, live: true };
  if (!acting(phase.press)) return { ...NONE, live: true, chip: on.cap.reason };
  const plan = phase.plan;
  const timing: Timing | null =
    plan === null ? null : plan.ok ? plan.timing : plan.why === "gap" ? plan.timing : null;
  // Nowhere to drop (the cancel zone): it shows where it is, as letting go would leave it.
  if (timing === null) return { ...NONE, live: true };
  const event = ghostEvent(on.row, timing);
  return {
    live: true,
    originId: on.row.id,
    ghost: {
      event,
      kind: on.grab === "body" ? "move" : "resize",
      invalid: plan !== null && !plan.ok && plan.why === "gap" ? gapText(plan.at) : null,
      label: whenText(event),
    },
    chip: null,
  };
}

/** The drag on the calendar's grid, for the life of the calendar. */
export function useCalendarGesture(host: GestureHost): CalendarGesture {
  // Read in the listeners, which are bound once: always this render's host.
  const hostRef = useRef(host);
  hostRef.current = host;
  const [phase, setPhase] = useState<GesturePhase>(IDLE);
  const pos = useRef({ x: 0, y: 0 });
  const chipEl = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let state: GesturePhase = IDLE;
    const now = () => state;
    let raf = 0;
    /** Where the pointer is held, and how to put the cursor and selection back. */
    let held: { el: HTMLElement; id: number; restore: () => void } | null = null;
    /** Takes off the listeners a press puts on. */
    let detach: (() => void) | null = null;
    /** Takes off a click swallow still waiting for its pointerup. */
    let unarm: (() => void) | null = null;

    const placeChip = () => {
      const el = chipEl.current;
      if (!el) return;
      el.style.left = `${pos.current.x + CHIP_OFFSET}px`;
      el.style.top = `${pos.current.y + CHIP_OFFSET}px`;
    };

    /** The plan with the pointer at (x, y), the grid measured now; `scroll` first scrolls the hours
     *  (or Month's weeks) when the pointer is at their top or bottom edge, or past it. */
    const planAt = (press: Press, x: number, y: number, scroll: boolean): Plan | null => {
      const h = hostRef.current;
      const wrapper = h.wrapper();
      const found = wrapper ? measure(wrapper) : null;
      if (found && scroll) {
        const g = found.geom;
        const { top, bottom } = found.kind === "time" ? found.geom.body : found.geom.stream;
        // The hours only over them or past their bottom, never from the all-day row or the
        // headers; Month's weeks from its weekday row too, which reads as the top week.
        const over = found.kind === "month" || y >= top;
        if (over && !inCancelZone(g.wrapper, x, y)) {
          const d = autoScrollDelta(y, top, bottom);
          if (d !== 0) {
            found.scroller.scrollTop += d;
            g.scrollTop = found.scroller.scrollTop;
          }
        }
      }
      const target = found ? targetIn(found, x, y) : null;
      const row = press.on.kind === "event" ? press.on.row : null;
      return planGesture(press, target, { lengthMin: h.lengthFor(row), zone: h.zone() });
    };

    /** Where a new event's slot is on screen, for quick create to open beside. */
    const anchorFor = (t: Timing): DOMRect | null => {
      const wrapper = hostRef.current.wrapper();
      const found = wrapper ? measure(wrapper) : null;
      const b = found ? slotRect(found.geom, t) : null;
      return b ? new DOMRect(b.left, b.top, b.right - b.left, b.bottom - b.top) : null;
    };

    const capture = (press: Press): boolean => {
      const el = hostRef.current.wrapper();
      if (!el) return false;
      try {
        el.setPointerCapture(press.pointerId);
      } catch {
        return false;
      }
      const cursor = cursorFor(press);
      const was = {
        body: document.body.style.cursor,
        cursor: el.style.cursor,
        select: el.style.getPropertyValue("user-select"),
        webkit: el.style.getPropertyValue("-webkit-user-select"),
      };
      document.body.style.cursor = cursor;
      el.style.cursor = cursor;
      el.style.setProperty("user-select", "none");
      el.style.setProperty("-webkit-user-select", "none");
      held = {
        el,
        id: press.pointerId,
        restore: () => {
          document.body.style.cursor = was.body;
          el.style.cursor = was.cursor;
          el.style.setProperty("user-select", was.select);
          el.style.setProperty("-webkit-user-select", was.webkit);
        },
      };
      return true;
    };

    const uncapture = () => {
      if (!held) return;
      const { el, id, restore } = held;
      held = null;
      restore();
      try {
        if (el.hasPointerCapture(id)) el.releasePointerCapture(id);
      } catch {
        // Already let go of (the engine's own release): nothing to do.
      }
    };

    /** Eat the next click, once: now, for a release (its click follows in this same input), or
     *  after the next pointerup, for a cancel with the button still down (Escape mid-drag, then
     *  letting go over the event opens nothing). A new press first means that pointerup was lost
     *  (the window lost focus), and nothing is eaten. Engines differ on whether a click follows a
     *  captured drag at all, so the swallow lapses after this turn. */
    const swallowClick = (releasing: boolean) => {
      const eat = () => {
        const onClick = (e: MouseEvent) => {
          e.stopPropagation();
          e.preventDefault();
          done();
        };
        const done = () => {
          window.removeEventListener("click", onClick, true);
          clearTimeout(timer);
        };
        window.addEventListener("click", onClick, true);
        const timer = setTimeout(done, 0);
      };
      unarm?.();
      if (releasing) {
        eat();
        return;
      }
      const onUp = () => {
        off();
        eat();
      };
      const off = () => {
        window.removeEventListener("pointerup", onUp, true);
        window.removeEventListener("pointerdown", off, true);
        unarm = null;
      };
      window.addEventListener("pointerup", onUp, true);
      window.addEventListener("pointerdown", off, true);
      unarm = off;
    };

    const end = () => {
      detach?.();
      detach = null;
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
    };

    const dispatch = (a: GestureAction) => {
      const before = state;
      const step = gestureStep(before, a);
      state = step.state;
      if (state !== before) setPhase(state);
      if (state.phase === "idle" && before.phase !== "idle") end();
      let lost = false;
      for (const fx of step.effects) {
        const h = hostRef.current;
        switch (fx.effect) {
          case "capture":
            if (state.phase === "dragging" && !capture(state.press)) lost = true;
            break;
          case "uncapture":
            uncapture();
            break;
          case "swallowClick":
            swallowClick(a.type === "release");
            break;
          case "commit":
            h.onCommit(fx);
            break;
          case "create":
            h.onCreate(fx, anchorFor(fx.timing));
            break;
          case "announce":
            h.announce(fx.text);
            break;
        }
      }
      // The pointer couldn't be held: put it back rather than lose the release.
      if (lost) dispatch({ type: "cancel", why: "lostcapture" });
    };
    const cancel = (why: CancelWhy) => {
      if (now().phase !== "idle") dispatch({ type: "cancel", why });
    };

    const tick = () => {
      raf = 0;
      const s = now();
      if (s.phase === "idle") return;
      // A dialog that opened mid-drag (a shortcut, a delete asking again) owns the pointer now.
      if (isAnyDialogOpen()) {
        cancel("dialog");
        return;
      }
      const { x, y } = pos.current;
      const press = s.press;
      if (
        s.phase === "dragging" ||
        exceedsThreshold(x - press.x0, y - press.y0, press.pointerType)
      ) {
        dispatch({
          type: "frame",
          x,
          y,
          plan: planAt(press, x, y, s.phase === "dragging" && acting(press)),
        });
      }
      if (now().phase !== "idle") raf = requestAnimationFrame(tick);
    };

    /** Listen from the press to its end. */
    const attach = (press: Press, wrapper: HTMLElement) => {
      const mine = (e: PointerEvent) => e.pointerId === press.pointerId;
      const onMove = (e: PointerEvent) => {
        if (!mine(e)) return;
        pos.current = { x: e.clientX, y: e.clientY };
        placeChip();
      };
      const onUp = (e: PointerEvent) => {
        if (!mine(e)) return;
        pos.current = { x: e.clientX, y: e.clientY };
        const s = now();
        if (s.phase === "idle") return;
        // An event that left the calendar mid-drag (deleted, or moved, elsewhere) has nowhere to
        // go: it's put back, and the next read shows where it is.
        const on = s.press.on;
        const gone = on.kind === "event" && hostRef.current.rowById(on.row.id) === undefined;
        dispatch({
          type: "release",
          x: e.clientX,
          y: e.clientY,
          plan: gone ? OUTSIDE : planAt(s.press, e.clientX, e.clientY, false),
        });
      };
      const onCancel = (e: PointerEvent) => {
        if (mine(e)) cancel("pointercancel");
      };
      const onKey = (e: KeyboardEvent) => {
        // Before the calendar's own keys and the details' Escape: mid-drag, Escape only puts it
        // back, and Ctrl/Cmd+Z (which would undo something else) does nothing.
        const undoKey = (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z";
        if (e.key !== "Escape" && !undoKey) return;
        e.preventDefault();
        e.stopPropagation();
        if (e.key === "Escape") cancel("escape");
      };
      const onBlur = () => cancel("blur");
      const onVisibility = () => {
        if (document.visibilityState === "hidden") cancel("hidden");
      };
      const onLost = () => {
        if (now().phase === "dragging") cancel("lostcapture");
      };
      // From the press, not the threshold: a selection starts at the mousedown, and a native drag
      // of a selected word would end the gesture with a pointercancel.
      const noDefault = (e: Event) => e.preventDefault();
      window.addEventListener("pointermove", onMove, true);
      window.addEventListener("pointerup", onUp, true);
      window.addEventListener("pointercancel", onCancel, true);
      window.addEventListener("keydown", onKey, true);
      window.addEventListener("blur", onBlur);
      document.addEventListener("visibilitychange", onVisibility);
      wrapper.addEventListener("lostpointercapture", onLost);
      document.addEventListener("selectstart", noDefault);
      document.addEventListener("dragstart", noDefault);
      detach = () => {
        window.removeEventListener("pointermove", onMove, true);
        window.removeEventListener("pointerup", onUp, true);
        window.removeEventListener("pointercancel", onCancel, true);
        window.removeEventListener("keydown", onKey, true);
        window.removeEventListener("blur", onBlur);
        document.removeEventListener("visibilitychange", onVisibility);
        wrapper.removeEventListener("lostpointercapture", onLost);
        document.removeEventListener("selectstart", noDefault);
        document.removeEventListener("dragstart", noDefault);
      };
    };

    /** What a press at `e` on the grid in `wrapper` took, or null when it isn't the drag's. */
    const pressOf = (e: PointerEvent, wrapper: HTMLElement): Press | null => {
      const h = hostRef.current;
      const t = e.target;
      // A press on something drawn over the grid but outside it (the details, a notice, quick
      // create) is that thing's, whatever box it sits in.
      if (!(t instanceof Element) || !wrapper.contains(t)) return null;
      const found = measure(wrapper);
      const at = found ? targetIn(found, e.clientX, e.clientY) : null;
      if (!at) return null;
      const base = {
        pointerId: e.pointerId,
        pointerType: e.pointerType,
        x0: e.clientX,
        y0: e.clientY,
        at,
        // Read before the mousedown that follows closes it: this press only closes it.
        dismissOnly: h.overlayOpen(),
      };
      const card = t.closest<HTMLElement>("[data-event-id]");
      if (card) {
        const row = h.rowById(card.dataset.eventId ?? "");
        const surface = surfaceOf(card.closest<HTMLElement>("[data-surface]")?.dataset.surface);
        if (!row || !surface) return null;
        const r = card.getBoundingClientRect();
        // The ends the card draws as draggable are the ones it may be dragged by; Month draws
        // none.
        const has = (edge: string) => card.querySelector(`[data-edge="${edge}"]`) !== null;
        const grab: Grab =
          surface === "grid"
            ? cardHitZone(e.clientY - r.top, r.height, { start: has("start"), end: has("end") })
            : surface === "band"
              ? bandHitZone(e.clientX - r.left, r.width, { left: has("left"), right: has("right") })
              : "body";
        return {
          ...base,
          on: {
            kind: "event",
            row,
            grab,
            surface,
            origin: timingOf(row),
            pieceDay: card.dataset.day ?? at.day,
            cap: h.capability(row),
          },
        };
      }
      if (t.closest(CONTROLS) || !t.closest("[data-surface]") || !h.canCreate()) return null;
      const surface: Surface = at.zone === "timed" ? "grid" : at.zone;
      return {
        ...base,
        on: {
          kind: "empty",
          surface,
          create: planClick(at, { lengthMin: h.lengthFor(null), zone: h.zone() }),
        },
      };
    };

    const onDown = (e: PointerEvent) => {
      const s = now();
      if (s.phase !== "idle") {
        // The same pointer down again: its release never came. Put it back.
        if (e.pointerId === s.press.pointerId) cancel("pointercancel");
        return;
      }
      const h = hostRef.current;
      const wrapper = h.wrapper();
      if (!wrapper || !h.surfaceOk()) return;
      // The main button, unmodified (a Mac's Ctrl-click is a right-click), never touch (Q14), and
      // never under a dialog, which owns the press.
      if (e.button !== 0 || e.shiftKey || e.ctrlKey || e.altKey || e.metaKey) return;
      if (e.pointerType === "touch" || isAnyDialogOpen()) return;
      const press = pressOf(e, wrapper);
      if (!press) return;
      pos.current = { x: e.clientX, y: e.clientY };
      dispatch({ type: "press", press });
      attach(press, wrapper);
      raf = requestAnimationFrame(tick);
    };

    // Passive: it never prevents the default (see the header).
    window.addEventListener("pointerdown", onDown, { capture: true, passive: true });
    return () => {
      window.removeEventListener("pointerdown", onDown, { capture: true });
      cancel("unmount");
      end();
      unarm?.();
    };
  }, []);

  const chipRef = useCallback((el: HTMLElement | null) => {
    chipEl.current = el;
    if (el) {
      el.style.left = `${pos.current.x + CHIP_OFFSET}px`;
      el.style.top = `${pos.current.y + CHIP_OFFSET}px`;
    }
  }, []);

  return useMemo(() => ({ ...viewOf(phase), chipRef }), [phase, chipRef]);
}
