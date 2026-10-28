// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar grid's gesture as a state machine (#884, C15): one test per row of its table, and the
// rules that keep a drag from writing anything it shouldn't. Plans arrive ready-made, as the hook
// will hand them in; only the decisions are tested here.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DragCapability, Plan, Press, PressOn, Timing, WallPoint } from "./calendarDrag";
import {
  gestureStep,
  IDLE,
  movingText,
  PUT_BACK,
  type CancelWhy,
  type GestureAction,
  type GesturePhase,
} from "./calendarGesture";
import type { CalendarEvent } from "./types";

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2027, 0, 5, 12));
});
afterEach(() => {
  vi.useRealTimers();
});

const D = "2026-10-13";
const at = (h: number, m = 0): WallPoint => ({ day: D, min: h * 60 + m });
const nine: Timing = { kind: "timed", start: at(9), end: at(10) };
const noon: Timing = { kind: "timed", start: at(12), end: at(13) };
const ok = (timing: Timing, noop = false): Plan => ({ ok: true, timing, noop });
const OUTSIDE: Plan = { ok: false, why: "outside" };
const GAP: Plan = {
  ok: false,
  why: "gap",
  at: { day: "2027-03-28", min: 90 },
  timing: {
    kind: "timed",
    start: { day: "2027-03-28", min: 90 },
    end: { day: "2027-03-28", min: 150 },
  },
};
const GAP_WORDS = "01:30 doesn't happen on 28-03.";

const CAN: DragCapability = {
  move: true,
  resizeStart: true,
  resizeEnd: true,
  bandEdges: false,
  reason: null,
};
const NONE: DragCapability = { ...CAN, move: false, resizeStart: false, resizeEnd: false };
const THEIRS = "Someone else organises this event.";

const ROW: CalendarEvent = {
  id: "e1",
  calendar_id: "c1",
  summary: "Standup",
  description: null,
  location: null,
  start: "2026-10-13T08:00:00Z",
  end: "2026-10-13T09:00:00Z",
  all_day: false,
  html_link: null,
  uid: null,
};

type EventOn = Extract<PressOn, { kind: "event" }>;

const onEvent = (over: Partial<EventOn> = {}): PressOn => ({
  kind: "event",
  row: ROW,
  grab: "body",
  surface: "grid",
  origin: nine,
  pieceDay: D,
  cap: CAN,
  ...over,
});
const onEmpty = (create: Plan | null = ok(nine)): PressOn => ({
  kind: "empty",
  surface: "grid",
  create,
});
const press = (on: PressOn, over: Partial<Press> = {}): Press => ({
  pointerId: 1,
  pointerType: "mouse",
  x0: 100,
  y0: 100,
  on,
  at: { zone: "timed", day: D, minute: 560 },
  dismissOnly: false,
  ...over,
});

const pressed = (p: Press): GesturePhase => ({ phase: "pressed", press: p });
const dragging = (p: Press, plan: Plan | null): GesturePhase => ({
  phase: "dragging",
  press: p,
  plan,
});
/** Past the threshold (10 px down) or not (2 px). */
const frame = (plan: Plan | null, far = true): GestureAction => ({
  type: "frame",
  x: 100,
  y: far ? 110 : 102,
  plan,
});
const release = (plan: Plan | null, far = true): GestureAction => ({
  type: "release",
  x: 100,
  y: far ? 110 : 102,
  plan,
});

const CANCELS: CancelWhy[] = [
  "escape",
  "pointercancel",
  "blur",
  "hidden",
  "lostcapture",
  "unmount",
  "dialog",
  "scrolled",
];

describe("from idle", () => {
  it("idle, press → pressed", () => {
    const p = press(onEvent());
    expect(gestureStep(IDLE, { type: "press", press: p })).toEqual({
      state: pressed(p),
      effects: [],
    });
  });

  it("idle, anything but a press → idle", () => {
    for (const a of [
      frame(ok(noon)),
      release(ok(noon)),
      { type: "cancel", why: "escape" } as const,
    ]) {
      const step = gestureStep(IDLE, a);
      expect(step.state).toBe(IDLE);
      expect(step.effects).toEqual([]);
    }
  });

  it("a press while idle is the only way out of idle", () => {
    const actions: GestureAction[] = [
      frame(ok(noon)),
      release(ok(noon)),
      ...CANCELS.map((why): GestureAction => ({ type: "cancel", why })),
    ];
    for (const a of actions) expect(gestureStep(IDLE, a).state).toBe(IDLE);
    expect(gestureStep(IDLE, { type: "press", press: press(onEmpty()) }).state.phase).toBe(
      "pressed",
    );
  });
});

describe("from pressed", () => {
  it("pressed, frame under the threshold → the same state", () => {
    const s = pressed(press(onEvent()));
    const step = gestureStep(s, frame(ok(noon), false));
    expect(step.state).toBe(s);
    expect(step.effects).toEqual([]);
  });

  it("pressed, frame over the threshold, an event with nothing allowed and no reason → idle", () => {
    // An overlay or the draft: nothing is picked up, and its click still happens.
    const step = gestureStep(pressed(press(onEvent({ cap: NONE }))), frame(null));
    expect(step).toEqual({ state: IDLE, effects: [] });
  });

  it("pressed, frame over the threshold, an event with nothing allowed and a reason → dragging, no plan", () => {
    const p = press(onEvent({ cap: { ...NONE, reason: THEIRS } }));
    expect(gestureStep(pressed(p), frame(ok(noon)))).toEqual({
      state: dragging(p, null),
      effects: [{ effect: "capture" }, { effect: "announce", text: THEIRS }],
    });
  });

  it("an event with no end can't be picked up either", () => {
    const p = press(onEvent({ origin: null, cap: { ...CAN, reason: "Never." } }));
    expect(gestureStep(pressed(p), frame(ok(noon))).state).toEqual(dragging(p, null));
  });

  it("pressed, frame over the threshold, empty where creating isn't allowed → idle", () => {
    expect(gestureStep(pressed(press(onEmpty(null))), frame(null))).toEqual({
      state: IDLE,
      effects: [],
    });
  });

  it("pressed, frame over the threshold, any press made while a popover was open → idle", () => {
    // One rule for every press: it only closes what was open.
    for (const on of [onEmpty(), onEvent()]) {
      const step = gestureStep(pressed(press(on, { dismissOnly: true })), frame(ok(noon)));
      expect(step).toEqual({ state: IDLE, effects: [] });
    }
  });

  it("pressed, frame over the threshold, otherwise → dragging", () => {
    const p = press(onEvent());
    expect(gestureStep(pressed(p), frame(ok(noon)))).toEqual({
      state: dragging(p, ok(noon)),
      effects: [{ effect: "capture" }, { effect: "announce", text: "Moving “Standup”." }],
    });
    // A drag over empty space says nothing: quick create speaks for itself.
    const e = press(onEmpty());
    expect(gestureStep(pressed(e), frame(ok(noon)))).toEqual({
      state: dragging(e, ok(noon)),
      effects: [{ effect: "capture" }],
    });
  });

  it("pressed, release, an event → idle (its own click opens its details)", () => {
    expect(gestureStep(pressed(press(onEvent())), release(ok(noon), false))).toEqual({
      state: IDLE,
      effects: [],
    });
  });

  it("pressed, release, empty where creating is allowed → create from the click", () => {
    // The click's plan, fixed at the press, not the release's.
    const p = press(onEmpty(ok(nine)));
    expect(gestureStep(pressed(p), release(ok(noon), false))).toEqual({
      state: IDLE,
      effects: [{ effect: "create", timing: nine, drag: false }],
    });
  });

  it("pressed, release, empty, made while a popover was open → idle", () => {
    const p = press(onEmpty(), { dismissOnly: true });
    expect(gestureStep(pressed(p), release(null, false))).toEqual({ state: IDLE, effects: [] });
  });

  it("pressed, release, empty where creating isn't allowed → idle", () => {
    expect(gestureStep(pressed(press(onEmpty(null))), release(null, false))).toEqual({
      state: IDLE,
      effects: [],
    });
  });

  it("a click in the hour the clocks skip creates nothing and says why", () => {
    expect(gestureStep(pressed(press(onEmpty(GAP))), release(null, false))).toEqual({
      state: IDLE,
      effects: [{ effect: "announce", text: GAP_WORDS }],
    });
  });

  it("pressed, cancel (any) → idle", () => {
    for (const why of CANCELS) {
      expect(gestureStep(pressed(press(onEvent())), { type: "cancel", why })).toEqual({
        state: IDLE,
        effects: [],
      });
    }
  });
});

describe("a flick (past the threshold between two frames) is a drag, not a click", () => {
  it("moves an event, with nothing to capture or let go of", () => {
    const p = press(onEvent());
    expect(gestureStep(pressed(p), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [
        { effect: "announce", text: "Moving “Standup”." },
        { effect: "swallowClick" },
        { effect: "commit", row: ROW, from: nine, to: noon, resize: false },
      ],
    });
  });

  it("creates over a range from empty space", () => {
    expect(gestureStep(pressed(press(onEmpty(ok(nine)))), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [{ effect: "swallowClick" }, { effect: "create", timing: noon, drag: true }],
    });
  });

  it("picks nothing up where a drag would pick nothing up", () => {
    expect(gestureStep(pressed(press(onEvent({ cap: NONE }))), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [],
    });
    const p = press(onEmpty(), { dismissOnly: true });
    expect(gestureStep(pressed(p), release(ok(noon)))).toEqual({ state: IDLE, effects: [] });
  });

  it("a touch is never a drag: a tap creates, however far it went", () => {
    const tap = press(onEmpty(ok(nine)), { pointerType: "touch" });
    expect(gestureStep(pressed(tap), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [{ effect: "create", timing: nine, drag: false }],
    });
    const onCard = press(onEvent(), { pointerType: "touch" });
    expect(gestureStep(pressed(onCard), frame(ok(noon))).state).toEqual(pressed(onCard));
    expect(gestureStep(pressed(onCard), release(ok(noon)))).toEqual({ state: IDLE, effects: [] });
  });
});

describe("from dragging", () => {
  const p = press(onEvent());

  it("dragging, frame → the same state while the plan is the same", () => {
    const s = dragging(p, ok(noon));
    const same = gestureStep(s, frame(ok({ ...noon })));
    expect(same.state).toBe(s);
    expect(same.effects).toEqual([]);
    const moved = gestureStep(s, frame(ok(nine, true)));
    expect(moved).toEqual({ state: dragging(p, ok(nine, true)), effects: [] });
  });

  it("a frame with no plan keeps the last one", () => {
    const s = dragging(p, ok(noon));
    expect(gestureStep(s, frame(null)).state).toBe(s);
    const fresh = dragging(p, null);
    expect(gestureStep(fresh, frame(null)).state).toBe(fresh);
  });

  it("a press that may do nothing stays planless, whatever a frame says", () => {
    const s = dragging(press(onEvent({ cap: { ...NONE, reason: THEIRS } })), null);
    expect(gestureStep(s, frame(ok(noon))).state).toBe(s);
  });

  it("dragging, release, a plan that changes something, on an event → commit", () => {
    expect(gestureStep(dragging(p, ok(nine)), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [
        { effect: "uncapture" },
        { effect: "swallowClick" },
        { effect: "commit", row: ROW, from: nine, to: noon, resize: false },
      ],
    });
    const edge = press(onEvent({ grab: "end" }));
    const step = gestureStep(dragging(edge, null), release(ok(noon)));
    expect(step.effects).toContainEqual({
      effect: "commit",
      row: ROW,
      from: nine,
      to: noon,
      resize: true,
    });
  });

  it("the commit carries the row as it was pressed", () => {
    const step = gestureStep(dragging(p, null), release(ok(noon)));
    const commit = step.effects.find((e) => e.effect === "commit");
    expect(commit && commit.effect === "commit" && commit.row).toBe(ROW);
  });

  it("a release with no plan of its own lets go on the last one", () => {
    const step = gestureStep(dragging(p, ok(noon)), release(null));
    expect(step.effects).toContainEqual({
      effect: "commit",
      row: ROW,
      from: nine,
      to: noon,
      resize: false,
    });
  });

  it("dragging, release, a plan that changes something, over empty space → create", () => {
    const e = press(onEmpty());
    expect(gestureStep(dragging(e, ok(nine)), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [
        { effect: "uncapture" },
        { effect: "swallowClick" },
        { effect: "create", timing: noon, drag: true },
      ],
    });
  });

  it("dragging, release, no change → idle, nothing sent", () => {
    expect(gestureStep(dragging(p, ok(noon)), release(ok(nine, true)))).toEqual({
      state: IDLE,
      effects: [{ effect: "uncapture" }, { effect: "swallowClick" }],
    });
  });

  it("dragging, release, in the hour the clocks skip → says why, nothing sent", () => {
    for (const on of [onEvent(), onEmpty()]) {
      expect(gestureStep(dragging(press(on), ok(noon)), release(GAP))).toEqual({
        state: IDLE,
        effects: [
          { effect: "uncapture" },
          { effect: "swallowClick" },
          { effect: "announce", text: GAP_WORDS },
        ],
      });
    }
  });

  it("dragging, release, an event with nothing allowed → idle (the pointer said why)", () => {
    const stuck = press(onEvent({ cap: { ...NONE, reason: THEIRS } }));
    expect(gestureStep(dragging(stuck, null), release(ok(noon)))).toEqual({
      state: IDLE,
      effects: [{ effect: "uncapture" }, { effect: "swallowClick" }],
    });
  });

  it("dragging, release, outside or with no plan → put back", () => {
    const putBack = {
      state: IDLE,
      effects: [
        { effect: "uncapture" },
        { effect: "swallowClick" },
        { effect: "announce", text: PUT_BACK },
      ],
    };
    expect(gestureStep(dragging(p, ok(noon)), release(OUTSIDE))).toEqual(putBack);
    // Never planned anything (let go before a frame did).
    expect(gestureStep(dragging(p, null), release(null))).toEqual(putBack);
    // A drag over empty space just ends.
    expect(gestureStep(dragging(press(onEmpty()), ok(noon)), release(OUTSIDE))).toEqual({
      state: IDLE,
      effects: [{ effect: "uncapture" }, { effect: "swallowClick" }],
    });
  });

  it("dragging, cancel (any) → idle, put back", () => {
    for (const why of CANCELS) {
      expect(gestureStep(dragging(p, ok(noon)), { type: "cancel", why })).toEqual({
        state: IDLE,
        effects: [
          { effect: "uncapture" },
          { effect: "swallowClick" },
          { effect: "announce", text: PUT_BACK },
        ],
      });
    }
  });

  it("a cancelled create, or a press that couldn't move, says nothing", () => {
    const quiet = [
      dragging(press(onEmpty()), ok(noon)),
      dragging(press(onEvent({ cap: { ...NONE, reason: THEIRS } })), null),
    ];
    for (const s of quiet) {
      expect(gestureStep(s, { type: "cancel", why: "escape" })).toEqual({
        state: IDLE,
        effects: [{ effect: "uncapture" }, { effect: "swallowClick" }],
      });
    }
  });

  it("a second press mid-drag changes nothing", () => {
    const s = dragging(p, ok(noon));
    expect(gestureStep(s, { type: "press", press: press(onEmpty()) })).toEqual({
      state: s,
      effects: [],
    });
    const q = pressed(p);
    expect(gestureStep(q, { type: "press", press: press(onEmpty()) }).state).toBe(q);
  });
});

describe("the rules a drag keeps", () => {
  it("pointercancel, blur, a hidden window and a lost capture never commit", () => {
    const p = press(onEvent());
    for (const why of CANCELS) {
      const { effects } = gestureStep(dragging(p, ok(noon)), { type: "cancel", why });
      expect(effects.some((e) => e.effect === "commit" || e.effect === "create")).toBe(false);
    }
    const e = press(onEmpty());
    for (const why of CANCELS) {
      const { effects } = gestureStep(dragging(e, ok(noon)), { type: "cancel", why });
      expect(effects.some((x) => x.effect === "commit" || x.effect === "create")).toBe(false);
    }
  });

  it("a step that changes nothing returns the same state", () => {
    const p = press(onEvent());
    const still = [
      [IDLE, frame(ok(noon))],
      [pressed(p), frame(ok(noon), false)],
      [dragging(p, ok(noon)), frame(ok(noon))],
      [dragging(p, ok(noon)), frame(null)],
    ] as const;
    for (const [s, a] of still) expect(gestureStep(s, a).state).toBe(s);
  });

  it("a press on something that can't move and says nothing picks nothing up", () => {
    // An overlay pressed past the threshold: idle with no effects, so its click isn't eaten.
    const s = pressed(press(onEvent({ cap: NONE })));
    const step = gestureStep(s, frame(null));
    expect(step.state).toBe(IDLE);
    expect(step.effects).toEqual([]);
  });

  it("names the event it picks up", () => {
    expect(movingText("Standup")).toBe("Moving “Standup”.");
    expect(movingText("  ")).toBe("Moving “(no title)”.");
  });
});
