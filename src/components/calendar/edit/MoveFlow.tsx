// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What PM asks after an event is dropped somewhere new (#884, C16b), with the dialogs every other
// change asks with: which occurrences of a repeating event (`SaveChoicesDialog`, "This event" picked,
// focus on Back, the email question inside it where a row would email guests), or, for one event
// with guests, whether Google emails them (`NotifyPrompt`, nothing picked). Google asks the same after
// a drag. Meanwhile the event is drawn where it was dropped; Back or Escape puts it back.
//
// A drop that asks nothing is sent straight away (`useMoveDrops`) and never comes here, so a second
// drop while the first saves has nothing to wait for. One whose answer is a question PM's calendar
// didn't know to ask (Google's copy repeats, or has guests) comes here with that question. A question
// is asked again once; a second, or guests changing after the occurrences were chosen (the choice
// binds whom it tells), puts the event back with a notice to drag it again, as a delete says to
// delete again. The flow is built on the delete's (`DeleteEventDialog`'s repeating path and
// `useDeleteDialog`): the same answers to the choices, words said once kept as a notice, settling an
// earlier change. It asks and sends through the write hook only, which says how each move ended.

import { useEffect, useRef, useState } from "react";
import type {
  GuestNotice,
  Notify,
  SeriesChoices,
  SplitAction,
  WriteOutcome,
} from "../../../lib/types";
import {
  A11_NOTE,
  MOVE_CHECKED,
  MOVE_NEW_GUESTS,
  moveChangedText,
  moveText,
} from "../../../lib/calendarEdit/editReasons";
import { moveNotice } from "../../../lib/calendarEdit/guests";
import { isRepeating, type Picked } from "../../../lib/calendarEdit/saveChoices";
import { PUT_BACK } from "../../../lib/calendarGesture";
import { NotifyPrompt } from "./NotifyPrompt";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";
import {
  cancelMove,
  choicesForMove,
  moveEvent,
  noteSeriesNotice,
  settleSplit,
} from "./useEventWrites";
import type { MoveAsk } from "./useMoveDrops";

interface Props {
  ask: MoveAsk;
  /** The account the move goes through: who Google sends guests' emails from, never counted as a
   *  guest. */
  account: string | null;
  /** Say something in the calendar's live region ("Put back."). */
  announce: (text: string) => void;
  /** The flow has ended: the move was sent, or the event put back. */
  onDone: () => void;
}

/** What is on screen. */
type Step =
  | { kind: "choices"; again: boolean }
  | { kind: "notify"; notice: GuestNotice; again: boolean; againText?: string }
  /** Sent, waiting for the answer: nothing is shown. */
  | { kind: "sending" };

/** What a drop asks first. */
function firstStep(ask: MoveAsk, account: string | null): Step {
  const s = ask.start;
  if (s) {
    return s.guests && !s.scope
      ? { kind: "notify", notice: s.guests, again: true, againText: MOVE_NEW_GUESTS }
      : { kind: "choices", again: false };
  }
  if (isRepeating(ask.drop.event)) return { kind: "choices", again: false };
  const notice = moveNotice(ask.drop.event, account);
  return notice ? { kind: "notify", notice, again: false } : { kind: "sending" };
}

/** Rendered while a drop is being asked about; keyed by its flow, so each drop starts afresh. */
export function MoveFlow({ ask, account, announce, onDone }: Props) {
  const { drop } = ask;
  const event = drop.event;
  const title = event.summary.trim() || "(no title)";
  const [step, setStep] = useState<Step>(() => firstStep(ask, account));
  const [view, setView] = useState<ChoicesView>({ state: "loading" });
  const [settling, setSettling] = useState(false);
  // Words the choices say once (a change PM was making when it closed): kept as a notice if the
  // dialog closes without a choice, so they aren't lost.
  const said = useRef<string | null>(null);
  const answered = useRef<SeriesChoices | null>(null);
  const mounted = useRef(false);
  // The flow is over: sent, or put back. Until then, going away puts the event back.
  const ended = useRef(false);
  // A question came back once already (a move sent without one counts): the next puts it back.
  const reasked = useRef(ask.start !== undefined);
  // Held for the answer's handler, which outlives the render that sent.
  const props = useRef({ announce, onDone });
  props.current = { announce, onDone };

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // StrictMode unmounts and mounts again at once; only staying unmounted (the tab went, or
      // the calendar) is the flow going away unanswered.
      setTimeout(() => {
        if (mounted.current || ended.current) return;
        ended.current = true;
        if (said.current) noteSeriesNotice(said.current);
        cancelMove(event.id);
      }, 0);
    };
  }, [event.id]);

  /** Back, Escape, or a settle: the event goes back where it was. */
  const back = () => {
    if (ended.current) return;
    ended.current = true;
    if (said.current) noteSeriesNotice(said.current);
    said.current = null;
    cancelMove(event.id);
    props.current.announce(PUT_BACK);
    props.current.onDone();
  };

  /** Put back with a notice to drag it again. */
  const giveUp = (text: string) => {
    ended.current = true;
    cancelMove(event.id);
    noteSeriesNotice(text);
    if (mounted.current) props.current.onDone();
  };

  const send = (picked: Picked | null, notify: Notify | null, asked: GuestNotice | null) => {
    if (ended.current) return;
    ended.current = true;
    said.current = null;
    setStep({ kind: "sending" });
    void moveEvent(drop, picked, notify, asked).then((outcome) =>
      afterSend(outcome, picked, asked),
    );
  };

  const afterSend = (outcome: WriteOutcome, picked: Picked | null, asked: GuestNotice | null) => {
    // Anything but a question has been said by the write hook, and the event is where it landed or
    // back where it was.
    if (outcome.outcome !== "needs_choice") {
      if (mounted.current) props.current.onDone();
      return;
    }
    // Gone meanwhile: nothing can ask now.
    if (!mounted.current) {
      giveUp(moveText(outcome, title, "", null).text);
      return;
    }
    // Asked twice, or guests after the occurrences were chosen (their token binds whom it tells):
    // the plan keeps changing under the user.
    if (
      reasked.current ||
      (picked !== null && !outcome.scope) ||
      (!outcome.scope && !outcome.guests)
    ) {
      giveUp(moveChangedText(title).text);
      return;
    }
    reasked.current = true;
    ended.current = false;
    if (outcome.scope) {
      answered.current = null;
      setView({ state: "loading" });
      setStep({ kind: "choices", again: picked !== null });
      return;
    }
    if (outcome.guests) {
      setStep({
        kind: "notify",
        notice: outcome.guests,
        again: true,
        againText: asked ? MOVE_CHECKED : MOVE_NEW_GUESTS,
      });
    }
  };

  // A drop that turns out to ask nothing (it doesn't repeat any more, and has no guests): sent.
  const oneOff = () => {
    const notice = moveNotice(event, account);
    if (notice) setStep({ kind: "notify", notice, again: false });
    else send(null, null, null);
  };

  // The choices, asked with exactly what the move sends (the token binds it), each time the dialog
  // opens. StrictMode runs this twice: the write hook asks once, and the answer is applied once.
  const asking = step.kind === "choices" ? step : null;
  useEffect(() => {
    if (!asking) return;
    const apply = (c: SeriesChoices) => {
      if (answered.current === c || !mounted.current) return;
      answered.current = c;
      switch (c.outcome) {
        case "not_repeating":
          // Google says it doesn't repeat: the move's own check is against Google's copy, so it
          // goes as one event.
          oneOff();
          return;
        case "refused":
          // A changed time has already put Google's copy in PM's calendar behind the dialog.
          setView({ state: "refused", ...moveText(c.result, title, "", null) });
          return;
        case "ready":
          said.current = c.notice ?? null;
          setView({ state: "ready", choices: c, again: asking.again, notice: said.current });
          return;
      }
    };
    choicesForMove(drop).then(apply, (e: unknown) => {
      if (!mounted.current) return;
      const message = e instanceof Error ? e.message : String(e);
      setView({ state: "refused", ...moveText({ outcome: "failed", message }, title, "", null) });
    });
    // `oneOff` and `title` follow the drop, which keys this flow.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asking, drop]);

  // A drop with nothing to ask (only reached when the calendar hands one over anyway): sent.
  const first = useRef(step);
  useEffect(() => {
    if (first.current.kind === "sending" && !ended.current) send(null, null, null);
    // Once, for the step the flow started on.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onSettle = async (token: string, action: SplitAction) => {
    setSettling(true);
    await settleSplit(token, action, title);
    if (!mounted.current) return;
    setSettling(false);
    // The choices were planned before the settle, whose notice says how it went: drag it again.
    back();
  };

  if (step.kind === "choices") {
    return (
      <SaveChoicesDialog
        kind="save"
        title={title}
        view={view}
        settling={settling}
        from={account}
        // The token binds whom each row tells, so no numbers go with the answer.
        onChoose={(picked, notify) => send(picked, notify, null)}
        onSettle={(token, action) => void onSettle(token, action)}
        onClose={back}
      />
    );
  }
  if (step.kind === "notify") {
    return (
      <NotifyPrompt
        notice={step.notice}
        kind="save"
        from={account}
        again={step.again}
        againText={step.againText}
        // Google's warning on someone else's event they let guests change, as the editor says it.
        note={event.organizer_self === false && event.guests_can_modify === true ? A11_NOTE : null}
        onSend={(n) => send(null, n, step.notice)}
        onBack={back}
      />
    );
  }
  return null;
}
