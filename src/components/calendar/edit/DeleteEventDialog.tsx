// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Delete this event?" (#884). Names the event, its calendar and its time, and says what Undo can
// and can't do: the delete waits a few seconds in PM, so quitting PM in that time means it never
// happens. Initial focus is on Cancel (ConfirmDialog's first button), so Enter can't delete.
//
// A repeating event asks which occurrences first, like Google Calendar; the held Undo still
// applies. The choices are read from Google when the dialog opens, against what the user saw of the
// row, and the delete sends the same, since the token binds it.
//
// An event with guests you organise also asks whether Google emails them the cancellation (C12),
// in this dialog: Delete stays off until Send or Don't send is picked, and the answer goes with the
// hold, so it's the one sent after the Undo window. A one-off's numbers come from its row, or from
// the backend when it asked again; a repeating event's rows carry their own.

import { useEffect, useMemo, useRef, useState } from "react";
import type {
  CalendarEvent,
  GuestNotice,
  Notify,
  SeriesChoices,
  SplitAction,
} from "../../../lib/types";
import {
  deleteText,
  SPECIAL_DATE_DELETE,
  specialKind,
} from "../../../lib/calendarEdit/editReasons";
import { deleteNotice, notifyOf } from "../../../lib/calendarEdit/guests";
import { isRepeating, type Picked } from "../../../lib/calendarEdit/saveChoices";
import { parseLocal } from "../../../lib/calendar-layout";
import { formatDateLocal } from "../../../lib/format";
import { Callout, ConfirmDialog } from "../../ui";
import { NotifyChoice } from "./NotifyChoice";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";
import { choicesForDelete, noteSeriesNotice, settleSplit } from "./useEventWrites";

interface Props {
  event: CalendarEvent | null;
  /** The event's calendar, as the list names it. */
  calendarName: string | null;
  /** When it happens, as the popover said it. */
  when: string;
  /** The account the delete goes through: who Google sends guests' emails from, and never counted
   *  as a guest. */
  from?: string | null;
  /** Re-asked after a delete answered that the plan changed since the choice: the dialog says so. */
  again?: boolean;
  /** Re-asked because the backend wants the occurrences chosen, though the row looked like a
   *  one-off: ask, whatever the row says. */
  forceRepeating?: boolean;
  /** Re-asked because the backend wants Send or Don't send: its numbers, and `again` when an
   *  answer came for others. Asked as a one-off. */
  guests?: { notice: GuestNotice; again: boolean } | null;
  /** The delete: the occurrences picked (a repeating event), Send or Don't send when it would email
   *  guests, and the numbers a one-off's question showed (null when nothing was asked). */
  onConfirm: (
    event: CalendarEvent,
    picked: Picked | null,
    notify: Notify | null,
    asked: GuestNotice | null,
  ) => void;
  onClose: () => void;
}

export function DeleteEventDialog({
  event,
  calendarName,
  when,
  from = null,
  again = false,
  forceRepeating = false,
  guests = null,
  onConfirm,
  onClose,
}: Props) {
  if (event && !guests && (forceRepeating || isRepeating(event))) {
    return (
      <RepeatingDelete
        key={event.id}
        event={event}
        calendarName={calendarName}
        when={when}
        from={from}
        again={again}
        onConfirm={onConfirm}
        onClose={onClose}
      />
    );
  }
  return (
    <PlainDelete
      event={event}
      calendarName={calendarName}
      when={when}
      from={from}
      guests={guests}
      onConfirm={onConfirm}
      onClose={onClose}
    />
  );
}

/** The one-event confirm, with the email question when it has guests to tell. */
function PlainDelete({
  event,
  calendarName,
  when,
  from,
  guests,
  onConfirm,
  onClose,
}: Pick<Props, "event" | "calendarName" | "when" | "onConfirm" | "onClose"> & {
  from: string | null;
  guests: { notice: GuestNotice; again: boolean } | null;
}) {
  const title = event ? event.summary.trim() || "(no title)" : "";
  const note = event ? deleteNoteOf(event) : null;
  // The backend's numbers when it asked again, else the row's own (null: nobody to tell).
  const notice = useMemo(
    () => guests?.notice ?? (event ? deleteNotice(event, from) : null),
    [guests, event, from],
  );
  // Send or Don't send, for the numbers on screen: none until picked, and none again for others.
  const [notify, setNotify] = useState<Notify | null>(null);
  const [answeredFor, setAnsweredFor] = useState(notice);
  if (notice !== answeredFor) {
    setAnsweredFor(notice);
    setNotify(null);
  }
  return (
    <ConfirmDialog
      open={event !== null}
      title="Delete this event?"
      confirmLabel="Delete"
      danger
      confirmDisabled={notice !== null && notify === null}
      initialFocus="cancel"
      helpId="calendar-delete"
      // Delete is off until answered; were it pressed anyway, the backend asks (R9). Closing
      // clears the event, so the next one asks afresh.
      onConfirm={() =>
        event && onConfirm(event, null, notice && notify ? notifyOf(notify) : null, notice)
      }
      onClose={onClose}
    >
      <p>
        <span className="text-ink">“{title}”</span> ({when}) will be deleted from{" "}
        {calendarName ? <span className="text-ink">{calendarName}</span> : "its calendar"} in Google
        Calendar.
      </p>
      <p className="mt-2">
        You can undo it for a few seconds. If you quit PM in that time, nothing is deleted.
        {note && ` ${note}`}
      </p>
      {/* Reopened by the backend: the delete didn't happen, which is said first, whether or not
          anyone had answered (C12c review). Neutral, as the editor's question is: the backend knows
          only that its numbers differ, not why (C12 critique C2-L6). With no answer yet it may be
          asking from PM's older row, after Google dropped the guests, so it doesn't say the event
          "has guests now"; the send checks Google's copy either way (C12c review round 2). */}
      {guests && (
        <Callout tone="warning" size="md" className="mt-3">
          {guests.again
            ? "It wasn't deleted. PM checked with Google before deleting, and these are the numbers it found."
            : "It wasn't deleted. PM found guests who would be told. Choose whether Google emails them."}
        </Callout>
      )}
      {notice && (
        <NotifyChoice
          notice={notice}
          kind="delete"
          scope={null}
          from={from}
          value={notify}
          onChange={setNotify}
          disabled={false}
          legend="shown"
        />
      )}
    </ConfirmDialog>
  );
}

/** A repeating event's delete: the choices, then the delete for the occurrences picked. */
function RepeatingDelete({
  event,
  calendarName,
  when,
  from,
  again,
  onConfirm,
  onClose,
}: {
  event: CalendarEvent;
  calendarName: string | null;
  when: string;
  from: string | null;
  again: boolean;
  onConfirm: Props["onConfirm"];
  onClose: () => void;
}) {
  const title = event.summary.trim() || "(no title)";
  const [view, setView] = useState<ChoicesView>({ state: "loading" });
  const [settling, setSettling] = useState(false);
  // Words the backend says once (C9a-3b-2): shown here, and kept as a notice if the dialog closes
  // without a choice, so they aren't lost.
  const notice = useRef<string | null>(null);
  const mounted = useRef(false);
  const answered = useRef<SeriesChoices | null>(null);

  useEffect(() => {
    mounted.current = true;
    // StrictMode runs this twice: the store asks once, and the answer is applied once (so words
    // that arrive after the dialog closed become one notice, not two). `mounted` is the
    // component's, not this run's: StrictMode's first run is cleaned up but the dialog is still up.
    const apply = (c: SeriesChoices) => {
      if (answered.current === c) return;
      answered.current = c;
      const said = c.outcome === "ready" ? (c.notice ?? null) : null;
      if (!mounted.current) {
        if (said) noteSeriesNotice(said);
        return;
      }
      switch (c.outcome) {
        case "not_repeating":
          // PM's calendar says it repeats (or the backend did, asking for a choice) and Google says
          // it doesn't. The delete is checked against PM's calendar, which still says it repeats, so
          // a one-event confirm would only be asked for a choice again: refresh first (C9b-1
          // critique 2 L8, C9b-1b review F7).
          setView({
            state: "refused",
            tone: "warn",
            text: "Refresh the calendar, then try again.",
          });
          return;
        case "refused":
          // A conflict has already put Google's copy in PM's calendar behind this dialog.
          setView({ state: "refused", ...deleteText(c.result, title) });
          return;
        case "ready":
          notice.current = said;
          setView({ state: "ready", choices: c, again, notice: said });
          return;
        default:
          return unreachable(c);
      }
    };
    choicesForDelete(event).then(apply, (e: unknown) => {
      if (!mounted.current) return;
      const message = e instanceof Error ? e.message : String(e);
      setView({ state: "refused", ...deleteText({ outcome: "failed", message }, title) });
    });
    return () => {
      mounted.current = false;
    };
  }, [event, again, title]);

  /** Every way out but a choice: the words said once go to a notice first. */
  const close = () => {
    if (notice.current) noteSeriesNotice(notice.current);
    notice.current = null;
    onClose();
  };

  const onSettle = async (token: string, action: SplitAction) => {
    setSettling(true);
    await settleSplit(token, action, title);
    // Closed meanwhile (Cancel stays open while it settles): its words went to a notice then, and
    // closing again would close whatever delete dialog is open now.
    if (!mounted.current) return;
    setSettling(false);
    // The choices were planned before the settle, and its notice says how it went: the next
    // delete asks again.
    close();
  };

  const start = parseLocal(event.start, event.all_day);
  return (
    <SaveChoicesDialog
      kind="delete"
      title={title}
      view={view}
      settling={settling}
      when={when}
      calendarName={calendarName}
      day={start ? formatDateLocal(start) : undefined}
      from={from}
      deleteNote={deleteNoteOf(event)}
      // The token binds who each row tells, so no numbers go with the answer.
      onChoose={(picked, notify) => onConfirm(event, picked, notify, null)}
      onSettle={(token, action) => void onSettle(token, action)}
      onClose={close}
    />
  );
}

/** What deleting `event` adds beside Undo: a birthday's, that Google Contacts and the Google
 *  Account keep it (C20 §6.5). The choices for a repeating one come from the backend, which refuses
 *  "This event" in its own words. */
function deleteNoteOf(event: CalendarEvent): string | null {
  return specialKind(event.event_type) === "special_date" ? SPECIAL_DATE_DELETE : null;
}

function unreachable(value: never): never {
  throw new Error(`unhandled choices answer: ${JSON.stringify(value)}`);
}
