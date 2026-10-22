// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Google Calendar's "Send update emails to guests?", in PM's words (#884, C12): who would get what,
// then Send or Don't send, with nothing picked until the user picks (R9: no default, every time).
// One component for the one-off prompt (`NotifyPrompt`), the delete and the which-occurrences dialog
// (`SaveChoicesDialog`); the caller keeps the answer and decides when it's needed. Where it differs
// from Google's (C12 spec §0.5): two radios with the counts and what each does, not three buttons.

import { useId } from "react";
import {
  dontSendDetail,
  guestLines,
  notifyQuestion,
  sendDetail,
  type NotifyKind,
} from "../../../lib/calendarEdit/guests";
import type { GuestNotice, Notify, SeriesScope } from "../../../lib/types";
import { RadioChoice } from "../../ui";

interface Props {
  /** Who Google would email with Send. */
  notice: GuestNotice;
  kind: NotifyKind;
  /** The occurrences chosen, for a repeating event. */
  scope: SeriesScope | null;
  /** The address Google sends from (the account). */
  from: string | null;
  /** The answer so far: null until the user picks. */
  value: Notify | null;
  onChange: (n: Notify) => void;
  disabled: boolean;
  /** The question as the legend, or only for screen readers when a dialog title already asks it. */
  legend: "shown" | "hidden";
}

export function NotifyChoice({
  notice,
  kind,
  scope,
  from,
  value,
  onChange,
  disabled,
  legend,
}: Props) {
  // Its own radio group: two of these mounted at once never share a name or an id (C12 critique
  // C1, C2-L4).
  const id = useId();
  const lines = guestLines(notice, scope, kind);
  const linesId = `${id}-lines`;
  return (
    <fieldset className="mt-3" aria-describedby={lines.length > 0 ? linesId : undefined}>
      <legend className={legend === "shown" ? "text-sm text-ink2" : "sr-only"}>
        {notifyQuestion(notice, kind)}
      </legend>
      {lines.length > 0 && (
        <div id={linesId}>
          {lines.map((line) => (
            <p key={line} className="text-xs text-ink3">
              {line}
            </p>
          ))}
        </div>
      )}
      <div className="mt-1">
        <RadioChoice
          name={`guest-notify-${id}`}
          value="all"
          current={value}
          onSelect={onChange}
          label="Send"
          detail={sendDetail(from)}
          disabled={disabled}
        />
        <RadioChoice
          name={`guest-notify-${id}`}
          value="none"
          current={value}
          onSelect={onChange}
          label="Don't send"
          detail={dontSendDetail(kind)}
          disabled={disabled}
        />
      </div>
    </fieldset>
  );
}
