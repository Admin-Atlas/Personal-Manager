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
// A wheel event is not one step, and neither is a pixel count: ONE SWIPE IS ONE DAY. A trackpad
// emits a stream of small deltas for a single flick and a notched mouse wheel one large one, so
// this accumulates pixels until the threshold is crossed, moves a day, and then ignores further
// travel the same way until the travel pauses (a deliberate flick back undoes the day: see
// `swipeStep`). An earlier version stepped a day per 55px, up to three per
// event: one flick on Bobby's trackpad jumped three days, and the dates flickered through each
// step as the swipe ran on (Bobby, 16-10-2026: "change the sidescroll 3 days to 1 day jump").
// One shared, tested helper rather than the two subtly different versions the calendar and the
// Focus card would otherwise grow.

/** Pixels of sideways travel before a swipe counts and moves its day. */
export const WHEEL_STEP_PX = 55;

/** How long sideways travel may pause before the next movement counts as a new swipe. Within one
 *  swipe a trackpad reports every frame or two, and the momentum events some platforms (macOS,
 *  Windows touchpads) send after the fingers lift arrive just as closely, so both stay part of the
 *  swipe they continue. Lifting the fingers and swiping again takes longer. With no lift signal in
 *  a wheel event, resting the fingers this long and moving on counts as a new swipe too.
 *
 *  The cost of the rule: on those platforms, a second flick the same way while the first one's
 *  momentum is still arriving joins the first swipe and moves nothing. A flick BACK still counts
 *  (see {@link swipeStep}), so an overshoot can always be undone at once. */
export const SWIPE_GAP_MS = 150;

/** How long travel short of a day is kept after a swipe that didn't move one: two notches of a tilt
 *  wheel clicked a second or two apart make a day, but small sideways nudges more than this apart
 *  never add up to one. */
export const SWIPE_CARRY_MS = 3000;

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
  if (e.deltaMode === 2) return e.deltaX * WHEEL_STEP_PX * 7; // pages: well past a whole swipe
  return e.deltaX;
}

/**
 * Whether a wheel event's pointer falls inside a day-stepping region's box.
 *
 * GEOMETRY, not DOM ancestry, and that distinction is the whole reason this exists. Deciding
 * ownership with `el.contains(e.target)` needs the event to still be dispatched through a node that
 * is inside `el` — and the calendar's day columns are keyed by date, so a step unmounts the column
 * that leaves the window (in a one-day view, the only one). A gesture that began over it has the rest
 * of its events targeted at a node that is no longer in the tree, which by ancestry is nobody's (the
 * sideways scroll once went dead after one day that way). A rectangle cannot be replaced out from
 * under a gesture.
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

/** One sideways wheel event, as a swipe reads it. */
export interface SwipeEvent {
  /** Its travel in pixels, from {@link horizontalPixels} (negative = earlier). */
  px: number;
  /** When the platform raised it: the event's `timeStamp`. */
  raisedAt: number;
  /** When the page handled it: `performance.now()` in the listener. */
  seenAt: number;
}

/** Where the current swipe has got to. */
export interface SwipeState {
  /** Travel not yet spent on a day, in pixels; once the swipe has moved its day, only travel back. */
  carry: number;
  /** 0 until this swipe moves its day, then that day's direction (1 or -1). */
  dir: number;
  /** The swipe's latest event's two times (see {@link SwipeEvent}). */
  raisedAt: number;
  seenAt: number;
}

export const SWIPE_IDLE: SwipeState = { carry: 0, dir: 0, raisedAt: -Infinity, seenAt: -Infinity };

/**
 * Fold one sideways event into the swipe, and say how many days to move now: -1, 0 or 1.
 *
 * Once the swipe has moved its day, travel on the same way moves nothing more, however far it
 * runs. Travel back gathers, and moves a day back at TWICE the threshold, so a deliberate flick back
 * undoes an overshoot without waiting for the swipe to end, while the small recoil a trackpad can
 * report as the fingers stop never flicks the day back and forth. A tilt wheel's notches more than
 * {@link SWIPE_GAP_MS} apart are separate swipes, and travel short of a day carries between them
 * for {@link SWIPE_CARRY_MS}: with macOS WebKit's 40px notch, every second notch moves a day
 * (WebKitGTK scales a notch with the window's width, past a day on its own at any usual size).
 */
export function swipeStep(
  state: SwipeState,
  e: SwipeEvent,
  stepPx: number = WHEEL_STEP_PX,
): { state: SwipeState; step: number } {
  // How long the travel paused, by both clocks: an engine busy drawing the new day holds the next
  // events back and hands the backlog over merged, stamped with its newest time, so the raised
  // times alone would show a pause the hand never made. The handled times show none, as the
  // backlog is handled in a burst; a real pause shows on both.
  const paused = Math.min(e.raisedAt - state.raisedAt, e.seenAt - state.seenAt);
  const times = { raisedAt: e.raisedAt, seenAt: e.seenAt };
  if (state.dir !== 0 && paused <= SWIPE_GAP_MS) {
    if (Math.sign(e.px) === state.dir) return { state: { ...state, ...times, carry: 0 }, step: 0 };
    const back = state.carry + e.px;
    if (Math.abs(back) < 2 * stepPx) return { state: { ...state, ...times, carry: back }, step: 0 };
    return { state: { carry: 0, dir: -state.dir, ...times }, step: -state.dir };
  }
  const kept = state.dir === 0 && paused <= SWIPE_CARRY_MS ? state.carry : 0;
  const carry = kept + e.px;
  if (Math.abs(carry) < stepPx) return { state: { carry, dir: 0, ...times }, step: 0 };
  const step = Math.sign(carry);
  return { state: { carry: 0, dir: step, ...times }, step };
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
