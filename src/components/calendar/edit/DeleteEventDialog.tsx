// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "Delete this event?" (#884). Names the event, its calendar and its time, and says what Undo can
// and can't do: the delete waits a few seconds in PM, so quitting PM in that time means it never
// happens. Initial focus is on Cancel (ConfirmDialog's first button), so Enter can't delete.

import type { CalendarEvent } from "../../../lib/types";
import { ConfirmDialog } from "../../ui";

interface Props {
  event: CalendarEvent | null;
  /** The event's calendar, as the list names it. */
  calendarName: string | null;
  /** When it happens, as the popover said it. */
  when: string;
  onConfirm: (event: CalendarEvent) => void;
  onClose: () => void;
}

export function DeleteEventDialog({ event, calendarName, when, onConfirm, onClose }: Props) {
  const title = event ? event.summary.trim() || "(no title)" : "";
  return (
    <ConfirmDialog
      open={event !== null}
      title="Delete this event?"
      confirmLabel="Delete"
      danger
      onConfirm={() => event && onConfirm(event)}
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
