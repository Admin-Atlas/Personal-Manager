// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The editor's video-call row (#884, C18): add or remove a Google Meet call, in Google Calendar's
// words; another service's call shown with its name and Join, never changed. The link opens through
// `openUrl`, with Copy link beside it as wherever a button sends a page to the browser; the call's
// name and link render as text. After Add, Don't add, Remove or Keep it, focus moves to the button
// that took the pressed one's place, and the line saying what Save will do is read out.

import { useId, useLayoutEffect, useRef, type ReactNode } from "react";
import type { CallForEdit } from "../../../lib/types";
import { openUrl } from "../../../lib/ipc";
import { Button, CopyLinkButton } from "../../ui";

interface Props {
  /** The call Google holds now; null for none. */
  call: CallForEdit | null;
  /** Whether the event will have a Meet call after Save (the draft's `meet`). */
  value: boolean;
  /** The calendar offers Google Meet. */
  offered: boolean;
  /** Turns off Add, Remove, Don't add and Keep it; Join stays, so anyone can still join. */
  disabled?: boolean;
  onChange: (meet: boolean) => void;
  /** Words to show under the row. */
  note?: string | null;
}

/** The link as it reads, without its scheme. */
const shownLink = (link: string) => link.replace(/^https:\/\//, "");

export function MeetField({ call, value, offered, disabled = false, onChange, note }: Props) {
  const labelId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  // Set by a press, so focus follows to the button that replaced it; never on a value the editor
  // set itself (a reload).
  const pressed = useRef(false);
  useLayoutEffect(() => {
    if (!pressed.current) return;
    pressed.current = false;
    toggleRef.current?.focus();
  }, [value]);
  const toggle = (meet: boolean) => {
    pressed.current = true;
    onChange(meet);
  };

  const meet = call?.kind === "meet" ? call : null;
  // Another service's call, Google's old Hangouts or a bare link: shown and kept (`other_call`).
  const other = call && call.kind !== "meet" ? call : null;
  const failed = meet?.state === "failed";
  const pending = meet?.state === "pending";
  // A Meet call Google holds, made or being made: Remove takes it off.
  const held = meet !== null && !failed;
  // Nothing to show and nothing to do: a calendar without Meet, and no call.
  if (!other && !held && !value && !offered) return null;

  const join = (label: string, link: string) => (
    <span className="flex min-w-0 flex-wrap items-center gap-2">
      <Button variant="secondary" size="sm" onClick={() => void openUrl(link)}>
        {label}
      </Button>
      {/* For joining from another browser, or sending the link on. */}
      <CopyLinkButton url={link} to="the call" />
    </span>
  );

  let status: string | null = null;
  let body: ReactNode = null;
  if (other) {
    body = (
      <div className="flex flex-col items-start gap-1">
        <span className="break-words text-sm text-ink2">{other.name || "Video call"}</span>
        {other.join && join("Join", other.join)}
      </div>
    );
  } else if (held) {
    if (pending) {
      status = "Google is still making the video call. Its link shows after a later refresh.";
    }
    if (!value) status = "The video call will be removed when you save.";
    body = (
      <div className="flex flex-col items-start gap-1.5">
        {value && meet.join && (
          <>
            {join("Join with Google Meet", meet.join)}
            <span className="break-all text-xs text-ink3">{shownLink(meet.join)}</span>
          </>
        )}
        {value ? (
          <Button
            ref={toggleRef}
            variant="tertiary"
            size="sm"
            aria-label="Remove the video call"
            disabled={disabled}
            onClick={() => toggle(false)}
          >
            Remove
          </Button>
        ) : (
          <Button
            ref={toggleRef}
            variant="secondary"
            size="sm"
            disabled={disabled}
            onClick={() => toggle(true)}
          >
            Keep it
          </Button>
        )}
      </div>
    );
  } else if (value) {
    status = "Google Meet video conferencing. Google makes the link when you save.";
    body = (
      <Button
        ref={toggleRef}
        variant="tertiary"
        size="sm"
        aria-label="Don't add the video call"
        disabled={disabled}
        onClick={() => toggle(false)}
      >
        Don't add
      </Button>
    );
  } else if (offered) {
    body = (
      <div className="flex flex-col items-start gap-1">
        {failed && <p className="text-xs text-st-due">Google couldn't make the video call.</p>}
        <Button
          ref={toggleRef}
          variant="secondary"
          size="sm"
          disabled={disabled}
          onClick={() => toggle(true)}
        >
          Add Google Meet video conferencing
        </Button>
      </div>
    );
  }

  return (
    <div role="group" aria-labelledby={labelId} className="flex flex-col items-start gap-1">
      <span id={labelId} className="text-sm text-ink2">
        Video call
      </span>
      {/* What Save will do, read out as it changes: always there, so a screen reader hears the
          first line too. */}
      <p aria-live="polite" className="text-xs text-ink3">
        {status}
      </p>
      {body}
      {note && <p className="text-xs text-ink3">{note}</p>}
    </div>
  );
}
