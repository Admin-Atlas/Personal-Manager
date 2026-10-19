// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Delete this event?" (#884). Names the event, its calendar and its time, and says what Undo can
// and can't do: the delete waits a few seconds in PM, so quitting PM in that time means it never
// happens. Initial focus is on Cancel (ConfirmDialog's first button), so Enter can't delete.
//
// A repeating event asks which occurrences first, like Google Calendar; the held Undo still
// applies. The choices are read from Google when the dialog opens, against what the user saw of the
// row, and the delete sends the same, since the token binds it.

import { useEffect, useRef, useState } from "react";
import type { CalendarEvent, SeriesChoices, SplitAction } from "../../../lib/types";
import { deleteText } from "../../../lib/calendarEdit/editReasons";
import { isRepeating, type Picked } from "../../../lib/calendarEdit/saveChoices";
import { parseLocal } from "../../../lib/calendar-layout";
import { formatDateLocal } from "../../../lib/format";
import { ConfirmDialog } from "../../ui";
import { SaveChoicesDialog, type ChoicesView } from "./SaveChoicesDialog";
import { choicesForDelete, noteSeriesNotice, settleSplit } from "./useEventWrites";

interface Props {
  event: CalendarEvent | null;
  /** The event's calendar, as the list names it. */
  calendarName: string | null;
  /** When it happens, as the popover said it. */
  when: string;
  /** Re-asked after a delete answered that the plan changed since the choice: the dialog says so. */
  again?: boolean;
  /** Re-asked because the backend wants the occurrences chosen, though the row looked like a
   *  one-off: ask, whatever the row says. */
  forceRepeating?: boolean;
  onConfirm: (event: CalendarEvent, picked: Picked | null) => void;
  onClose: () => void;
}

export function DeleteEventDialog({
  event,
  calendarName,
  when,
  again = false,
  forceRepeating = false,
  onConfirm,
  onClose,
}: Props) {
  if (event && (forceRepeating || isRepeating(event))) {
    return (
      <RepeatingDelete
        key={event.id}
        event={event}
        calendarName={calendarName}
        when={when}
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
      onConfirm={onConfirm}
      onClose={onClose}
    />
  );
}

/** The one-event confirm. */
function PlainDelete({
  event,
  calendarName,
  when,
  onConfirm,
  onClose,
}: Pick<Props, "event" | "calendarName" | "when" | "onConfirm" | "onClose">) {
  const title = event ? event.summary.trim() || "(no title)" : "";
  return (
    <ConfirmDialog
      open={event !== null}
      title="Delete this event?"
      confirmLabel="Delete"
      danger
      onConfirm={() => event && onConfirm(event, null)}
      onClose={onClose}
    >
      <p>
        <span className="text-ink">“{title}”</span> ({when}) will be deleted from{" "}
        {calendarName ? <span className="text-ink">{calendarName}</span> : "its calendar"} in Google
        Calendar.
      </p>
      <p className="mt-2">
        You can undo it for a few seconds. If you quit PM in that time, nothing is deleted.
      </p>
    </ConfirmDialog>
  );
}

/** A repeating event's delete: the choices, then the delete for the occurrences picked. */
function RepeatingDelete({
  event,
  calendarName,
  when,
  again,
  onConfirm,
  onClose,
}: {
  event: CalendarEvent;
  calendarName: string | null;
  when: string;
  again: boolean;
  onConfirm: (event: CalendarEvent, picked: Picked | null) => void;
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
      onChoose={(picked) => onConfirm(event, picked)}
      onSettle={(token, action) => void onSettle(token, action)}
      onClose={close}
    />
  );
}

function unreachable(value: never): never {
  throw new Error(`unhandled choices answer: ${JSON.stringify(value)}`);
}
