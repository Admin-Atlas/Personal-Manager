// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Opening a copy of an event (#884, C18): reads the original from Google, never the mirror, then
// hands its fields to the full editor's create mode. Nothing is sent: the read opens an editor
// session that is never saved through, and lapses with the others.

import { useEffect, useRef, useState } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { loadText } from "../../../lib/calendarEdit/editReasons";
import { seedDuplicate, type EventCopy } from "../../../lib/calendarEdit/eventDraft";
import { isRepeating } from "../../../lib/calendarEdit/saveChoices";
import { Button, Callout, Dialog, Skeleton } from "../../ui";
import { openForEdit } from "./useEventWrites";

interface Props {
  /** The row the user chose to duplicate. */
  row: CalendarEvent;
  /** Google's copy read: the new event's fields and what the copy leaves out. */
  onReady: (copy: EventCopy) => void;
  onClose: () => void;
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export function CopyOpening({ row, onReady, onClose }: Props) {
  const [error, setError] = useState<string | null>(null);
  const title = row.summary.trim() || "(no title)";
  // The row is the safer sign of a repeating event: the series is null when Google's copy of it
  // couldn't be read.
  const repeating = isRepeating(row);
  // The latest `onReady`: a new one from the parent's render is no reason to read Google again.
  const ready = useRef(onReady);
  useEffect(() => {
    ready.current = onReady;
  });

  useEffect(() => {
    let alive = true;
    openForEdit(row.id)
      .then((load) => {
        if (!alive) return;
        if (load.outcome !== "ready") {
          setError(loadText(load));
          return;
        }
        ready.current(seedDuplicate(load.event, load.series, repeating));
      })
      .catch((e) => {
        if (alive) setError(message(e));
      });
    return () => {
      alive = false;
    };
  }, [row.id, repeating]);

  return (
    <Dialog
      open
      onClose={onClose}
      chrome="bar"
      title="Duplicate event"
      closeLabel="Close"
      widthClassName="max-w-lg"
      footer={
        error ? (
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
        ) : undefined
      }
    >
      {error ? (
        <Callout tone="danger" live>
          {error}
        </Callout>
      ) : (
        <div className="space-y-3" aria-busy="true">
          <p className="text-sm text-ink3">Copying “{title}” from Google…</p>
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-20 w-full" />
        </div>
      )}
    </Dialog>
  );
}
