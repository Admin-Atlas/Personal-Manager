// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Google Calendar's "Edit recurring event" / "Delete recurring event" dialog, in PM's words (#884):
// "This event", "This and following events" or "All events", each planned against Google by the
// backend, with what choosing it would do. Nothing is sent until Save or Delete. First focus is on
// Back (or Cancel), so Enter on open closes it rather than choosing (plan Q5). It imports no write
// wrapper: the parent does the writes through `useEventWrites`.
//
// Where it differs from Google's (C9b-1 spec §4.5): a row Google wouldn't allow is shown disabled
// with the reason rather than hidden; the selected row's warnings show under the rows, in one live
// region, so a radio's spoken name stays one line; and a change PM couldn't finish earlier gets one
// settle block, since Google never leaves a change half done.
//
// A row that would email guests also asks whether Google sends the emails (C12), in this dialog
// rather than a second one after it (Google asks in turn, and gives no counts): Save waits for the
// answer, and picking another row asks again, since the answer is to the numbers on screen.

import { useRef, useState } from "react";
import {
  againText,
  defaultScope,
  pendingOf,
  pendingText,
  rowsOf,
  warningLines,
  type ChoiceKind,
  type Picked,
  type ReadyChoices,
} from "../../../lib/calendarEdit/saveChoices";
import {
  CALLOUT_TONE,
  SPLIT_ORDER,
  splitActionLabel,
  type Tone,
} from "../../../lib/calendarEdit/editReasons";
import { notifyOf } from "../../../lib/calendarEdit/guests";
import type { Notify, SeriesScope, SplitAction } from "../../../lib/types";
import { Button, Callout, Dialog, RadioChoice, Skeleton } from "../../ui";
import { NotifyChoice } from "./NotifyChoice";

/** What the dialog shows: the choices loading (several reads of Google), ready, or refused. */
export type ChoicesView =
  | { state: "loading" }
  /** `again`: a write answered that the plan changed, so this is the re-ask. `notice`: words the
   *  backend says once (a change PM was making when it closed), kept by the parent across a re-ask. */
  | { state: "ready"; choices: ReadyChoices; again: boolean; notice: string | null }
  | { state: "refused"; tone: Tone; text: string };

interface Props {
  kind: ChoiceKind;
  /** The event's title as the user sees it (the draft's, for a save). */
  title: string;
  view: ChoicesView;
  /** A settle of the pending change is on its way: everything is disabled meanwhile. */
  settling: boolean;
  /** A delete: when the occurrence is, as the popover said it, and its calendar's name. */
  when?: string;
  calendarName?: string | null;
  /** The occurrence's day as the user sees it (DD-MM), which a delete's "This event" names. */
  day?: string;
  /** The address Google sends guests' emails from (the account), for a row that would email them. */
  from?: string | null;
  /** A delete's own sentence beside what Undo can do: a birthday's, that Google Contacts keeps it
   *  (C20). */
  deleteNote?: string | null;
  /** The row picked, and Send or Don't send when the row would email guests (null when it wouldn't:
   *  nothing was asked). */
  onChoose: (picked: Picked, notify: Notify | null) => void;
  onSettle: (token: string, action: SplitAction) => void;
  onClose: () => void;
}

/** Rendered only while open (the parent mounts it), so its selection starts afresh each time. */
export function SaveChoicesDialog({
  kind,
  title,
  view,
  settling,
  when,
  calendarName,
  day,
  from = null,
  deleteNote = null,
  onChoose,
  onSettle,
  onClose,
}: Props) {
  const name = title.trim() || "(no title)";
  const choices = view.state === "ready" ? view.choices : null;
  const [picked, setPicked] = useState<SeriesScope | null>(() =>
    choices ? defaultScope(choices) : null,
  );
  // Send or Don't send, for the numbers on screen: none until the user picks, and none again
  // whenever the row or the backend's answer changes (R9).
  const [notify, setNotify] = useState<Notify | null>(null);
  // A new answer from the backend picks afresh; anything else re-rendering the parent (a notice
  // expiring, an event list loading) keeps the user's pick (C9b-1 critique 1 M5).
  const [pickedFor, setPickedFor] = useState(choices);
  const sent = useRef(false);
  if (choices !== pickedFor) {
    setPickedFor(choices);
    setPicked(choices ? defaultScope(choices) : null);
    setNotify(null);
    sent.current = false;
  }
  const pick = (scope: SeriesScope) => {
    setPicked(scope);
    setNotify(null);
  };

  const rows = choices ? rowsOf(choices, kind, day) : [];
  const selected = rows.find((r) => r.scope === picked) ?? null;
  const pending = choices ? pendingOf(choices) : null;
  const lines = selected?.selectable ? warningLines(selected, kind, name) : [];
  const guests = selected?.selectable ? selected.guests : null;
  const canSend =
    choices !== null &&
    selected !== null &&
    selected.selectable &&
    !settling &&
    (guests === null || notify !== null);
  // Nothing picked because "This event" can't be (visibility, say): its reason, where a keyboard
  // user hears it, since a disabled radio is skipped (C9b-1 critique 2 M7).
  const thisRefused =
    choices && defaultScope(choices) === null && choices.this.row === "refused"
      ? choices.this.message
      : null;

  function send() {
    // Once: a second click before the parent unmounts the dialog sends nothing.
    if (!canSend || sent.current || !selected?.token) return;
    sent.current = true;
    onChoose(
      { scope: selected.scope, token: selected.token, asAll: selected.asAll },
      guests !== null && notify !== null ? notifyOf(notify) : null,
    );
  }

  const lead =
    kind === "save"
      ? `Which occurrences of “${name}” should change?`
      : `Which occurrences of “${name}”${when ? ` (${when})` : ""} should be deleted${
          calendarName ? ` from ${calendarName} in Google Calendar` : ""
        }?`;

  return (
    <Dialog
      open
      onClose={onClose}
      title={kind === "save" ? "Edit repeating event" : "Delete repeating event"}
      tone={kind === "delete" ? "danger" : "default"}
      helpId="calendar-choices"
      widthClassName="max-w-md"
      footer={
        <>
          <Button variant="tertiary" data-autofocus onClick={onClose}>
            {kind === "save" ? "Back" : "Cancel"}
          </Button>
          <Button
            variant={kind === "delete" ? "danger" : "primary"}
            disabled={!canSend}
            onClick={send}
          >
            {kind === "save" ? "Save" : "Delete"}
          </Button>
        </>
      }
    >
      <p className="mt-2 text-sm text-ink3">
        {lead}
        {thisRefused && ` ${thisRefused}`}
      </p>
      {view.state === "ready" && view.again && (
        <Callout tone="warning" size="md" live className="mt-3">
          {againText(kind)}
        </Callout>
      )}
      {view.state === "ready" && view.notice && (
        <Callout tone="warning" size="md" body="ink" className="mt-3">
          {view.notice}
        </Callout>
      )}
      {view.state === "loading" && (
        <div className="mt-3 space-y-2" aria-busy="true">
          <p className="text-sm text-ink3">Checking “{name}” in Google…</p>
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
        </div>
      )}
      {view.state === "refused" && (
        <Callout as="p" tone={CALLOUT_TONE[view.tone]} size="md" className="mt-3" live>
          {view.text}
        </Callout>
      )}
      {choices && (
        <fieldset className="mt-3">
          <legend className="sr-only">Occurrences</legend>
          {rows.map((row) => (
            <RadioChoice
              key={row.scope}
              name="series-scope"
              value={row.scope}
              current={picked}
              onSelect={pick}
              label={row.label}
              detail={row.detail}
              disabled={!row.selectable || settling}
            />
          ))}
        </fieldset>
      )}
      {pending && (
        <Callout tone="warning" size="md" body="ink" live={false} className="mt-3">
          <p>{pendingText(pending.offers, pending.from, pending.emails)}</p>
          <div className="mt-2 flex flex-wrap gap-2">
            {SPLIT_ORDER.filter((a) => pending.offers.includes(a)).map((action) => (
              <Button
                key={action}
                variant="secondary"
                size="xs"
                disabled={settling}
                onClick={() => onSettle(pending.token, action)}
              >
                {splitActionLabel(action)}
              </Button>
            ))}
          </div>
        </Callout>
      )}
      {/* Always mounted, so a screen reader hears the lines as the selection changes. */}
      <div role="status">
        {lines.length > 0 && (
          <Callout tone="info" size="md" body="ink" live={false} className="mt-3">
            {lines.map((line) => (
              <p key={line}>{line}</p>
            ))}
          </Callout>
        )}
      </div>
      {guests !== null && selected !== null && (
        <NotifyChoice
          notice={guests}
          kind={kind}
          scope={selected.scope}
          from={from}
          value={notify}
          onChange={setNotify}
          disabled={settling}
          legend="shown"
        />
      )}
      {kind === "delete" && (
        <p className="mt-3 text-xs text-ink4">
          You can undo it for a few seconds. If you quit PM in that time, nothing is deleted.
          {deleteNote && ` ${deleteNote}`}
        </p>
      )}
    </Dialog>
  );
}
