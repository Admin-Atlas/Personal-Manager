// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Repeat row's two reads (#884, C11): the repeat menu's ready-made rules for the event's start
// date, and what the backend says of the chosen rule at the event's start (its words, the day it
// really starts on, or why it can't be saved). Both are pure on the backend and cheap, so they're
// asked on every change, undebounced; only the newest answer is ever shown, since an older one
// arriving late describes a start or rule the editor no longer holds.

import { useCallback, useEffect, useRef, useState } from "react";
import { describeRecurrence, recurrencePresets } from "../../../lib/ipc";
import type { RepeatPreset, RepeatRule, RepeatStart } from "../../../lib/types";
import type { RepeatValue } from "../../../lib/calendarEdit/recurrenceDraft";

/** What the backend says of the Repeat row's rule. `none`: no repeat (or one PM can only show), so
 *  nothing to ask. */
export type RepeatState =
  | { state: "none" }
  | { state: "loading" }
  | { state: "ready"; summary: string; starts: string; moved: boolean }
  | { state: "refused"; message: string };

/** Said when the read itself failed, rather than a crash or a rule that looks fine. */
export const REPEAT_UNCHECKED = "PM couldn't check this repeat. Try again.";

/** `ask()`, with a throw before it returns a promise read as a failed promise. */
function attempt<T>(ask: () => Promise<T>): Promise<T> {
  return new Promise<T>((resolve) => resolve(ask()));
}

/** What a rule at a start is asked as (the key both the read and a save compare), or null when
 *  there's nothing to ask: no repeat, one PM can only show, or no start yet. */
function repeatAsk(value: RepeatValue, start: RepeatStart | null): string | null {
  return value.kind === "rule" && start ? JSON.stringify([value.rule, start]) : null;
}

/** The Repeat row's rule as the backend sees it, and a save's way to ask the same. */
export interface RepeatCheck {
  state: RepeatState;
  /** Whether the newest answer says `value` at `start` can be saved: no repeat, or a rule the backend
   *  called ready at exactly that start. A save reads its draft as it is then (a date the blur before
   *  it just committed is a start not yet asked about), so it asks this rather than `state`, which
   *  is the last render's. */
  saveable: (value: RepeatValue, start: RepeatStart | null) => boolean;
}

/** The repeat menu's ready-made rules for `day` (YYYY-MM-DD), newest answer only; null while
 *  loading or with no day. A failed read gives none: the menu still has "Does not repeat" and
 *  "Custom…". */
export function usePresets(day: string | null): RepeatPreset[] | null {
  const [answer, setAnswer] = useState<{ day: string; presets: RepeatPreset[] } | null>(null);
  const seq = useRef(0);
  useEffect(() => {
    const mine = ++seq.current;
    if (!day) return;
    const apply = (presets: RepeatPreset[]) => {
      if (seq.current === mine) setAnswer({ day, presets });
    };
    attempt(() => recurrencePresets(day)).then(
      (presets) => apply(Array.isArray(presets) ? presets : []),
      () => apply([]),
    );
  }, [day]);
  // Another day's presets aren't this day's: loading until its own answer.
  return day && answer?.day === day ? answer.presets : null;
}

/** What the backend says of `value` at `start`: its words, the day it really starts on, or why it
 *  can't be saved. Loading until the answer for exactly this rule and start arrives. */
export function useRepeatView(value: RepeatValue, start: RepeatStart | null): RepeatCheck {
  const ask = repeatAsk(value, start);
  const [answer, setAnswer] = useState<{ ask: string; state: RepeatState } | null>(null);
  // The same answer, readable at once by a save in the middle of an event.
  const answerRef = useRef<{ ask: string; state: RepeatState } | null>(null);
  const seq = useRef(0);
  useEffect(() => {
    const mine = ++seq.current;
    if (ask === null) return;
    const [rule, at] = JSON.parse(ask) as [RepeatRule, RepeatStart];
    const apply = (state: RepeatState) => {
      if (seq.current !== mine) return;
      answerRef.current = { ask, state };
      setAnswer(answerRef.current);
    };
    const unchecked: RepeatState = { state: "refused", message: REPEAT_UNCHECKED };
    attempt(() => describeRecurrence(rule, at)).then(
      (view) =>
        apply(
          view?.outcome === "ready"
            ? { state: "ready", summary: view.summary, starts: view.starts, moved: view.moved }
            : view?.outcome === "refused"
              ? { state: "refused", message: view.message }
              : unchecked,
        ),
      () => apply(unchecked),
    );
  }, [ask]);
  const saveable = useCallback((v: RepeatValue, s: RepeatStart | null) => {
    const asked = repeatAsk(v, s);
    const got = answerRef.current;
    return asked === null || (got !== null && got.ask === asked && got.state.state === "ready");
  }, []);
  const state: RepeatState =
    ask === null ? { state: "none" } : answer?.ask === ask ? answer.state : { state: "loading" };
  return { state, saveable };
}
