// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's drops (#884, C16b): what happens the moment an event is let go somewhere new. It is
// drawn there at once (the write hook's pending look). A drop that asks nothing is sent straight
// away and never waits on a dialog, so a second drop while the first saves is free to go; one that
// asks (which occurrences of a repeating event, whether to email guests) is handed to `MoveFlow`,
// one at a time. A quiet drop whose answer asks after all (Google's copy repeats, or has guests,
// which PM's calendar didn't show) is asked then if nothing else is open, and otherwise goes back
// with a notice to drag it again, as a delete's late question does (C16 critique C1-H2: one flow per
// drop, each closing only its own).

import { useCallback, useEffect, useRef, useState } from "react";
import type { CalendarEvent, WriteOutcome } from "../../../lib/types";
import { moveText } from "../../../lib/calendarEdit/editReasons";
import { moveNotice } from "../../../lib/calendarEdit/guests";
import { dropChoicesNeeded } from "../../../lib/calendarDrag";
import type { GestureEffect } from "../../../lib/calendarGesture";
import {
  beginMove,
  cancelMove,
  dropOf,
  moveEvent,
  noteSeriesNotice,
  type Drop,
} from "./useEventWrites";

type Commit = Extract<GestureEffect, { effect: "commit" }>;

/** A drop to ask about. */
export interface MoveAsk {
  drop: Drop;
  /** `beginMove`'s token for it: its pending entry lasts while this flow does. */
  flow: string;
  /** The question a move sent without one came back with: Google's copy repeats, or has guests,
   *  which PM's calendar didn't show. */
  start?: Extract<WriteOutcome, { outcome: "needs_choice" }>;
}

/** What the calendar tells the drops. */
export interface DropsHost {
  /** The account a row's writes go through: who Google sends its guests' emails from. */
  accountOf: (ev: CalendarEvent) => string | null;
  /** Nothing is open that a question would land on top of (a dialog, the event's details). */
  free: () => boolean;
  /** Focus the moved event, as an edited one is after its save. */
  focus: (eventId: string) => void;
}

/** The calendar's drops, and the one being asked about (for `MoveFlow`), if any. */
export function useMoveDrops(host: DropsHost): {
  ask: MoveAsk | null;
  drop: (e: Commit) => void;
  /** `mine` has ended: closed, unless a newer drop has taken its place. */
  end: (mine: MoveAsk) => void;
} {
  const hostRef = useRef(host);
  hostRef.current = host;
  const [ask, setAsk] = useState<MoveAsk | null>(null);
  // What an answer that comes later reads.
  const askRef = useRef<MoveAsk | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const open = useCallback((a: MoveAsk) => {
    askRef.current = a;
    setAsk(a);
  }, []);
  const end = useCallback((mine: MoveAsk) => {
    if (askRef.current !== mine) return;
    askRef.current = null;
    setAsk(null);
  }, []);
  const drop = useCallback(
    (e: Commit) => {
      const h = hostRef.current;
      const d = dropOf(e.row, e.from, e.to, e.resize);
      const flow = beginMove(d);
      const needs = dropChoicesNeeded(e.row, moveNotice(e.row, h.accountOf(e.row)));
      if (needs.scope || needs.guests) {
        open({ drop: d, flow });
        return;
      }
      h.focus(d.event.id);
      void moveEvent(d, null, null, null).then((outcome) => {
        if (outcome.outcome !== "needs_choice") return;
        if (mounted.current && askRef.current === null && hostRef.current.free()) {
          open({ drop: d, flow, start: outcome });
          return;
        }
        cancelMove(d.event.id);
        noteSeriesNotice(moveText(outcome, d.event.summary, "", null).text);
      });
    },
    [open],
  );
  return { ask, drop, end };
}
