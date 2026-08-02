// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Turning a horizontal wheel or trackpad swipe into discrete day steps.
//
// The calendar's day window and the Focus card's Upcoming strip are not scroll containers — they
// render a window of N days chosen by state, with ‹ › buttons to move it. So the app-wide wheel
// normaliser (lib/scrollAxis.ts) can do nothing for them: it looks for an element with real
// overflow, finds none, and correctly leaves the event alone. A trackpad swipe over either simply
// did nothing, with no indication that sideways was even a direction that meant something.
//
// A wheel event is not one step. A trackpad emits a stream of small deltas for a single flick and a
// notched mouse wheel emits one large one, so anything that treats "an event" as "a day" either
// crawls or teleports. This accumulates pixels and emits a step each time the threshold is crossed,
// which is the same shape a native scroller uses — hence one shared, tested helper rather than the
// two subtly different versions the two surfaces would otherwise grow.

/** Pixels of horizontal travel per day step. Tuned so one deliberate trackpad flick moves a day or
 *  two rather than a week, and one notch of a tilt wheel (typically 40–120px) moves exactly one. */
export const WHEEL_STEP_PX = 55;

/** The largest jump one event may produce. A single trackpad "fling" can report several hundred
 *  pixels at once; without this the view would leap most of a month and lose the user's place. */
const MAX_STEPS_PER_EVENT = 3;

/**
 * One event's sideways travel in PIXELS, whatever unit the platform chose to report.
 *
 * `deltaX` is only in pixels when `deltaMode` is 0. A device or engine that reports LINES (1) or
 * PAGES (2) sends numbers one to two orders of magnitude smaller — a line-mode notch is `deltaX: 1`,
 * which against a 55px threshold means 55 notches per day rather than one, and reads as the gesture
 * simply not working. `scrollAxis.ts` has normalised this since it was written; this half was
 * comparing raw units to a pixel constant. The 16px-per-line figure is the same one used there.
 */
export function horizontalPixels(e: { deltaX: number; deltaMode: number }): number {
  if (e.deltaMode === 1) return e.deltaX * 16; // lines
  if (e.deltaMode === 2) return e.deltaX * WHEEL_STEP_PX * 7; // pages ≈ a week's worth
  return e.deltaX;
}

/**
 * Whether a wheel event's pointer falls inside a day-stepping region's box.
 *
 * GEOMETRY, not DOM ancestry, and that distinction is the whole reason this exists. Deciding
 * ownership with `el.contains(e.target)` needs the event to still be dispatched through a node that
 * is inside `el` — and the calendar's day columns are keyed by date, so EVERY one of them is
 * replaced the instant a step lands. A gesture that began over a column and continues after the
 * step has its subsequent events targeted at a node that is no longer in the tree, and the swipe
 * dies after exactly one day. A rectangle cannot be replaced out from under a gesture.
 *
 * A zero-extent box is never owned: an unmounted or display:none region reports 0×0 at 0,0, which
 * would otherwise claim every wheel event in the top-left corner of the window.
 */
export function withinRect(
  rect: { left: number; right: number; top: number; bottom: number },
  x: number,
  y: number,
): boolean {
  if (rect.right <= rect.left || rect.bottom <= rect.top) return false;
  return x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom;
}

export interface ShiftResult {
  /** Whole day steps to apply now (negative = earlier). */
  steps: number;
  /** Sub-threshold travel to carry into the next event. */
  carry: number;
}

/**
 * Fold one event's horizontal delta into the running carry.
 *
 * The carry is RESET, not kept, whenever a step is emitted beyond the per-event cap — otherwise a
 * fling's discarded travel would sit in the carry and fire spuriously on the next, unrelated flick.
 */
export function accumulateShift(
  carry: number,
  deltaX: number,
  stepPx: number = WHEEL_STEP_PX,
): ShiftResult {
  const total = carry + deltaX;
  const raw = Math.trunc(total / stepPx);
  if (raw === 0) return { steps: 0, carry: total };
  const steps = Math.max(-MAX_STEPS_PER_EVENT, Math.min(MAX_STEPS_PER_EVENT, raw));
  return { steps, carry: steps === raw ? total - raw * stepPx : 0 };
}

/**
 * Whether a wheel event is a horizontal gesture this should act on.
 *
 * Ctrl (zoom) and Shift (the browser's own "wheel scrolls sideways" convention) are left alone, to
 * match `scrollAxis.ts` — the Map binds both. The axis test is strict: a trackpad reports small
 * cross-axis noise on almost every vertical scroll, and treating that as sideways would make the
 * calendar drift a day while the user was scrolling through the hours of one.
 */
export function isHorizontalGesture(e: {
  deltaX: number;
  deltaY: number;
  ctrlKey: boolean;
  shiftKey: boolean;
}): boolean {
  if (e.ctrlKey || e.shiftKey) return false;
  return Math.abs(e.deltaX) > Math.abs(e.deltaY);
}
